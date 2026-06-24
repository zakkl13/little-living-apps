//! The Lila MCP server: a loopback streamable-HTTP MCP server exposing the manager's memory +
//! orchestration tools, on `rmcp`. The manager
//! thread reaches it via `mcp_servers.lila.url` + a per-boot bearer token. There is deliberately no
//! shell/file/net tool — the manager's "no hands" boundary is exactly this tool list.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header::AUTHORIZATION};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::manager::settings;
use crate::memory::{MemFs, MemoryCommand};
use crate::workers::Orchestrator;

/// The shared state the Lila tools operate on.
#[derive(Clone)]
pub struct LilaServer {
    mem: Arc<Mutex<MemFs>>,
    orch: Arc<Orchestrator>,
    /// The app workspace — where `settings_*` read/write structured state (e.g. `design.lock`).
    workspace_dir: PathBuf,
    tool_router: ToolRouter<Self>,
}

// ---- tool request shapes (JSON Schema is derived for the MCP tool definitions) ----

#[derive(Debug, Deserialize, JsonSchema)]
struct ViewReq {
    /// A `/memories/...` path.
    path: String,
    /// Optional 1-based inclusive `[start, end]` line range (`end = -1` for end-of-file).
    view_range: Option<[i64; 2]>,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct CreateReq {
    path: String,
    file_text: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct ReplaceReq {
    path: String,
    old_str: String,
    new_str: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct InsertReq {
    path: String,
    insert_line: usize,
    insert_text: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct PathReq {
    path: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct RenameReq {
    old_path: String,
    new_path: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct SearchReq {
    query: String,
    limit: Option<usize>,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct SubagentReq {
    objective: String,
    /// Project dir under the workspace (optional).
    project: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct SettingsGetReq {
    /// A single setting to read (e.g. `design`). Omit to list every setting.
    key: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct SettingsSetReq {
    /// The setting to change (e.g. `design`).
    key: String,
    /// The new value (for `design`, a brand from the browsable pool).
    value: String,
}

#[tool_router(router = tool_router)]
impl LilaServer {
    fn run_memory(&self, cmd: MemoryCommand) -> String {
        // Recover from a poisoned lock (a prior panic) rather than crashing the tool call.
        let mem = self.mem.lock().unwrap_or_else(|e| e.into_inner());
        mem.execute(&cmd).unwrap_or_else(|e| format!("error: {e}"))
    }

    /// Read a memory file, or list a directory. Paths are under /memories.
    #[tool(
        description = "Read a memory file, or list a directory. Paths are under /memories (e.g. /memories/archival/decisions/stack.md)."
    )]
    async fn memory_view(&self, p: Parameters<ViewReq>) -> String {
        self.run_memory(MemoryCommand::View {
            path: p.0.path,
            view_range: p.0.view_range,
        })
    }

    /// Create or overwrite a memory file with the given text.
    #[tool(description = "Create or overwrite a memory file with the given text.")]
    async fn memory_create(&self, p: Parameters<CreateReq>) -> String {
        self.run_memory(MemoryCommand::Create {
            path: p.0.path,
            file_text: p.0.file_text,
        })
    }

    /// Replace a unique substring in a memory file (add context if it isn't unique).
    #[tool(
        description = "Replace a unique substring in a memory file (add context if it isn't unique)."
    )]
    async fn memory_str_replace(&self, p: Parameters<ReplaceReq>) -> String {
        self.run_memory(MemoryCommand::StrReplace {
            path: p.0.path,
            old_str: p.0.old_str,
            new_str: p.0.new_str,
        })
    }

    /// Insert a line of text into a memory file at the given 0-based line index.
    #[tool(
        description = "Insert a line of text into a memory file at the given 0-based line index."
    )]
    async fn memory_insert(&self, p: Parameters<InsertReq>) -> String {
        self.run_memory(MemoryCommand::Insert {
            path: p.0.path,
            insert_line: p.0.insert_line,
            insert_text: p.0.insert_text,
        })
    }

    /// Delete a memory file or directory.
    #[tool(description = "Delete a memory file or directory.")]
    async fn memory_delete(&self, p: Parameters<PathReq>) -> String {
        self.run_memory(MemoryCommand::Delete { path: p.0.path })
    }

    /// Rename or move a memory file or directory.
    #[tool(description = "Rename or move a memory file or directory.")]
    async fn memory_rename(&self, p: Parameters<RenameReq>) -> String {
        self.run_memory(MemoryCommand::Rename {
            old_path: p.0.old_path,
            new_path: p.0.new_path,
        })
    }

    /// Full-text search across ALL memory files. Returns paths + snippets.
    #[tool(
        description = "Full-text search across ALL memory files (system, archival, recall). Returns paths + snippets; use memory_view to read a hit in full."
    )]
    async fn memory_search(&self, p: Parameters<SearchReq>) -> String {
        let mem = self.mem.lock().unwrap_or_else(|e| e.into_inner());
        format_hits(mem.search(&p.0.query, p.0.limit.unwrap_or(10)))
    }

    /// Full-text search restricted to recall/ (summarized past conversations).
    #[tool(description = "Full-text search restricted to recall/ (summarized past conversations).")]
    async fn recall_search(&self, p: Parameters<SearchReq>) -> String {
        let mem = self.mem.lock().unwrap_or_else(|e| e.into_inner());
        format_hits(mem.recall_search(&p.0.query, p.0.limit.unwrap_or(10)))
    }

    /// Spawn a single-use worker on an objective. It reports back once as an event and is gone.
    #[tool(
        description = "Spawn a single-use worker on an objective. It runs in the background, reports back once as an event, and is then gone — there is no follow-up channel, so put everything it needs (context, scope, how to verify) in the objective. It starts cold, with only the workspace, the git history, and what you wrote."
    )]
    async fn subagent_start(&self, p: Parameters<SubagentReq>) -> String {
        let id = self.orch.start(p.0.objective, p.0.project);
        format!("subagent {id} started — it will report back once when it finishes")
    }

    /// Read the app's structured settings.
    #[tool(
        description = "Read the app's structured settings (the typed counterpart to memories). Omit `key` to list all; pass e.g. key=\"design\" for one. `design` is the app's locked look — reading it shows the current system and the browsable pool you can switch to."
    )]
    async fn settings_get(&self, p: Parameters<SettingsGetReq>) -> String {
        settings::get(p.0.key.as_deref(), &self.workspace_dir)
    }

    /// Change a writable setting.
    #[tool(
        description = "Change a writable setting. settings_set key=\"design\" value=\"<brand>\" switches the app's locked look to a browsable-pool system: it stages the new system (refreshes .lila/ and re-locks design.lock as the owner's choice) in-process. The new look only ships once you hand a worker the stack-fit it returns. Read settings_get first to see valid brands."
    )]
    async fn settings_set(&self, p: Parameters<SettingsSetReq>) -> String {
        match settings::set(&p.0.key, &p.0.value, &self.workspace_dir) {
            Ok(summary) => summary,
            Err(e) => format!("error: {e}"),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for LilaServer {
    // `ServerInfo` is `#[non_exhaustive]`, so we must build from Default and assign fields.
    #[allow(clippy::field_reassign_with_default)]
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.instructions = Some(
            "The manager's only hands: memory_* tools, settings_get/settings_set, and subagent_start. No shell/file/net."
                .to_string(),
        );
        info
    }
}

fn format_hits(
    result: Result<Vec<crate::memory::SearchHit>, crate::memory::MemoryError>,
) -> String {
    match result {
        Ok(hits) if hits.is_empty() => "(no matches)".to_string(),
        Ok(hits) => hits
            .iter()
            .map(|h| format!("{}\n    {}", h.path, h.snippet))
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => format!("error: {e}"),
    }
}

/// A running Lila MCP server.
pub struct RunningMcp {
    /// The URL the backend's `mcp_servers.lila.url` points at.
    pub url: String,
    /// The bearer token required on every request.
    pub token: String,
    /// Bound port (meaningful when port 0 was requested).
    pub port: u16,
    cancel: CancellationToken,
}

impl RunningMcp {
    /// Shut the server down.
    pub fn close(&self) {
        self.cancel.cancel();
    }
}

/// Start the Lila MCP server on `127.0.0.1:port` (0 = a free port), bearer-guarded.
pub async fn start(
    mem: Arc<Mutex<MemFs>>,
    orch: Arc<Orchestrator>,
    workspace_dir: PathBuf,
    token: String,
    port: u16,
) -> anyhow::Result<RunningMcp> {
    let cancel = CancellationToken::new();
    let factory_mem = mem.clone();
    let factory_orch = orch.clone();
    let factory_ws = workspace_dir.clone();
    let service = StreamableHttpService::new(
        move || {
            Ok(LilaServer {
                mem: factory_mem.clone(),
                orch: factory_orch.clone(),
                workspace_dir: factory_ws.clone(),
                tool_router: LilaServer::tool_router(),
            })
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().with_cancellation_token(cancel.child_token()),
    );

    let token = Arc::new(token);
    let app = Router::new()
        .nest_service("/mcp", service)
        .layer(from_fn_with_state(token.clone(), require_bearer));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let bound = listener.local_addr()?.port();
    let serve_cancel = cancel.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move { serve_cancel.cancelled_owned().await })
            .await;
    });

    let url = format!("http://127.0.0.1:{bound}/mcp");
    tracing::info!(%url, "Lila MCP server listening (loopback, bearer-guarded)");
    Ok(RunningMcp {
        url,
        token: token.as_ref().clone(),
        port: bound,
        cancel,
    })
}

/// Bearer-auth middleware: reject anything without `Authorization: Bearer <token>`.
async fn require_bearer(
    State(token): State<Arc<String>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let presented = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if presented == Some(format!("Bearer {token}").as_str()) {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}
