//! The read-only Inspector plane: stand it up over a populated in-process Telemetry + MemFs and
//! drive its HTTP API the way the browser shell does. Asserts the auth gate (token via header AND
//! `?t=`), the HTML shell, and that each JSON view reflects the live state — including the
//! manager-vs-worker token split that is the whole point of the enriched telemetry.

use std::sync::{Arc, Mutex};

use lila::inspector::{InspectorConfig, start};
use lila::memory::{MemFs, MemFsOptions};
use lila::runtime::telemetry::Telemetry;
use lila::runtime::{TokenUsage, TraceBlock};
use tokio::sync::Notify;

fn seed_memfs(dir: &std::path::Path) -> Arc<Mutex<MemFs>> {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("MEMORY.md"), "- ship the thing\n").unwrap();
    let mem = MemFs::open(MemFsOptions {
        dir: dir.to_path_buf(),
        fts_path: format!("{}.fts.sqlite", dir.display()),
    })
    .unwrap();
    Arc::new(Mutex::new(mem))
}

fn seed_telemetry() -> Arc<Mutex<Telemetry>> {
    let tel = Arc::new(Mutex::new(Telemetry::new()));
    let mut t = tel.lock().unwrap();
    t.begin_turn(1, "owner_message", "build a /health route".into(), 7);
    t.record_user_message("build a /health route".into());
    t.record_assistant_blocks(vec![TraceBlock::Text {
        text: "On it — delegating.".into(),
    }]);
    t.record_usage(
        1,
        TokenUsage {
            input_tokens: 100,
            output_tokens: 20,
            ..Default::default()
        },
    );
    t.record_worker_launch();
    t.record_worker_prompt(1, "w1".into(), "start", "add a GET /health route".into());
    t.record_worker_usage(TokenUsage {
        input_tokens: 500,
        output_tokens: 60,
        ..Default::default()
    });
    drop(t);
    tel
}

async fn json(client: &reqwest::Client, url: String) -> serde_json::Value {
    client
        .get(url)
        .header("x-inspector-token", "secret")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn inspector_serves_readonly_views() {
    let tmp = tempfile::tempdir().unwrap();
    let mem = seed_memfs(&tmp.path().join("memory"));
    let tel = seed_telemetry();
    let shutdown = Arc::new(Notify::new());

    let port = start(
        InspectorConfig {
            host: "127.0.0.1".into(),
            port: 0,
            token: Some("secret".into()),
            manager_model: "claude-opus-4-8".into(),
            backend: "claude".into(),
            workspace_dir: "/tmp/ws".into(),
            app_public_url: "https://example.test".into(),
            telemetry: tel.clone(),
            mem: mem.clone(),
        },
        shutdown.clone(),
    )
    .await
    .unwrap();

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Auth gate: no token → 401.
    let unauth = client
        .get(format!("{base}/api/overview"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401, "missing token must be rejected");

    // HTML shell (token via header).
    let html = client
        .get(format!("{base}/"))
        .header("x-inspector-token", "secret")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("lila Inspector"), "serves the SPA shell");

    // Overview reflects the live counts + the manager/worker token split.
    let o = json(&client, format!("{base}/api/overview")).await;
    assert_eq!(o["backend"], "claude");
    assert_eq!(o["counts"]["turns"], 1);
    assert_eq!(o["counts"]["workers"], 1);
    assert_eq!(o["usage"]["input_tokens"], 100);
    assert_eq!(o["usage"]["worker_input_tokens"], 500);
    assert!(
        o["app_public_url"]
            .as_str()
            .unwrap()
            .contains("example.test")
    );

    // Workers view — token via the `?t=` query string this time.
    let w: serde_json::Value = client
        .get(format!("{base}/api/workers?t=secret"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(w["workers"][0]["id"], "w1");
    assert!(
        w["workers"][0]["prompts"][0]["prompt"]
            .as_str()
            .unwrap()
            .contains("/health")
    );

    // Conversation reconstructed (user + assistant).
    let c = json(&client, format!("{base}/api/conversation")).await;
    assert_eq!(c["message_count"], 2);
    assert_eq!(c["messages"][0]["role"], "user");

    // Memories read from disk.
    let m = json(&client, format!("{base}/api/memories")).await;
    let paths: Vec<&str> = m["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.contains(&"MEMORY.md"),
        "lists the memory file: {paths:?}"
    );

    shutdown.notify_waiters();
}
