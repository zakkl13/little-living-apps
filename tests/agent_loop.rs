//! End-to-end tests driving the COMPILED `lila run` daemon against a fake Telegram server + the
//! scripted fake backend (no real subscription). Covers the serialized loop, authorization, the
//! reply path, command handling, and lossless restart.

mod common;

use std::process::Stdio;
use std::time::Duration;

use common::FakeTelegram;
use tokio::process::{Child, Command};

/// Spawn `lila run` with a hermetic env pointed at `tg`, using the scripted fake backend.
fn spawn_run(
    tg: &FakeTelegram,
    state_dir: &std::path::Path,
    memory_dir: &std::path::Path,
    reply: &str,
) -> Child {
    let bin = assert_cmd::cargo::cargo_bin("lila");
    let path = std::env::var("PATH").unwrap_or_default();
    Command::new(bin)
        .arg("run")
        .env_clear()
        // Forward the coverage profile path through env_clear() so the spawned (instrumented)
        // daemon writes its own .profraw; absent under plain `cargo test`, so the env stays
        // hermetic. The %p-%10m pattern keeps each subprocess's profile distinct.
        .envs(
            std::env::var("LLVM_PROFILE_FILE")
                .ok()
                .map(|v| ("LLVM_PROFILE_FILE", v)),
        )
        .env("PATH", path)
        .env("HOME", state_dir) // git init is happy with any HOME
        .env("TELEGRAM_BOT_TOKEN", "test-token")
        .env("ALLOWED_USER_IDS", "42")
        .env("TELEGRAM_API_BASE_URL", tg.base_url())
        .env("LILA_FAKE_BACKEND", "1")
        .env("LILA_FAKE_REPLY", reply)
        .env("MEMORY_DIR", memory_dir)
        .env("MANAGER_STATE_DIR", state_dir)
        .env("LOG_LEVEL", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn lila run")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_message_gets_a_reply() {
    let tmp = tempfile::tempdir().unwrap();
    let tg = FakeTelegram::start().await;
    tg.push_owner_message(42, 100, "hello there");

    let mut child = spawn_run(
        &tg,
        &tmp.path().join("state"),
        &tmp.path().join("memory"),
        "pong",
    );
    let got = tg.wait_for_sent("pong", Duration::from_secs(20)).await;
    common::terminate(&mut child).await;

    assert!(
        got,
        "manager should reply 'pong' to an owner message; got {:?}",
        tg.sent()
    );
    // The reply must go to the owner's chat (100), not anywhere else.
    assert!(
        tg.sent()
            .iter()
            .any(|(chat, t)| *chat == 100 && t.contains("pong"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_user_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let tg = FakeTelegram::start().await;
    tg.push_owner_message(999, 200, "let me in"); // 999 is not in ALLOWED_USER_IDS

    let mut child = spawn_run(
        &tg,
        &tmp.path().join("state"),
        &tmp.path().join("memory"),
        "pong",
    );
    let refused = tg
        .wait_for_sent("not authorized", Duration::from_secs(20))
        .await;
    common::terminate(&mut child).await;

    assert!(
        refused,
        "unauthorized sender must be refused; got {:?}",
        tg.sent()
    );
    assert!(
        !tg.sent().iter().any(|(_, t)| t.contains("pong")),
        "must not run a turn"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_command_reports_state() {
    let tmp = tempfile::tempdir().unwrap();
    let tg = FakeTelegram::start().await;
    tg.push_owner_message(42, 100, "/status");

    let mut child = spawn_run(
        &tg,
        &tmp.path().join("state"),
        &tmp.path().join("memory"),
        "pong",
    );
    let got = tg
        .wait_for_sent("Workers running", Duration::from_secs(20))
        .await;
    common::terminate(&mut child).await;

    assert!(got, "/status should report state; got {:?}", tg.sent());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn snapshot_persists_across_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    let memory = tmp.path().join("memory");
    let tg = FakeTelegram::start().await;

    // First run: one message, get a reply, then kill — a snapshot is written each turn.
    tg.push_owner_message(42, 100, "first");
    let mut child = spawn_run(&tg, &state, &memory, "reply-one");
    assert!(tg.wait_for_sent("reply-one", Duration::from_secs(20)).await);
    common::terminate(&mut child).await;

    // The snapshot file must exist and carry the captured fake session id.
    let snap = std::fs::read_to_string(state.join("snapshot.json")).expect("snapshot written");
    assert!(
        snap.contains("fake-session-1"),
        "snapshot should carry the session id: {snap}"
    );

    // Second run resumes cleanly and serves a new message.
    tg.push_owner_message(42, 100, "second");
    let mut child2 = spawn_run(&tg, &state, &memory, "reply-two");
    assert!(tg.wait_for_sent("reply-two", Duration::from_secs(20)).await);
    common::terminate(&mut child2).await;
}
