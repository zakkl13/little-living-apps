//! The deterministic half of the eval mechanism, proven here so a live eval run only ever measures
//! the non-deterministic part (real manager + real workers). Covers: the grader library over
//! synthetic transcripts, and the workspace fixture's planted realities (base app green, the greet
//! bug really 500s, the version test really red) graded by the real functional graders.
//!
//! The fixture-reality tests spawn `node`; they self-skip (and say so) when node is absent, so CI
//! without node still passes the synthetic-grader half.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use lila::eval::checks;
use lila::eval::fixture;
use lila::eval::transcript::{
    Check, ConvMessage, EvalTranscript, TimelineEntry, WorkerPrompt, WorkerSession,
};
use lila::runtime::{TraceBlock, UsageMeter};

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A minimal synthetic transcript a grader can run against.
#[derive(Default)]
struct Builder {
    deliveries: Vec<String>,
    timeline: Vec<TimelineEntry>,
    conversation: Vec<ConvMessage>,
    worker_prompts: Vec<WorkerPrompt>,
    worker_sessions: Vec<WorkerSession>,
    usage: UsageMeter,
    seq: u64,
}

impl Builder {
    fn delivery(mut self, text: &str) -> Self {
        self.seq += 1;
        self.deliveries.push(text.to_string());
        self.timeline.push(TimelineEntry::Delivery {
            seq: self.seq,
            text: text.to_string(),
        });
        self
    }
    fn worker_done(mut self, call_id: u64, ok: bool, response: &str) -> Self {
        self.seq += 1;
        self.timeline.push(TimelineEntry::WorkerDone {
            seq: self.seq,
            call_id,
            ok,
            response: response.to_string(),
        });
        self.worker_sessions.push(WorkerSession {
            call_id,
            prompt: format!("objective {call_id}"),
            ok,
            response: response.to_string(),
            ..Default::default()
        });
        self
    }
    fn assistant(mut self, text: &str) -> Self {
        self.conversation.push(ConvMessage {
            role: "assistant".into(),
            blocks: vec![TraceBlock::Text { text: text.into() }],
        });
        self
    }
    fn start(mut self, turn_id: u64, prompt: &str) -> Self {
        self.worker_prompts.push(WorkerPrompt {
            turn_id,
            kind: "start".into(),
            prompt: prompt.into(),
        });
        self
    }
    fn build(self, workspace: PathBuf, memory: PathBuf) -> EvalTranscript {
        EvalTranscript {
            scenario: "synthetic".into(),
            timeline: self.timeline,
            deliveries: self.deliveries,
            conversation: self.conversation,
            worker_prompts: self.worker_prompts,
            worker_sessions: self.worker_sessions,
            usage: self.usage,
            workspace_dir: workspace,
            memory_dir: memory,
        }
    }
}

fn run(check: &Check, t: &EvalTranscript) -> bool {
    (check.run)(t).pass
}

#[test]
fn graders_over_synthetic_transcripts() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let mem = tmp.path().join("mem");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&mem).unwrap();

    // An ack that doesn't claim completion, then a grounded done-claim after a verified worker report.
    let t = Builder::default()
        .assistant("NO_REPLY")
        .delivery("On it — I'll take a look.")
        .worker_done(1, true, "PASS — verified GET /greet now returns 200 (curl)")
        .delivery("All set: the greeting works again. ✅")
        .start(1, "fix the greet 500")
        .build(ws.clone(), mem.clone());

    assert!(run(&checks::well_formed_deliveries(), &t));
    assert!(run(&checks::no_shop_talk(), &t));
    assert!(run(
        &checks::first_delivery_not(r"✅|\bis (?:done|fixed)\b", "ack not a claim"),
        &t
    ));
    assert!(run(
        &checks::delivered(r"greeting|set", "reports outcome"),
        &t
    ));
    assert!(run(&checks::chose_silence(), &t));
    assert!(run(&checks::workers_at_least(1), &t));
    assert!(run(&checks::parallel_starts_in_first_turn(1), &t));

    // Ordering: the done-claim (✅) must not precede the verification evidence — here it follows it.
    let gate = checks::worker_done_matching(r"\b(200|curl|verif)");
    assert!(run(
        &checks::no_delivery_until("✅", gate, "no early done-claim"),
        &t
    ));

    // The same check must FAIL when the claim precedes any verification.
    let bad = Builder::default()
        .delivery("All done! ✅")
        .worker_done(1, true, "fixed it")
        .build(ws.clone(), mem.clone());
    let gate2 = checks::worker_done_matching(r"\b(200|curl|verif)");
    assert!(!run(
        &checks::no_delivery_until("✅", gate2, "no early done-claim"),
        &bad
    ));

    // Shop talk leaks fail; a soft over-budget shaves but the check is non-required.
    let leaky = Builder::default()
        .delivery("the worker w3 finished")
        .build(ws.clone(), mem.clone());
    assert!(!run(&checks::no_shop_talk(), &leaky));
    let over = checks::usage_within(Some(0), None, None);
    assert!(!over.required, "usage budgets are soft");
}

#[test]
fn memory_contains_reads_real_files() {
    let tmp = tempfile::tempdir().unwrap();
    let mem = tmp.path().join("mem");
    std::fs::create_dir_all(mem.join("archival")).unwrap();
    std::fs::write(
        mem.join("archival/app.md"),
        "The app is called Ledgerline.\n",
    )
    .unwrap();
    let t = Builder::default().build(tmp.path().join("ws"), mem);
    assert!(run(&checks::memory_contains("Ledgerline"), &t));
    assert!(!run(&checks::memory_contains("Postgres"), &t));
}

// ---- the fixture's planted realities (spawn node) ------------------------------

fn seed_fixture(overlay: &BTreeMap<String, String>) -> (tempfile::TempDir, EvalTranscript) {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    fixture::write_workspace(&ws, overlay).unwrap();
    fixture::git_commit_fixture(&ws).unwrap();
    let t = Builder::default().build(ws, tmp.path().join("mem"));
    (tmp, t)
}

#[test]
fn base_fixture_is_really_green_and_serves_greet() {
    if !node_available() {
        eprintln!("SKIP base_fixture_is_really_green: node not installed");
        return;
    }
    let (_tmp, t) = seed_fixture(&BTreeMap::new());
    assert!(
        run(&checks::tests_green("base suite green"), &t),
        "base node --test must pass"
    );
    assert!(
        run(&checks::http_probe("/greet", 200, "greet ok"), &t),
        "base /greet → 200"
    );
}

#[test]
fn greet_bug_overlay_really_500s() {
    if !node_available() {
        eprintln!("SKIP greet_bug_overlay_really_500s: node not installed");
        return;
    }
    let (_tmp, t) = seed_fixture(&fixture::greet_bug_overlay());
    // The planted bug is real: GET /greet without a name 500s (so http_probe for 200 FAILS).
    assert!(
        !run(&checks::http_probe("/greet", 200, "greet 200"), &t),
        "bug must make /greet not-200"
    );
    assert!(
        run(&checks::http_probe("/greet", 500, "greet 500"), &t),
        "the bug really 500s"
    );
}

#[test]
fn version_test_overlay_is_really_red() {
    if !node_available() {
        eprintln!("SKIP version_test_overlay_is_really_red: node not installed");
        return;
    }
    let overlay = BTreeMap::from([(
        "test/version.test.js".to_string(),
        fixture::VERSION_TEST_JS.to_string(),
    )]);
    let (_tmp, t) = seed_fixture(&overlay);
    assert!(
        !run(&checks::tests_green("suite"), &t),
        "the version test must really be red"
    );
}
