//! The Inspector — a read-only HTTP plane over the manager's live state. In Docker it is bound to
//! the Compose network and fronted by Caddy at `/_inspect` (a
//! `handle_path` strips the prefix, so this server sees plain `/`, `/api/*`). It is deliberately NOT
//! a model tool (the manager's "no hands" boundary stays airtight); it only observes.
//!
//! It reads two cheap in-process sources behind their existing `Arc<Mutex>` handles — the passive
//! [`Telemetry`] recorder (usage, turns, the reconstructed conversation, the worker dispatch
//! history) and [`MemFs`] (memory files on disk). Nothing here mutates runtime state.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::get;
use serde::Serialize;
use tokio::sync::Notify;

use super::html::INSPECTOR_HTML;
use crate::memory::MemFs;
use crate::runtime::telemetry::{ConvMessage, Telemetry, TurnRecord, UsageMeter, WorkerPrompt};

/// Everything the Inspector needs to stand up. Built in the `run` command from the same handles the
/// loop already owns, so it observes live state with no extra bookkeeping.
pub struct InspectorConfig {
    pub host: String,
    pub port: u16,
    /// Required secret; when set, every request must carry `?t=` or `x-inspector-token`.
    pub token: Option<String>,
    pub manager_model: String,
    pub backend: String,
    pub workspace_dir: String,
    pub app_public_url: String,
    pub telemetry: Arc<Mutex<Telemetry>>,
    pub mem: Arc<Mutex<MemFs>>,
}

#[derive(Clone)]
struct InspectorState {
    meta: Arc<Meta>,
    telemetry: Arc<Mutex<Telemetry>>,
    mem: Arc<Mutex<MemFs>>,
}

struct Meta {
    manager_model: String,
    backend: String,
    workspace_dir: String,
    app_public_url: String,
}

/// Start the Inspector on its loopback port; returns the bound port (matters when 0 is requested,
/// e.g. in tests). The server runs in a background task and stops on `shutdown`.
pub async fn start(cfg: InspectorConfig, shutdown: Arc<Notify>) -> std::io::Result<u16> {
    let state = InspectorState {
        meta: Arc::new(Meta {
            manager_model: cfg.manager_model,
            backend: cfg.backend,
            workspace_dir: cfg.workspace_dir,
            app_public_url: cfg.app_public_url,
        }),
        telemetry: cfg.telemetry,
        mem: cfg.mem,
    };
    let token = Arc::new(cfg.token);
    let app = Router::new()
        .route("/", get(index))
        .route("/api/overview", get(overview))
        .route("/api/conversation", get(conversation))
        .route("/api/usage", get(usage))
        .route("/api/workers", get(workers))
        .route("/api/memories", get(memories))
        .route("/api/trace", get(trace))
        .with_state(state)
        .layer(from_fn_with_state(token, require_token));

    let host = cfg.host;
    let listener = tokio::net::TcpListener::bind((host.as_str(), cfg.port)).await?;
    let bound_addr = listener.local_addr()?;
    let bound = bound_addr.port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move { shutdown.notified().await })
            .await;
    });
    tracing::info!(addr = %bound_addr, "Inspector listening (read-only)");
    Ok(bound)
}

/// Auth middleware: a single shared secret, accepted as `?t=` or the `x-inspector-token` header.
/// Skipped only when no token is configured (Caddy basic_auth is then expected to be the guard).
async fn require_token(
    State(token): State<Arc<Option<String>>>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(expected) = (*token).as_deref()
        && !token_ok(&req, expected)
    {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    next.run(req).await
}

fn token_ok(req: &Request, expected: &str) -> bool {
    let from_header = req
        .headers()
        .get("x-inspector-token")
        .and_then(|v| v.to_str().ok());
    let from_query = req.uri().query().and_then(|q| query_param(q, "t"));
    from_header == Some(expected) || from_query.as_deref() == Some(expected)
}

/// Pull a single query-string value (no decoding needed: the token is URL-safe and the HTML also
/// sends it via the `x-inspector-token` header, which is the canonical path).
fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

// ---- handlers --------------------------------------------------------------

async fn index() -> Html<&'static str> {
    Html(INSPECTOR_HTML)
}

async fn overview(State(s): State<InspectorState>) -> Json<Overview> {
    let memories = read_mem(&s.mem).len();
    Json(read_tel(&s.telemetry, |t| {
        let prompts = t.prompts();
        let turns = t.turns();
        Overview {
            manager_model: s.meta.manager_model.clone(),
            backend: s.meta.backend.clone(),
            workspace_dir: s.meta.workspace_dir.clone(),
            app_public_url: non_empty(&s.meta.app_public_url),
            context_tokens: t.context_tokens(),
            usage: t.meter(),
            counts: Counts {
                turns: turns.len(),
                workers: unique_workers(&prompts),
                memories,
            },
            last_turn: turns.into_iter().next_back(),
        }
    }))
}

async fn conversation(State(s): State<InspectorState>) -> Json<Conversation> {
    Json(read_tel(&s.telemetry, |t| {
        let messages = t.conversation();
        Conversation {
            context_tokens: t.context_tokens(),
            message_count: messages.len(),
            messages,
        }
    }))
}

async fn usage(State(s): State<InspectorState>) -> Json<Usage> {
    Json(read_tel(&s.telemetry, |t| Usage {
        meter: t.meter(),
        note: NOTE,
        turns: t.turns(),
    }))
}

// Workers are ephemeral (single-shot) — there is no registry. This is the dispatch history: every
// launch the telemetry traced, with the exact prompt it received, grouped by worker, newest first.
async fn workers(State(s): State<InspectorState>) -> Json<Workers> {
    Json(read_tel(&s.telemetry, |t| Workers {
        workers: group_workers(t.prompts()),
    }))
}

async fn memories(State(s): State<InspectorState>) -> Json<Memories> {
    let files = read_mem(&s.mem)
        .into_iter()
        .map(|(path, body)| MemoryFile { path, body })
        .collect();
    Json(Memories { files })
}

// Every turn with the worker prompts it spawned, newest first (the Inspector's Trace timeline).
async fn trace(State(s): State<InspectorState>) -> Json<Trace> {
    Json(read_tel(&s.telemetry, |t| {
        let prompts = t.prompts();
        let mut turns: Vec<TraceTurn> = t
            .turns()
            .into_iter()
            .map(|turn| TraceTurn {
                prompts: prompts
                    .iter()
                    .filter(|p| p.turn_id == turn.turn_id)
                    .cloned()
                    .collect(),
                turn,
            })
            .collect();
        turns.reverse();
        Trace { turns }
    }))
}

// ---- view models -----------------------------------------------------------

const NOTE: &str = "Everything rides the subscription — no metered $. These are token counts only.";

#[derive(Serialize, Default)]
struct Overview {
    manager_model: String,
    backend: String,
    workspace_dir: String,
    app_public_url: Option<String>,
    context_tokens: u64,
    usage: UsageMeter,
    counts: Counts,
    last_turn: Option<TurnRecord>,
}

#[derive(Serialize, Default)]
struct Counts {
    turns: usize,
    workers: usize,
    memories: usize,
}

#[derive(Serialize, Default)]
struct Conversation {
    context_tokens: u64,
    message_count: usize,
    messages: Vec<ConvMessage>,
}

#[derive(Serialize, Default)]
struct Usage {
    meter: UsageMeter,
    note: &'static str,
    turns: Vec<TurnRecord>,
}

#[derive(Serialize, Default)]
struct Workers {
    workers: Vec<WorkerLane>,
}

#[derive(Serialize)]
struct WorkerLane {
    id: String,
    prompts: Vec<WorkerPrompt>,
}

#[derive(Serialize, Default)]
struct Memories {
    files: Vec<MemoryFile>,
}

#[derive(Serialize)]
struct MemoryFile {
    path: String,
    body: String,
}

#[derive(Serialize, Default)]
struct Trace {
    turns: Vec<TraceTurn>,
}

#[derive(Serialize)]
struct TraceTurn {
    #[serde(flatten)]
    turn: TurnRecord,
    prompts: Vec<WorkerPrompt>,
}

// ---- helpers ---------------------------------------------------------------

/// Read the telemetry under its lock, mapping it to a view; a poisoned lock yields the default view.
fn read_tel<T: Default>(tel: &Arc<Mutex<Telemetry>>, f: impl FnOnce(&Telemetry) -> T) -> T {
    tel.lock().map(|t| f(&t)).unwrap_or_default()
}

fn read_mem(mem: &Arc<Mutex<MemFs>>) -> Vec<(String, String)> {
    mem.lock().map(|m| m.list_all()).unwrap_or_default()
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

fn unique_workers(prompts: &[WorkerPrompt]) -> usize {
    let mut ids: Vec<&str> = prompts.iter().map(|p| p.worker_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.len()
}

/// Group the dispatch history by worker id, newest worker first.
fn group_workers(prompts: Vec<WorkerPrompt>) -> Vec<WorkerLane> {
    let mut order: Vec<String> = Vec::new();
    for p in &prompts {
        if !order.contains(&p.worker_id) {
            order.push(p.worker_id.clone());
        }
    }
    order
        .into_iter()
        .rev()
        .map(|id| WorkerLane {
            prompts: prompts
                .iter()
                .filter(|p| p.worker_id == id)
                .cloned()
                .collect(),
            id,
        })
        .collect()
}
