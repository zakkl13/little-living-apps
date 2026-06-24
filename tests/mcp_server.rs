//! Integration test for the Lila MCP server: connect a real rmcp client over streamable-HTTP,
//! exercise the memory tools + subagent_start, and confirm bearer auth rejects unauthenticated
//! clients. (rmcp server ↔ rmcp client both implement the MCP spec the codex/claude clients follow.)

use std::sync::{Arc, Mutex};

use lila::manager::mcp;
use lila::memory::{MemFs, MemFsOptions};
use lila::runtime::ManagerEvent;
use lila::runtime::telemetry::Telemetry;
use lila::workers::Orchestrator;
use lila::workers::fake_runner::FakeRunner;

use rmcp::model::{CallToolRequestParams, CallToolResult, ClientInfo};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceExt};

fn tool_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn args(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    value.as_object().cloned().unwrap_or_default()
}

async fn start_server(
    token: &str,
) -> (
    mcp::RunningMcp,
    tokio::sync::mpsc::UnboundedReceiver<ManagerEvent>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.keep(); // outlive the test; cleaned by the OS
    let mem = Arc::new(Mutex::new(
        MemFs::open(MemFsOptions {
            dir: dir.join("memory"),
            fts_path: ":memory:".into(),
        })
        .unwrap(),
    ));
    let telemetry = Arc::new(Mutex::new(Telemetry::new()));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let orch = Arc::new(Orchestrator::new(
        Arc::new(FakeRunner::default()),
        dir.join("ws"),
        tx,
        telemetry,
        None,
    ));
    let server = mcp::start(mem, orch, dir.join("ws"), token.to_string(), 0)
        .await
        .unwrap();
    (server, rx)
}

async fn connect(
    url: &str,
    token: Option<&str>,
) -> anyhow::Result<RunningService<RoleClient, ClientInfo>> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(url.to_string());
    if let Some(t) = token {
        config = config.auth_header(t.to_string());
    }
    let transport = StreamableHttpClientTransport::with_client(reqwest::Client::new(), config);
    Ok(ClientInfo::default().serve(transport).await?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_tools_round_trip_over_mcp() {
    let (server, _rx) = start_server("secret-token").await;
    let client = connect(&server.url, Some("secret-token"))
        .await
        .expect("connect");

    let tools = client.list_tools(None).await.expect("list_tools");
    let names: Vec<&str> = tools.tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(names.contains(&"memory_create"), "tools: {names:?}");
    assert!(names.contains(&"subagent_start"), "tools: {names:?}");
    assert!(names.contains(&"settings_get"), "tools: {names:?}");
    assert!(names.contains(&"settings_set"), "tools: {names:?}");

    let created = client
        .call_tool(
            CallToolRequestParams::new("memory_create").with_arguments(args(serde_json::json!({
                "path": "/memories/archival/fact.md",
                "file_text": "the moon orbits earth"
            }))),
        )
        .await
        .expect("create");
    assert!(tool_text(&created).contains("Created"));

    let found = client
        .call_tool(
            CallToolRequestParams::new("memory_search")
                .with_arguments(args(serde_json::json!({ "query": "moon" }))),
        )
        .await
        .expect("search");
    assert!(
        tool_text(&found).contains("fact.md"),
        "search result: {}",
        tool_text(&found)
    );

    let _ = client.cancel().await;
    server.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_start_dispatches_a_worker() {
    let (server, mut rx) = start_server("secret-token").await;
    let client = connect(&server.url, Some("secret-token"))
        .await
        .expect("connect");

    let started = client
        .call_tool(
            CallToolRequestParams::new("subagent_start")
                .with_arguments(args(serde_json::json!({ "objective": "build the thing" }))),
        )
        .await
        .expect("subagent_start");
    assert!(tool_text(&started).contains("started"));

    let event = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("worker event within timeout")
        .expect("event");
    assert!(matches!(event, ManagerEvent::WorkerEvent { .. }));

    let _ = client.cancel().await;
    server.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthenticated_client_is_rejected() {
    let (server, _rx) = start_server("secret-token").await;
    let result = connect(&server.url, None).await;
    assert!(
        result.is_err(),
        "connecting without a bearer token must fail"
    );
    server.close();
}
