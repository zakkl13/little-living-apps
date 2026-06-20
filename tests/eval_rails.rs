//! Proves the Rails fixture's planted realities with the REAL Rails graders, so a live eval run only
//! measures the agent: the base app boots + tests green + serves /greet, the greet-bug overlay really
//! breaks /greet, and the version-test overlay is really red. Self-skips when Ruby or the built
//! template is absent (mirrors the Node graders' node-absent skip). One sequential test fn — it boots
//! Rails several times, so it does not parallelize.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use lila::eval::checks::{rails_http_probe, rails_tests_green};
use lila::eval::fixture::{
    RAILS_VERSION_TEST, git_commit_fixture, rails_greet_bug_overlay, rails_template_dir, seed_rails,
};
use lila::eval::transcript::{Check, EvalTranscript};
use lila::runtime::UsageMeter;

fn ruby_available() -> bool {
    Command::new("ruby")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn transcript_for(workspace: PathBuf) -> EvalTranscript {
    EvalTranscript {
        scenario: "rails-fixture".into(),
        timeline: vec![],
        deliveries: vec![],
        conversation: vec![],
        worker_prompts: vec![],
        worker_sessions: vec![],
        usage: UsageMeter::default(),
        workspace_dir: workspace,
        memory_dir: std::env::temp_dir(),
    }
}

fn seed(overlay: &BTreeMap<String, String>) -> (tempfile::TempDir, EvalTranscript) {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("workspace");
    seed_rails(&ws, overlay).unwrap();
    git_commit_fixture(&ws).unwrap();
    let t = transcript_for(ws);
    (tmp, t)
}

fn pass(c: &Check, t: &EvalTranscript) -> bool {
    (c.run)(t).pass
}

#[test]
fn rails_fixture_planted_realities() {
    if !ruby_available() || !rails_template_dir().join("bin/rails").exists() {
        eprintln!("SKIP rails_fixture_planted_realities: ruby or built template absent");
        return;
    }

    // Base app: suite green and GET /greet → 200.
    let (_b, base) = seed(&BTreeMap::new());
    assert!(
        pass(&rails_tests_green("base green"), &base),
        "base bin/rails test must pass"
    );
    assert!(
        pass(&rails_http_probe("/greet", 200, "greet ok"), &base),
        "base /greet → 200"
    );

    // Greet-bug overlay: GET /greet without a name no longer 200 (it 500s on nil.strip).
    let (_g, bug) = seed(&rails_greet_bug_overlay());
    assert!(
        !pass(&rails_http_probe("/greet", 200, "greet 200"), &bug),
        "the planted bug must make /greet not-200"
    );

    // Version-test overlay: the suite is really red (route doesn't exist).
    let overlay = BTreeMap::from([(
        "test/controllers/version_test.rb".to_string(),
        RAILS_VERSION_TEST.to_string(),
    )]);
    let (_v, red) = seed(&overlay);
    assert!(
        !pass(&rails_tests_green("suite"), &red),
        "the version test must really be red"
    );
}
