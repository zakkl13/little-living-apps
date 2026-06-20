//! Hermetic self-test of the eval harness: drive a full trial against the COMPILED binary on the
//! scripted FAKE backend (no subscription), and assert the whole pipeline works end to end —
//! spawn → owner turn → trace → drain via the idle marker → graded transcript with token stats.
//! This exercises everything the live eval does except real model behavior.

use lila::eval::harness::{HarnessOptions, run_trial};
use lila::eval::transcript::Axis;

fn fake_opts() -> HarnessOptions {
    HarnessOptions {
        backend: "codex".into(),
        sandbox: "workspace-write".into(),
        timeout_secs: 30,
        fake: true,
        keep_tmp: false,
    }
}

/// A throwaway one-turn scenario whose checks the fake backend (reply "ack") can satisfy: a
/// well-formed, shop-talk-free delivery. The fake records a manager-usage trace + snapshot, so the
/// token stats must be populated.
fn ack_scenario() -> lila::eval::scenarios::Scenario {
    // Build via the public suite, then pick a memory scenario that drives no workers and just acks.
    // remember-fact is ideal: no workers, terse reply — but it grades memory writes the fake can't do.
    // So we use a hand-built scenario through the suite's selection isn't possible; instead drive the
    // simplest real scenario and assert only the harness mechanics (pass flag may be false).
    lila::eval::scenarios::scenarios()
        .into_iter()
        .find(|s| s.name == "remember-fact")
        .expect("remember-fact scenario exists")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn harness_drives_a_full_trial_on_the_fake_backend() {
    let scenario = ack_scenario();
    assert_eq!(scenario.axis, Axis::Memory);

    let report = run_trial(&scenario, 1, &fake_opts())
        .await
        .expect("trial runs against the fake backend");

    // The harness completed the cascade (no timeout error) and produced a transcript.
    assert!(
        report.error.is_none(),
        "unexpected harness error: {:?}",
        report.error
    );
    assert_eq!(report.scenario, "remember-fact");
    assert_eq!(report.backend, "codex");

    // The fake backend replied (ack), so a delivery was captured and the conversation/timeline exist.
    assert!(
        !report.deliveries.is_empty(),
        "expected at least one captured delivery"
    );

    // Token stats flowed from the binary's snapshot: the fake manager turn reported usage.
    assert!(report.stats.manager_turns >= 1, "expected ≥1 manager turn");
    assert!(
        report.stats.manager_tokens > 0,
        "expected manager tokens from the fake usage record"
    );

    // The baseline invariants were graded (well-formed deliveries + no shop talk are always present).
    assert!(
        report.checks.iter().any(|c| c.name.contains("well-formed")),
        "baseline checks must be graded: {:?}",
        report.checks.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
}
