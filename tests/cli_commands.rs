//! Integration tests driving the COMPILED `lila` binary through its CLI. This is the primary test
//! surface (per the project's integration-test obsession).

use assert_cmd::Command;
use predicates::prelude::*;

/// A `lila` command with a clean env: no inherited vars, so tests are hermetic and the billing guard
/// can't be tripped by the developer's shell.
fn lila() -> Command {
    let mut cmd = Command::cargo_bin("lila").expect("binary builds");
    cmd.env_clear();
    cmd
}

#[test]
fn config_check_fails_without_required_env() {
    lila()
        .arg("config-check")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("TELEGRAM_BOT_TOKEN"));
}

#[test]
fn config_check_passes_with_valid_env() {
    lila()
        .arg("config-check")
        .env("TELEGRAM_BOT_TOKEN", "tok")
        .env("ALLOWED_USER_IDS", "42")
        .assert()
        .success()
        .stdout(predicate::str::contains("configuration valid"))
        .stdout(predicate::str::contains("backend:    codex"));
}

#[test]
fn config_check_refuses_billing_flip_key() {
    lila()
        .arg("config-check")
        .env("TELEGRAM_BOT_TOKEN", "tok")
        .env("ALLOWED_USER_IDS", "42")
        .env("OPENAI_API_KEY", "sk-xxx")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("metered API billing"));
}

#[test]
fn config_check_claude_backend_refuses_anthropic_key() {
    lila()
        .arg("config-check")
        .env("TELEGRAM_BOT_TOKEN", "tok")
        .env("ALLOWED_USER_IDS", "42")
        .env("AGENT_BACKEND", "claude")
        .env("ANTHROPIC_API_KEY", "sk-ant")
        .assert()
        .failure()
        .stderr(predicate::str::contains("metered API billing"));
}

#[test]
fn status_reports_no_snapshot() {
    lila()
        .arg("status")
        .env("TELEGRAM_BOT_TOKEN", "tok")
        .env("ALLOWED_USER_IDS", "42")
        .env("MANAGER_STATE_DIR", "/tmp/lila-nonexistent-xyz")
        .assert()
        .success()
        .stdout(predicate::str::contains("(none yet)"));
}
