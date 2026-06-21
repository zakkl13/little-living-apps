//! Integration tests driving the COMPILED `lila` binary through its CLI. This is the primary test
//! surface (per the project's integration-test obsession).

use assert_cmd::Command;
use predicates::prelude::*;

/// A `lila` command with a clean env: no inherited vars, so tests are hermetic and the billing guard
/// can't be tripped by the developer's shell.
fn lila() -> Command {
    let mut cmd = Command::cargo_bin("lila").expect("binary builds");
    cmd.env_clear();
    // Forward the coverage profile path through env_clear() so the spawned (instrumented) binary
    // writes its own .profraw; absent under plain `cargo test`, so the env stays hermetic.
    if let Ok(v) = std::env::var("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", v);
    }
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

// --- `lila doctor` -----------------------------------------------------------------------------

#[test]
fn doctor_fails_without_config() {
    lila()
        .arg("doctor")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG ERROR"));
}

#[test]
fn doctor_reports_missing_codex_cli() {
    // env_clear() means there's no PATH, so the dependency-free `which` finds nothing → exit 1.
    lila()
        .arg("doctor")
        .env("TELEGRAM_BOT_TOKEN", "tok")
        .env("ALLOWED_USER_IDS", "42")
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("config:   OK (codex backend)"))
        .stderr(predicate::str::contains("codex CLI NOT found"));
}

#[test]
fn doctor_finds_codex_via_bin_override() {
    // CODEX_BIN points at an existing file (the lila binary itself), so the override branch resolves.
    let existing = assert_cmd::cargo::cargo_bin("lila");
    lila()
        .arg("doctor")
        .env("TELEGRAM_BOT_TOKEN", "tok")
        .env("ALLOWED_USER_IDS", "42")
        .env("CODEX_BIN", existing)
        .assert()
        .success()
        .stdout(predicate::str::contains("codex CLI found"));
}

#[test]
fn doctor_checks_the_claude_backend() {
    lila()
        .arg("doctor")
        .env("TELEGRAM_BOT_TOKEN", "tok")
        .env("ALLOWED_USER_IDS", "42")
        .env("AGENT_BACKEND", "claude")
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("config:   OK (claude backend)"))
        .stderr(predicate::str::contains("claude CLI NOT found"));
}

// --- `lila memory view|search` -----------------------------------------------------------------

/// A `lila memory ...` command with a valid config pointed at a throwaway memory store. The
/// `TempDir` is returned alongside so the caller keeps it alive for the duration of the run.
fn lila_memory(memdir: &std::path::Path) -> Command {
    let mut cmd = lila();
    cmd.env("TELEGRAM_BOT_TOKEN", "tok")
        .env("ALLOWED_USER_IDS", "42")
        .env("HOME", memdir) // git init (memory repo) is happy with any HOME
        .env("MEMORY_DIR", memdir.join("memories"));
    cmd
}

#[test]
fn memory_view_lists_the_scaffolded_mount() {
    let tmp = tempfile::tempdir().unwrap();
    lila_memory(tmp.path())
        .args(["memory", "view", "/memories"])
        .assert()
        .success();
}

#[test]
fn memory_view_rejects_a_path_outside_the_mount() {
    let tmp = tempfile::tempdir().unwrap();
    lila_memory(tmp.path())
        .args(["memory", "view", "/etc/passwd"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("error"));
}

#[test]
fn memory_search_reports_no_matches() {
    let tmp = tempfile::tempdir().unwrap();
    lila_memory(tmp.path())
        .args(["memory", "search", "zzqqxxnomatch"])
        .assert()
        .success()
        .stdout(predicate::str::contains("(no matches)"));
}
