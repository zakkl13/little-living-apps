//! Validates the eval/inspector trace (`LILA_EVAL_TRACE`) end-to-end against the COMPILED `lila run`
//! binary: an owner turn must produce a well-formed JSONL trace carrying the timeline (owner_msg,
//! delivery), the manager conversation (manager_msg), per-turn manager usage, and an `idle`
//! quiescence marker. This is the foundation the eval harness reconstructs each trial from.

mod common;

use std::process::Stdio;
use std::time::Duration;

use common::FakeTelegram;
use serde_json::Value;
use tokio::process::{Child, Command};

fn spawn_run_with_trace(
    tg: &FakeTelegram,
    state_dir: &std::path::Path,
    memory_dir: &std::path::Path,
    trace_path: &std::path::Path,
    reply: &str,
) -> Child {
    let bin = assert_cmd::cargo::cargo_bin("lila");
    let path = std::env::var("PATH").unwrap_or_default();
    Command::new(bin)
        .arg("run")
        .env_clear()
        .env("PATH", path)
        .env("HOME", state_dir)
        .env("TELEGRAM_BOT_TOKEN", "test-token")
        .env("ALLOWED_USER_IDS", "42")
        .env("TELEGRAM_API_BASE_URL", tg.base_url())
        .env("LILA_FAKE_BACKEND", "1")
        .env("LILA_FAKE_REPLY", reply)
        .env("LILA_EVAL_TRACE", trace_path)
        .env("MEMORY_DIR", memory_dir)
        .env("MANAGER_STATE_DIR", state_dir)
        .env("LOG_LEVEL", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn lila run")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trace_records_timeline_conversation_and_usage() {
    let tmp = tempfile::tempdir().unwrap();
    let trace_path = tmp.path().join("trace.jsonl");
    let tg = FakeTelegram::start().await;
    tg.push_owner_message(42, 100, "build me a thing");

    let mut child = spawn_run_with_trace(
        &tg,
        &tmp.path().join("state"),
        &tmp.path().join("memory"),
        &trace_path,
        "on it",
    );
    let got = tg.wait_for_sent("on it", Duration::from_secs(20)).await;
    assert!(got, "manager should reply; got {:?}", tg.sent());
    // Give the loop a beat to emit the post-turn idle marker, then stop the binary.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = child.kill().await;

    let body = std::fs::read_to_string(&trace_path).expect("trace file written");
    let records: Vec<Value> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("each line is valid JSON"))
        .collect();
    let kinds: Vec<&str> = records.iter().filter_map(|r| r["type"].as_str()).collect();

    assert!(
        kinds.contains(&"owner_msg"),
        "missing owner_msg in {kinds:?}"
    );
    assert!(
        kinds.contains(&"manager_msg"),
        "missing manager_msg in {kinds:?}"
    );
    assert!(kinds.contains(&"delivery"), "missing delivery in {kinds:?}");
    assert!(kinds.contains(&"usage"), "missing usage in {kinds:?}");
    assert!(kinds.contains(&"idle"), "missing idle marker in {kinds:?}");

    // The owner message text round-trips, and usage is tagged manager-tier.
    let owner = records.iter().find(|r| r["type"] == "owner_msg").unwrap();
    assert_eq!(owner["text"], "build me a thing");
    let usage = records.iter().find(|r| r["type"] == "usage").unwrap();
    assert_eq!(usage["tier"], "manager");
    // The delivered reply is captured at the model level too (the choseSilence/judge view).
    let delivery = records.iter().find(|r| r["type"] == "delivery").unwrap();
    assert_eq!(delivery["text"], "on it");
}
