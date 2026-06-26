//! The grader library — the first line of evaluation (deterministic, free, fast). Graders assert
//! OUTCOMES and observable state, not the exact tool path ("a validator ran before the done-claim",
//! not "the 3rd call was subagent_start"). Each returns short evidence in `detail` so a failed run
//! is triageable straight from the report. Proven in the `cargo test` graders module.

use std::path::Path;
use std::process::Command;

use regex::Regex;

use crate::design::{self, DesignLock, Pool};
use crate::eval::transcript::{Check, CheckOutcome, EvalTranscript, TimelineEntry};
use crate::manager::driver::apply_no_reply;
use crate::stack::StackProfile;

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

// ---- functional graders (profile-driven: any stack, one code path) -------------

/// The app's own test suite is green in the final workspace, run via the active stack's `test_cmd`
/// (`bin/rails test`, `node --test`, …).
pub fn tests_green(profile: &StackProfile, name: &str) -> Check {
    let cmd = profile.test_cmd.clone();
    let name = name.to_string();
    Check::new(name, move |t| run_shell(&cmd, &t.workspace_dir))
}

/// Boot the app via the stack's `serve_exec` and assert a GET on `path` returns `status`. Picks a
/// free port, runs the stack's optional `prepare` step, boots the server, waits for its `health_path`
/// to answer 200, probes the target path, then stops the server.
pub fn http_probe(profile: &StackProfile, path: &str, status: u16, name: &str) -> Check {
    let script = serve_probe_script(profile, path, status);
    let name = name.to_string();
    Check::new(name, move |t| run_shell(&script, &t.workspace_dir))
}

/// Render the boot-poll-probe-kill shell script from a stack profile. `serve_exec` binds localhost
/// and reads `${APP_PORT}`/`$PORT` (both exported); `serve_env` becomes `export` lines; `prepare` (if
/// any) runs failure-tolerantly first; `health_path` gates readiness.
fn serve_probe_script(profile: &StackProfile, path: &str, status: u16) -> String {
    let port = free_port();
    let mut env_exports = String::new();
    for (k, v) in &profile.serve_env {
        env_exports.push_str(&format!("export {k}={v}\n"));
    }
    let prepare = if profile.probe_prepare.is_empty() {
        String::new()
    } else {
        format!("{} >/dev/null 2>&1 || true\n", profile.probe_prepare)
    };
    let serve = &profile.serve_exec;
    let health = &profile.health_path;
    format!(
        r#"set -e
{env_exports}export APP_PORT={port}
export PORT={port}
export APP_HOST=127.0.0.1
{prepare}{serve} >/tmp/lila-eval-serve.$$.log 2>&1 &
PID=$!
ok=0
for i in $(seq 1 40); do
  c=$(curl -s -o /dev/null -w '%{{http_code}}' "http://127.0.0.1:{port}{health}" 2>/dev/null || true)
  [ "$c" = "200" ] && {{ ok=1; break; }}
  sleep 0.5
done
code=$(curl -s -o /dev/null -w '%{{http_code}}' "http://127.0.0.1:{port}{path}" 2>/dev/null || true)
kill $PID 2>/dev/null || true
wait $PID 2>/dev/null || true
[ "$ok" = 1 ] || {{ echo "app did not boot (see /tmp/lila-eval-serve.$$.log)"; exit 2; }}
[ "$code" = "{status}" ] || {{ echo "GET {path} -> $code (wanted {status})"; exit 1; }}"#
    )
}

// ---- the aesthetic axis (looks_designed) -------------------------------------------------------
//
// The §G v1 grader, a sibling of `http_probe` / `tests_green` over the SAME captured workspace. Taste
// has no oracle, so the LLM-judged half rides the scenario `rubric` (scored separately, never gating);
// THIS grader is the falsifiable, offline, reproducible floor: the app faithfully applied its *locked*
// system rather than sprinkling slop. Per the repo's evidence-not-claims bar it asserts concrete
// signals — tokens defined + referenced, the lock present, no raw hex outside `tokens.css` — not an
// adjective. This grader is Rails-shaped (it reads `app/views` + `app/assets/stylesheets`); it only
// runs on the design scenarios, which are pinned to the rails-pwa stack.

/// The canonical Rails token sink the design baseline renders into.
const RAILS_TOKENS_PATH: &str = "app/assets/stylesheets/tokens.css";

/// The app cleanly applied its locked design system: tokens rendered + referenced, `design.lock`
/// present, and no raw hex colors sprinkled into views/stylesheets (the canonical anti-slop signal).
pub fn looks_designed(name: &str) -> Check {
    let name = name.to_string();
    Check::new(name, move |t| {
        grade_design(&t.workspace_dir, RAILS_TOKENS_PATH)
    })
}

fn grade_design(ws: &Path, tokens_rel: &str) -> CheckOutcome {
    let Ok(tokens) = std::fs::read_to_string(ws.join(tokens_rel)) else {
        return CheckOutcome::fail(format!("{tokens_rel} missing (nothing installed)"));
    };
    // Open Design's curated tokens.css always defines the schema's `--accent` on `:root`.
    if !tokens.contains("--accent") {
        return CheckOutcome::fail(format!("{tokens_rel} defines no --accent token"));
    }
    if read_design_lock(ws).is_none() {
        return CheckOutcome::fail("no readable design.lock (look not locked)");
    }
    match raw_hex_in_views(ws, tokens_rel) {
        Some(hit) => CheckOutcome::fail(format!("raw hex outside tokens — slop signal: {hit}")),
        None => CheckOutcome::pass_with("tokens defined + locked; no raw hex in views"),
    }
}

/// First `file: #hex` found in `app/views` or `app/assets/stylesheets` (excluding the tokens file
/// itself), or `None` if the UI references tokens cleanly. Authored regex ⇒ `expect` is a build guard.
#[allow(clippy::expect_used)]
fn raw_hex_in_views(ws: &Path, tokens_rel: &str) -> Option<String> {
    let hex = Regex::new(r"#[0-9a-fA-F]{6}\b|#[0-9a-fA-F]{3}\b").expect("hex regex");
    let skip = ws.join(tokens_rel);
    ["app/views", "app/assets/stylesheets"]
        .iter()
        .find_map(|d| find_hex(&ws.join(d), &hex, &skip))
}

/// Recurse `dir` for the first file (other than `skip`) containing a raw hex color.
fn find_hex(dir: &Path, hex: &Regex, skip: &Path) -> Option<String> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(hit) = find_hex(&path, hex, skip) {
                return Some(hit);
            }
        } else if path != skip && std::fs::read_to_string(&path).is_ok_and(|b| hex.is_match(&b)) {
            return Some(path.file_name()?.to_string_lossy().into_owned());
        }
    }
    None
}

// ---- design selection-flow graders (deterministic: read state, not pixels) ---------------------
//
// The aesthetic axis (`looks_designed`) needs an LLM judge because taste has no oracle. The selection
// FLOW is the opposite: draw → lock → invite → choose is a state machine whose entire ground truth is
// the committed `design.lock` plus the manager's delivered messages. So it gets cheap, exact,
// non-flaky graders — siblings of `tests_green` / `http_probe`.

/// The stable, detectable shape of the one-time look invitation (§F.3): an offer that names the look
/// and invites the owner to pick or change it. Counted by [`invitation_count`]; matched
/// case-insensitively (via [`re`]).
pub const DESIGN_INVITATION: &str = r"(look|design|theme|style|vibe|aesthetic)[^.?!]{0,90}(want|fancy|prefer|pick|choose|change|swap|more personality|something (?:like|bolder|warmer|different))";

/// Read + parse the workspace `design.lock`, if present and well-formed.
fn read_design_lock(ws: &Path) -> Option<DesignLock> {
    DesignLock::parse(&std::fs::read_to_string(ws.join("design.lock")).ok()?).ok()
}

/// The active system's pool is `expected` AND its brand really belongs to that pool per the catalog
/// INDEX (nesting applies). Confirms the blind draw can only land in the bounded pool.
pub fn design_lock_pool(expected: &str) -> Check {
    let expected = expected.to_string();
    Check::new(
        format!("design.lock pool == {expected}"),
        move |t| match check_pool(&t.workspace_dir, &expected) {
            Ok(detail) => CheckOutcome::pass_with(detail),
            Err(e) => CheckOutcome::fail(e),
        },
    )
}

fn check_pool(ws: &Path, expected: &str) -> Result<String, String> {
    let lock = read_design_lock(ws).ok_or_else(|| "no readable design.lock".to_string())?;
    let want = Pool::parse(expected).ok_or_else(|| format!("bad expected pool {expected:?}"))?;
    if lock.pool != want {
        return Err(format!(
            "pool is {} (wanted {expected})",
            lock.pool.as_str()
        ));
    }
    if brand_in_pool(&lock.brand, want)? {
        Ok(format!("{} ∈ {expected}", lock.brand))
    } else {
        Err(format!("{} is not a member of {expected}", lock.brand))
    }
}

/// Is `brand` a member of the queried pool (with nesting) per `design/systems/INDEX.md`?
fn brand_in_pool(brand: &str, want: Pool) -> Result<bool, String> {
    let dir = design::catalog_dir();
    let entries = design::load_index(&dir).map_err(|e| e.to_string())?;
    Ok(entries
        .iter()
        .find(|e| e.brand == brand)
        .is_some_and(|e| want.admits(e.pool)))
}

/// The `source` field of `design.lock` (the selection-flow state) equals `expected`
/// (`default` | `invited` | `chosen` | `pinned`).
pub fn design_source_is(expected: &str) -> Check {
    let expected = expected.to_string();
    Check::new(
        format!("design.lock source == {expected}"),
        move |t| match read_design_lock(&t.workspace_dir) {
            Some(lock) if lock.source == expected => CheckOutcome::pass_with(lock.source),
            Some(lock) => {
                CheckOutcome::fail(format!("source is {} (wanted {expected})", lock.source))
            }
            None => CheckOutcome::fail("no readable design.lock"),
        },
    )
}

/// The lock did not drift during the run: `design.lock` is byte-identical to its committed baseline
/// (no per-render switching, no reroll on restart). Uses `git diff` against the standup commit.
pub fn design_lock_stable() -> Check {
    Check::new("design.lock unchanged (no reroll/drift)", |t| {
        let out = Command::new("git")
            .args(["diff", "--quiet", "--", "design.lock"])
            .current_dir(&t.workspace_dir)
            .status();
        match out {
            Ok(s) if s.success() => CheckOutcome::pass_with("design.lock unchanged"),
            Ok(_) => CheckOutcome::fail("design.lock changed since standup (drift/reroll)"),
            Err(e) => CheckOutcome::fail(format!("git diff: {e}")),
        }
    })
}

/// The number of look-offers in the owner-visible messages equals `n` — the invitation fires *at most
/// once* and never nags (§F.3).
pub fn invitation_count(n: usize) -> Check {
    let needle = re(DESIGN_INVITATION);
    Check::new(format!("exactly {n} design invitation(s)"), move |t| {
        let got = t.deliveries.iter().filter(|d| needle.is_match(d)).count();
        if got == n {
            CheckOutcome::pass_with(format!("{got} offer(s)"))
        } else {
            CheckOutcome::fail(format!("{got} offer(s), wanted {n}"))
        }
    })
}

/// A free localhost TCP port (bind :0, read it, release) so concurrent leftovers can't collide.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map_or(39_517, |a| a.port())
}

/// Run a shell script in `cwd` (inherits the process env, so PATH must carry the stack's toolchain —
/// Ruby for Rails, Node for node-react). Ok(()) on exit 0; Err(clipped combined output) otherwise.
/// Scripts here are self-bounding (the probe polls then kills the server; `*test` commands
/// terminate), so no external timeout is needed.
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
