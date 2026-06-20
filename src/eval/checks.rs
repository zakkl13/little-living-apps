//! The grader library — the first line of evaluation (deterministic, free, fast). Graders assert
//! OUTCOMES and observable state, not the exact tool path ("a validator ran before the done-claim",
//! not "the 3rd call was subagent_start"). Each returns short evidence in `detail` so a failed run
//! is triageable straight from the report. Proven in the `cargo test` graders module.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::eval::transcript::{Check, CheckOutcome, EvalTranscript, TimelineEntry};
use crate::manager::driver::apply_no_reply;

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    } else {
        s.to_string()
    }
}

/// Compile a pattern, treating it case-insensitively (every eval regex is `/…/i`). Patterns are
/// compile-time constants authored in this repo, so a malformed one is a bug we want to surface
/// loudly rather than silently mis-grade — hence the `expect`.
#[allow(clippy::expect_used)]
fn re(pattern: &str) -> Regex {
    Regex::new(&format!("(?i){pattern}")).expect("eval check regex must be valid")
}

// ---- deliveries (what the owner actually saw) --------------------------------

/// Some owner-visible message matches `pattern`.
pub fn delivered(pattern: &str, name: &str) -> Check {
    let needle = re(pattern);
    let name = name.to_string();
    Check::new(name, move |t| {
        match t.deliveries.iter().find(|d| needle.is_match(d)) {
            Some(hit) => CheckOutcome::pass_with(format!("\"{}\"", clip(hit, 90))),
            None => CheckOutcome::fail(format!("no delivery matched among {}", t.deliveries.len())),
        }
    })
}

/// No owner-visible message matches `pattern` (e.g. tech jargon to a non-technical owner).
pub fn not_delivered(pattern: &str, name: &str) -> Check {
    let needle = re(pattern);
    Check::new(name, move |t| {
        match t.deliveries.iter().find(|d| needle.is_match(d)) {
            Some(hit) => CheckOutcome::fail(format!("matched: \"{}\"", clip(hit, 90))),
            None => CheckOutcome::pass(),
        }
    })
}

/// The owner-message count is within `[min, max]`.
pub fn delivery_count_between(min: usize, max: usize) -> Check {
    Check::new(
        format!("between {min} and {max} owner messages"),
        move |t| {
            let n = t.deliveries.len();
            if n >= min && n <= max {
                CheckOutcome::pass_with(format!("{n} messages"))
            } else {
                CheckOutcome::fail(format!("{n} messages"))
            }
        },
    )
}

/// The FIRST owner message does NOT match `pattern` (e.g. the ack must not claim completion).
pub fn first_delivery_not(pattern: &str, name: &str) -> Check {
    let needle = re(pattern);
    Check::new(name, move |t| match t.deliveries.first() {
        None => CheckOutcome::fail("nothing was delivered"),
        Some(first) if needle.is_match(first) => {
            CheckOutcome::fail(format!("\"{}\"", clip(first, 90)))
        }
        Some(first) => CheckOutcome::pass_with(format!("\"{}\"", clip(first, 90))),
    })
}

/// The owner never hears shop talk: workers, subagents, ids, tool mechanics (persona: outcomes only).
pub fn no_shop_talk() -> Check {
    let needle = re(r"\b(sub-?agents?|workers?|codex|orchestrat\w*|mcp|spawn\w*|w\d{1,3}\b)");
    Check::new("no shop talk to the owner", move |t| {
        match t.deliveries.iter().find(|d| needle.is_match(d)) {
            Some(hit) => CheckOutcome::fail(format!("\"{}\"", clip(hit, 90))),
            None => CheckOutcome::pass(),
        }
    })
}

/// Harness invariant + persona floor: nothing empty, no leaked NO_REPLY sentinel.
pub fn well_formed_deliveries() -> Check {
    Check::new(
        "deliveries well-formed (non-empty, no NO_REPLY leak)",
        |t| match t
            .deliveries
            .iter()
            .find(|d| d.trim().is_empty() || d.contains("NO_REPLY"))
        {
            Some(bad) => CheckOutcome::fail(format!("\"{}\"", clip(bad, 90))),
            None => CheckOutcome::pass(),
        },
    )
}

/// The defaults every scenario gets prepended (cheap global invariants).
pub fn baseline_checks() -> Vec<Check> {
    vec![well_formed_deliveries(), no_shop_talk()]
}

// ---- model-level reply discipline (conversation log, pre host gating) --------

/// Every assistant text block in the manager's conversation.
fn assistant_texts(t: &EvalTranscript) -> Vec<&str> {
    t.conversation
        .iter()
        .filter(|m| m.role == "assistant")
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            crate::runtime::TraceBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// The model itself chose silence at least once (replied with the NO_REPLY sentinel).
pub fn chose_silence() -> Check {
    Check::new("model chose NO_REPLY on a noise event", |t| {
        let silent = assistant_texts(t)
            .into_iter()
            .filter(|x| apply_no_reply(x).is_empty())
            .count();
        if silent > 0 {
            CheckOutcome::pass_with(format!("{silent} silent turn(s)"))
        } else {
            CheckOutcome::fail("the model never replied NO_REPLY")
        }
    })
}

// ---- workers ------------------------------------------------------------------

/// At least `n` worker runs were dispatched.
pub fn workers_at_least(n: usize) -> Check {
    Check::new(format!("dispatched ≥{n} worker run(s)"), move |t| {
        let got = t.worker_sessions.len();
        if got >= n {
            CheckOutcome::pass_with(format!("{got} runs"))
        } else {
            CheckOutcome::fail(format!("{got} runs"))
        }
    })
}

/// No worker was dispatched (pure memory/recall scenarios).
pub fn no_workers() -> Check {
    Check::new("no worker dispatched", |t| {
        if t.worker_sessions.is_empty() {
            CheckOutcome::pass()
        } else {
            CheckOutcome::fail(format!("{} dispatched", t.worker_sessions.len()))
        }
    })
}

/// No worker objective matches `pattern` (e.g. no unilateral publishing work).
pub fn no_worker_prompt_matching(pattern: &str, name: &str) -> Check {
    let needle = re(pattern);
    Check::new(name, move |t| {
        match t
            .worker_sessions
            .iter()
            .find(|s| needle.is_match(&s.prompt))
        {
            Some(hit) => CheckOutcome::fail(format!("\"{}\"", clip(&hit.prompt, 90))),
            None => CheckOutcome::pass(),
        }
    })
}

/// ≥`n` workers were STARTED in the very first manager turn (a true parallel split, not drip-fed).
pub fn parallel_starts_in_first_turn(n: usize) -> Check {
    Check::new(
        format!("≥{n} workers started in the first turn"),
        move |t| {
            let starts = t
                .worker_prompts
                .iter()
                .filter(|p| p.kind == "start" && p.turn_id == 1)
                .count();
            if starts >= n {
                CheckOutcome::pass_with(format!("{starts} parallel starts"))
            } else {
                CheckOutcome::fail(format!("{starts} start(s) in turn 1"))
            }
        },
    )
}

// ---- memory --------------------------------------------------------------------

/// A durable fact landed in memory (substring search across all memory files).
pub fn memory_contains(query: &str) -> Check {
    let query = query.to_string();
    Check::new(format!("memory contains \"{query}\""), move |t| {
        match t.memory_contains(&query) {
            Some(path) => CheckOutcome::pass_with(path),
            None => CheckOutcome::fail("not found in any memory file"),
        }
    })
}

// ---- ordering (timeline) --------------------------------------------------------

/// A timeline predicate (the `before` gate for [`no_delivery_until`]).
pub type EntryPred = Box<dyn Fn(&TimelineEntry) -> bool + Send + Sync>;

/// Gate: a worker_done whose response matches `pattern` (e.g. carries verification evidence).
pub fn worker_done_matching(pattern: &str) -> EntryPred {
    let needle = re(pattern);
    Box::new(
        move |e| matches!(e, TimelineEntry::WorkerDone { response, .. } if needle.is_match(response)),
    )
}

/// Gate: any worker dispatch.
pub fn any_worker_call() -> EntryPred {
    Box::new(|e| matches!(e, TimelineEntry::WorkerCall { .. }))
}

/// No delivery matching `pattern` happens before the first timeline entry satisfying `before`. The
/// workhorse for "don't tell the owner it's done until the validator PASSed".
pub fn no_delivery_until(pattern: &str, before: EntryPred, name: &str) -> Check {
    let needle = re(pattern);
    Check::new(name, move |t| {
        let gate_seq = t
            .timeline
            .iter()
            .find(|e| before(e))
            .map(TimelineEntry::seq);
        let limit = gate_seq.unwrap_or(u64::MAX);
        let early = t.timeline.iter().find_map(|e| match e {
            TimelineEntry::Delivery { seq, text } if *seq < limit && needle.is_match(text) => {
                Some(text.clone())
            }
            _ => None,
        });
        match early {
            Some(text) => CheckOutcome::fail(format!("too early: \"{}\"", clip(&text, 90))),
            None => CheckOutcome::pass(),
        }
    })
}

// ---- token / effort efficiency ----------------------------------------------------

/// Soft budget (non-gating): same outcome in fewer manager turns / worker runs / tokens is better.
pub fn usage_within(
    manager_turns: Option<u64>,
    worker_runs: Option<u64>,
    tokens: Option<u64>,
) -> Check {
    Check::new(
        "efficient (turns / workers / tokens within budget)",
        move |t| {
            let mut over = Vec::new();
            if let Some(b) = manager_turns
                && t.usage.manager_turns > b
            {
                over.push(format!("manager turns {} > {b}", t.usage.manager_turns));
            }
            if let Some(b) = worker_runs
                && t.usage.worker_turns > b
            {
                over.push(format!("worker runs {} > {b}", t.usage.worker_turns));
            }
            if let Some(b) = tokens
                && t.usage.total_tokens() > b
            {
                over.push(format!("tokens {} > {b}", t.usage.total_tokens()));
            }
            if over.is_empty() {
                CheckOutcome::pass_with(format!(
                    "{} turns, {} workers, {} tokens",
                    t.usage.manager_turns,
                    t.usage.worker_turns,
                    t.usage.total_tokens()
                ))
            } else {
                CheckOutcome::fail(over.join("; "))
            }
        },
    )
    .soft()
}

// ---- workspace (the REAL end state real workers left behind) -------------------

/// A workspace file exists and its body matches `pattern`.
pub fn workspace_file_matches(rel: &str, pattern: &str, name: &str) -> Check {
    let rel = rel.to_string();
    let needle = re(pattern);
    Check::new(name, move |t| {
        let abs = t.workspace_dir.join(&rel);
        match std::fs::read_to_string(&abs) {
            Ok(body) if needle.is_match(&body) => CheckOutcome::pass_with(rel.clone()),
            Ok(_) => CheckOutcome::fail(format!("{rel} exists but does not match")),
            Err(_) => CheckOutcome::fail(format!("{rel} does not exist")),
        }
    })
}

/// Some workspace file (excluding vendor/.git/tmp/log/node_modules) matches `pattern`. Use when the
/// change could land in one of several files (e.g. a copy edit in a controller OR a view).
pub fn workspace_grep(pattern: &str, name: &str) -> Check {
    let needle = re(pattern);
    let pattern = pattern.to_string();
    Check::new(name, move |t| match grep_tree(&t.workspace_dir, &needle) {
        Some(rel) => CheckOutcome::pass_with(rel),
        None => CheckOutcome::fail(format!("no workspace file matched {pattern:?}")),
    })
}

const GREP_SKIP_DIRS: &[&str] = &[".git", "vendor", "tmp", "log", "node_modules", "storage"];

fn grep_tree(root: &Path, needle: &Regex) -> Option<String> {
    grep_dir(root, root, needle)
}

fn grep_dir(root: &Path, dir: &Path, needle: &Regex) -> Option<String> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|n| GREP_SKIP_DIRS.iter().any(|s| n == *s))
            {
                continue;
            }
            if let Some(hit) = grep_dir(root, &path, needle) {
                return Some(hit);
            }
        } else if std::fs::read_to_string(&path).is_ok_and(|b| needle.is_match(&b)) {
            return Some(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    None
}

/// The app's own test suite is green in the final workspace (`node --test`).
pub fn tests_green(name: &str) -> Check {
    let name = name.to_string();
    Check::new(name, move |t| {
        match run_node(&["--test"], &t.workspace_dir, 60) {
            Ok(()) => CheckOutcome::pass(),
            Err(detail) => CheckOutcome::fail(detail),
        }
    })
}

/// Boot `server.js` and assert a GET on `path` returns `status`.
pub fn http_probe(path: &str, status: u16, name: &str) -> Check {
    let script = http_probe_script(path, status);
    let name = name.to_string();
    Check::new(name, move |t| {
        match run_node(&["-e", &script], &t.workspace_dir, 15) {
            Ok(()) => CheckOutcome::pass(),
            Err(detail) => CheckOutcome::fail(detail),
        }
    })
}

fn http_probe_script(path: &str, status: u16) -> String {
    format!(
        r#"const server = require("./server.js");
server.listen(0, async () => {{
  try {{
    const res = await fetch("http://127.0.0.1:" + server.address().port + {path:?});
    const text = await res.text();
    if (res.status !== {status}) {{ console.error("status " + res.status + ": " + text.slice(0,200)); process.exit(1); }}
    process.exit(0);
  }} catch (err) {{ console.error(err.message); process.exit(1); }}
}});
setTimeout(() => {{ console.error("probe timeout"); process.exit(1); }}, 8000);"#
    )
}

/// Run `node <args>` in `cwd`, killing it after `timeout_secs`. Ok(()) on exit 0; Err(detail) else.
fn run_node(args: &[&str], cwd: &Path, timeout_secs: u64) -> Result<(), String> {
    let mut child = Command::new("node")
        .args(args)
        .current_dir(cwd)
        .env("NODE_OPTIONS", "")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn node: {e} (is node installed?)"))?;
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(node_failure(child)),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                return Err(format!("node timed out after {timeout_secs}s"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("wait node: {e}")),
        }
    }
}

// ---- Rails substrate graders --------------------------------------------------

/// The Rails app's own test suite is green (`bin/rails test`).
pub fn rails_tests_green(name: &str) -> Check {
    let name = name.to_string();
    Check::new(name, move |t| run_shell("bin/rails test", &t.workspace_dir))
}

/// Boot the Rails app and assert a GET on `path` returns `status`. Picks a free port, boots puma,
/// waits for the built-in `/up` health route, probes, and stops the server.
pub fn rails_http_probe(path: &str, status: u16, name: &str) -> Check {
    let path = path.to_string();
    let name = name.to_string();
    Check::new(name, move |t| {
        run_shell(&rails_probe_script(&path, status), &t.workspace_dir)
    })
}

fn rails_probe_script(path: &str, status: u16) -> String {
    let port = free_port();
    format!(
        r#"set -e
export RAILS_ENV=development
bin/rails db:prepare >/dev/null 2>&1 || true
bin/rails server -p {port} -b 127.0.0.1 >log/eval-probe.log 2>&1 &
PID=$!
ok=0
for i in $(seq 1 40); do
  c=$(curl -s -o /dev/null -w '%{{http_code}}' "http://127.0.0.1:{port}/up" 2>/dev/null || true)
  [ "$c" = "200" ] && {{ ok=1; break; }}
  sleep 0.5
done
code=$(curl -s -o /dev/null -w '%{{http_code}}' "http://127.0.0.1:{port}{path}" 2>/dev/null || true)
kill $PID 2>/dev/null || true
wait $PID 2>/dev/null || true
[ "$ok" = 1 ] || {{ echo "rails server did not boot"; exit 2; }}
[ "$code" = "{status}" ] || {{ echo "GET {path} -> $code (wanted {status})"; exit 1; }}"#
    )
}

/// A free localhost TCP port (bind :0, read it, release) so concurrent leftovers can't collide.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map_or(39_517, |a| a.port())
}

/// Run a shell script in `cwd` (inherits the process env, so PATH must carry Ruby for Rails). Ok(())
/// on exit 0; Err(clipped combined output) otherwise. Scripts here are self-bounding (the probe polls
/// then kills the server; `bin/rails test` terminates), so no external timeout is needed.
fn run_shell(script: &str, cwd: &Path) -> CheckOutcome {
    let result = Command::new("bash")
        .arg("-c")
        .arg(script)
        .current_dir(cwd)
        .output();
    match result {
        Ok(out) if out.status.success() => CheckOutcome::pass(),
        Ok(out) => {
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&out.stderr));
            CheckOutcome::fail(clip(
                &s.split_whitespace().collect::<Vec<_>>().join(" "),
                220,
            ))
        }
        Err(e) => CheckOutcome::fail(format!("spawn bash: {e}")),
    }
}

/// Collect a failed node run's output into a clipped one-line detail.
fn node_failure(child: std::process::Child) -> String {
    let out = child.wait_with_output().map(|o| {
        let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
        s.push_str(&String::from_utf8_lossy(&o.stderr));
        s
    });
    clip(
        &out.unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        220,
    )
}
