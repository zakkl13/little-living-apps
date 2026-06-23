//! One trial: boot the COMPILED `lila run` binary against a fake Telegram server with the eval trace
//! on, send the scenario's owner turns one at a time, drain to quiescence via the trace's `idle`
//! marker, then grade the captured transcript + the real on-disk workspace/memory the workers left
//! behind. The single substitution is Telegram (deliveries captured, not sent); everything else —
//! manager thread, MCP, memory, workers — is exactly what ships. Token usage is read from the
//! binary's own snapshot (authoritative manager/worker split).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};

use crate::eval::fake_telegram::FakeTelegram;
use crate::eval::report::TrialReport;
use crate::eval::scenarios::Scenario;
use crate::eval::transcript::{Axis, EvalTranscript, TokenStats, grade, score};
use crate::eval::{checks, fixture, trace_read};
use crate::runtime::SnapshotStore;
use crate::stack::StackProfile;

/// The eval owner's Telegram id (any allowed id; deliveries are captured, never sent).
const EVAL_OWNER_ID: i64 = 77_000_001;
const EVAL_CHAT_ID: i64 = 77_000_001;

/// Knobs shared across a run (production parity is the default; no model/effort overrides).
pub struct HarnessOptions {
    /// `codex` | `claude` — selects the backend the trial boots (production parity each).
    pub backend: String,
    /// Worker sandbox (`CODEX_SANDBOX_MODE`). The eval DEFAULTS to `workspace-write` (safe on a dev
    /// box); pass `danger-full-access` explicitly for prod parity on a disposable host.
    pub sandbox: String,
    /// Global per-trial wall-clock budget (seconds); a scenario may raise its own floor.
    pub timeout_secs: u64,
    /// Hermetic mode: drive the scripted fake backend (no subscription) — used to self-test the
    /// harness machinery itself.
    pub fake: bool,
    /// Keep the trial's temp dirs for forensics.
    pub keep_tmp: bool,
}

/// Per-trial temp layout.
struct TrialDirs {
    _tmp: Option<tempfile::TempDir>,
    root: PathBuf,
    workspace: PathBuf,
    memory: PathBuf,
    state: PathBuf,
    trace: PathBuf,
}

/// Run one trial of `scenario` and return its graded report.
pub async fn run_trial(
    scenario: &Scenario,
    trial: u32,
    opts: &HarnessOptions,
) -> anyhow::Result<TrialReport> {
    let dirs = setup_dirs(opts.keep_tmp)?;
    seed_workspace(&dirs.workspace, scenario)?;
    seed_memory(&dirs.memory, scenario)?;

    let tg = FakeTelegram::start().await?;
    let mut child = spawn_binary(&tg, &dirs, opts, scenario)?;

    let budget = scenario.timeout_secs.max(opts.timeout_secs);
    let deadline = Instant::now() + Duration::from_secs(budget);
    let drive = drive_turns(&tg, &dirs.trace, scenario, deadline).await;
    let _ = child.kill().await; // snapshot is already persisted per-turn, so SIGKILL is lossless here

    let report = assemble(scenario, trial, opts, &dirs);
    let report = match (drive, report) {
        (Ok(()), r) => r,
        (Err(e), mut r) => {
            r.error = Some(e.to_string());
            r.pass = false;
            r
        }
    };
    Ok(report)
}

fn setup_dirs(keep: bool) -> anyhow::Result<TrialDirs> {
    let tmp = tempfile::Builder::new().prefix("lila-eval-").tempdir()?;
    let root = tmp.path().to_path_buf();
    let dirs = TrialDirs {
        workspace: root.join("workspace"),
        memory: root.join("memory"),
        state: root.join("state"),
        trace: root.join("trace.jsonl"),
        root,
        _tmp: if keep { None } else { Some(tmp) },
    };
    std::fs::create_dir_all(&dirs.workspace)?;
    if keep {
        // Leak the TempDir so it survives; report the path for forensics.
        tracing::info!(dir = %dirs.root.display(), "eval --keep-tmp: trial dir retained");
    }
    Ok(dirs)
}

fn seed_workspace(ws: &Path, scenario: &Scenario) -> anyhow::Result<()> {
    let profile = StackProfile::load(&scenario.stack)?;
    fixture::seed_stack(&profile, ws, &scenario.workspace)?;
    if let Some(setup) = scenario.setup {
        setup(ws)?;
    }
    fixture::git_commit_fixture(ws)?;
    Ok(())
}

/// Write the scenario's memory seeds onto disk (the binary's MemFs indexes them on open). The memory
/// dir IS the `/memories` root, so the leading prefix is stripped.
fn seed_memory(mem: &Path, scenario: &Scenario) -> anyhow::Result<()> {
    for (path, body) in &scenario.memory {
        let rel = path
            .trim_start_matches('/')
            .strip_prefix("memories/")
            .unwrap_or(path);
        let abs = mem.join(rel);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(abs, body)?;
    }
    Ok(())
}

fn spawn_binary(
    tg: &FakeTelegram,
    dirs: &TrialDirs,
    opts: &HarnessOptions,
    scenario: &Scenario,
) -> anyhow::Result<Child> {
    let bin = locate_lila_binary()?;
    let path = std::env::var("PATH").unwrap_or_default();
    // Forward the REAL home so the spawned Codex/Claude CLIs find their subscription auth
    // (`~/.codex`, `~/.claude`); fall back to the trial root only when HOME is unset.
    let home = std::env::var("HOME").unwrap_or_else(|_| dirs.root.to_string_lossy().into_owned());
    let mut cmd = Command::new(bin);
    cmd.arg("run")
        .env_clear()
        .env("PATH", path)
        .env("HOME", home)
        .env("TELEGRAM_BOT_TOKEN", "eval-token")
        .env("ALLOWED_USER_IDS", EVAL_OWNER_ID.to_string())
        .env("TELEGRAM_API_BASE_URL", tg.base_url())
        .env("AGENT_BACKEND", &opts.backend)
        .env("CODEX_SANDBOX_MODE", &opts.sandbox)
        .env("LILA_STACK", &scenario.stack)
        .env("WORKSPACE_DIR", &dirs.workspace)
        .env("MEMORY_DIR", &dirs.memory)
        .env("MANAGER_STATE_DIR", &dirs.state)
        .env("LILA_EVAL_TRACE", &dirs.trace)
        .env("LOG_LEVEL", "warn")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if opts.fake {
        cmd.env("LILA_FAKE_BACKEND", "1");
    }
    // Forward subscription auth (Codex/Claude read it from the real home).
    forward_auth_env(&mut cmd);
    Ok(cmd.spawn()?)
}

/// Find the compiled `lila` binary: `$LILA_BIN`, else a sibling of the running executable (both
/// `lila` and `lila-eval` land in the same target dir; integration-test runners live one level down
/// in `deps/`).
fn locate_lila_binary() -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var("LILA_BIN")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no executable dir"))?;
    for candidate in [dir.join("lila"), dir.join("../lila")] {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("could not locate the `lila` binary (build it, or set LILA_BIN)")
}

/// Forward the few env vars the real backends need to reach the subscription (auth lives on disk).
fn forward_auth_env(cmd: &mut Command) {
    for key in ["CODEX_HOME", "CODEX_BIN", "MANAGER_MODEL", "WORKER_MODEL"] {
        if let Ok(val) = std::env::var(key)
            && !val.is_empty()
        {
            cmd.env(key, val);
        }
    }
}

/// Send each owner turn and wait for the cascade it triggers to settle (the trace's `idle` count
/// must increase past where it was before the turn).
async fn drive_turns(
    tg: &FakeTelegram,
    trace: &Path,
    scenario: &Scenario,
    deadline: Instant,
) -> anyhow::Result<()> {
    // Wait for the binary's initial idle (it has booted and is long-polling).
    wait_for_idle(trace, 1, deadline).await?;
    let mut target = count_idle(trace);
    for turn in &scenario.turns {
        tg.push_owner_message(EVAL_OWNER_ID, EVAL_CHAT_ID, turn);
        target += 1;
        wait_for_idle(trace, target, deadline).await?;
    }
    Ok(())
}

/// Count `idle` markers in the trace so far.
fn count_idle(trace: &Path) -> usize {
    std::fs::read_to_string(trace)
        .map(|b| {
            b.lines()
                .filter(|l| l.contains("\"type\":\"idle\""))
                .count()
        })
        .unwrap_or(0)
}

/// Poll until the trace shows ≥ `target` idle markers, or the deadline passes.
async fn wait_for_idle(trace: &Path, target: usize, deadline: Instant) -> anyhow::Result<()> {
    loop {
        if count_idle(trace) >= target {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!("trial timed out waiting for the manager to settle (idle marker)");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Build the transcript from the trace + snapshot + on-disk end state, grade it, assemble the report.
fn assemble(
    scenario: &Scenario,
    trial: u32,
    opts: &HarnessOptions,
    dirs: &TrialDirs,
) -> TrialReport {
    let parsed = trace_read::parse_file(&dirs.trace);
    let usage = SnapshotStore::new(&dirs.state)
        .load()
        .map(|s| s.usage)
        .unwrap_or_default();
    let transcript = EvalTranscript {
        scenario: scenario.name.clone(),
        timeline: parsed.timeline,
        deliveries: parsed.deliveries,
        conversation: parsed.conversation,
        worker_prompts: parsed.worker_prompts,
        worker_sessions: parsed.worker_sessions,
        usage,
        workspace_dir: dirs.workspace.clone(),
        memory_dir: dirs.memory.clone(),
    };

    // The harness prepends the global invariants; grade them and the scenario's own checks (the
    // latter borrowed — `Check` wraps a boxed closure and isn't `Clone`) and concatenate.
    let baseline = checks::baseline_checks();
    let mut graded = grade(&baseline, &transcript);
    graded.extend(grade(&scenario.checks, &transcript));
    let (frac, pass) = score(&graded);

    TrialReport {
        scenario: scenario.name.clone(),
        axis: scenario.axis.as_str().to_string(),
        backend: opts.backend.clone(),
        trial,
        pass,
        score: frac,
        stats: TokenStats::from_meter(&transcript.usage),
        checks: graded,
        judge: None,
        error: None,
        deliveries: transcript.deliveries,
        timeline: transcript.timeline,
        worker_sessions: transcript.worker_sessions,
        conversation: transcript.conversation,
        worker_prompts: transcript.worker_prompts,
    }
}

impl Axis {
    /// Parse an axis name (for `--axis`); `None` if unknown.
    pub fn parse(s: &str) -> Option<Axis> {
        [
            Axis::Delegation,
            Axis::Validation,
            Axis::ReplyDiscipline,
            Axis::Memory,
            Axis::Autonomy,
            Axis::Honesty,
        ]
        .into_iter()
        .find(|a| a.as_str() == s)
    }
}
