//! LIVE smoke test of the real Codex backend (real model call + Lila MCP attach + serialized loop),
//! driven through the compiled binary against the fake Telegram server. Ignored by default — it
//! needs a Codex subscription login in CODEX_HOME and consumes tokens. Run explicitly:
//!
//!   cargo test --test live_codex -- --ignored --nocapture

mod common;

use std::process::Stdio;
use std::time::Duration;

use common::FakeTelegram;
use tokio::process::Command;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: needs Codex subscription auth + network"]
async fn real_codex_manager_replies() {
    let tmp = tempfile::tempdir().unwrap();
    let tg = FakeTelegram::start().await;
    tg.push_owner_message(42, 100, "Please reply to me with exactly: SMOKE-OK");

    let bin = assert_cmd::cargo::cargo_bin("lila");
    let mut child = Command::new(bin)
        .arg("run")
        // Inherit the dev env (so codex finds its auth in CODEX_HOME/~/.codex) but strip billing keys.
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env("AGENT_BACKEND", "codex")
        .env("MANAGER_REASONING_EFFORT", "low") // fast + cheap for a smoke
        .env("TELEGRAM_BOT_TOKEN", "test-token")
        .env("ALLOWED_USER_IDS", "42")
        .env("TELEGRAM_API_BASE_URL", tg.base_url())
        .env("MEMORY_DIR", tmp.path().join("memory"))
        .env("MANAGER_STATE_DIR", tmp.path().join("state"))
        .env("MANAGER_DIR", tmp.path().join("manager"))
        .env("WORKSPACE_DIR", tmp.path().join("workspace"))
        .env("LOG_LEVEL", "info")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn lila run");

    let got = tg.wait_for_sent("SMOKE-OK", Duration::from_secs(180)).await;
    let _ = child.kill().await;
    assert!(
        got,
        "real Codex manager should reply 'SMOKE-OK'; sent: {:?}",
        tg.sent()
    );
}
