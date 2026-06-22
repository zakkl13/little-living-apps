//! The manager runtime composition root + the serialized loop.
//!
//! Design (idiomatic, no global state): ONE task owns all mutable manager state — the queue, the
//! turn driver, the snapshot store, the Telegram client. Producers (the poller, the worker
//! orchestrator) push into channels this task drains. Turns are serialized because this is the sole
//! consumer. The only shared state is memory + orchestrator + telemetry (touched by the MCP server
//! task too), held behind `Arc<Mutex<…>>`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;
use tokio::sync::mpsc::{self, UnboundedReceiver};

use super::backend::{BackendEvent, TurnInput};
use super::driver::{ManagerDriver, TurnOutcome};
use super::prompt::build_context_header;
use crate::config::{AgentBackend, Config};
use crate::memory::MemFs;
use crate::runtime::event::ManagerEvent;
use crate::runtime::telemetry::Telemetry;
use crate::runtime::trace::{EvalTrace, TraceBlock, TraceExt, TraceRecord};
use crate::runtime::{EventQueue, ManagerSnapshot, SnapshotStore};
use crate::transport::deliver::deliver;
use crate::transport::{TelegramClient, TelegramUpdate, poller};
use crate::workers::Orchestrator;

/// Owned, single-task manager state.
pub struct App {
    cfg: Config,
    queue: EventQueue,
    driver: ManagerDriver,
    snapshots: SnapshotStore,
    telegram: TelegramClient,
    mem: Arc<Mutex<MemFs>>,
    orch: Arc<Orchestrator>,
    telemetry: Arc<Mutex<Telemetry>>,
    /// The eval/inspector trace (None in prod): records the timeline + conversation for grading.
    trace: Option<Arc<EvalTrace>>,
    owner_chat: i64,
    turn_counter: u64,
    /// Shutdown signal shared with the poller; also flipped on a `/backend` swap to exit cleanly.
    shutdown: Arc<Notify>,
    restart_requested: Arc<AtomicBool>,
}

/// Handles the `run` command wires into the app (everything but the queue/loop internals).
pub struct AppConfig {
    pub cfg: Config,
    pub driver: ManagerDriver,
    pub telegram: TelegramClient,
    pub mem: Arc<Mutex<MemFs>>,
    pub orch: Arc<Orchestrator>,
    pub telemetry: Arc<Mutex<Telemetry>>,
    pub trace: Option<Arc<EvalTrace>>,
    pub shutdown: Arc<Notify>,
    pub restart_requested: Arc<AtomicBool>,
}

impl App {
    pub fn new(c: AppConfig) -> Self {
        let owner_chat = c.cfg.owner_user_id();
        let snapshots = SnapshotStore::new(&c.cfg.manager_state_dir);
        Self {
            cfg: c.cfg,
            queue: EventQueue::new(),
            driver: c.driver,
            snapshots,
            telegram: c.telegram,
            mem: c.mem,
            orch: c.orch,
            telemetry: c.telemetry,
            trace: c.trace,
            owner_chat,
            turn_counter: 0,
            shutdown: c.shutdown,
            restart_requested: c.restart_requested,
        }
    }

    /// The shutdown signal (shared with the poller).
    pub fn shutdown(&self) -> Arc<Notify> {
        self.shutdown.clone()
    }

    /// Rehydrate from the last snapshot (cold-restart recovery).
    pub fn restore(&mut self) {
        let Some(snap) = self.snapshots.load() else {
            return;
        };
        // Session ids are backend-specific; only resume if the boot backend matches the snapshot's.
        let resume = (snap.backend == self.cfg.agent_backend.as_str())
            .then_some(snap.manager_session_id.clone())
            .flatten();
        self.driver.adopt_session_id(resume.clone());
        self.queue.load(snap.queue);
        if let Ok(mut t) = self.telemetry.lock() {
            t.load_usage(snap.usage);
        }
        tracing::info!(
            resumed = resume.is_some(),
            pending = self.queue.len(),
            "restored manager state from snapshot"
        );
    }

    /// Persist session id + queue + usage (after each turn).
    pub fn persist(&self) {
        let usage = self
            .telemetry
            .lock()
            .map(|t| t.usage_snapshot())
            .unwrap_or_default();
        let snap = ManagerSnapshot::new(
            self.cfg.agent_backend,
            self.driver.session_id(),
            self.queue.snapshot(),
            usage,
        );
        if let Err(err) = self.snapshots.save(&snap) {
            tracing::error!(%err, "failed to persist snapshot");
        }
    }

    /// Authorize + handle commands + photo intake + enqueue, from a raw Telegram update.
    async fn ingest(&mut self, update: TelegramUpdate) {
        let Some(msg) = update.message().cloned() else {
            return;
        };
        let chat_id = msg.chat.id;
        let from_id = msg.from.as_ref().map(|u| u.id);

        if from_id.is_none_or(|id| !self.cfg.allowed_user_ids.contains(&id)) {
            tracing::warn!(?from_id, "rejected unauthorized update");
            let _ = self
                .telegram
                .send_message(chat_id, "⛔ You are not authorized to use this bot.")
                .await;
            return;
        }

        // Photo intake (vision): download the largest size and open the turn with it.
        if let Some(photos) = msg.photo.as_ref().filter(|p| !p.is_empty()) {
            let image_path = self.download_photo(&photos[photos.len() - 1].file_id).await;
            let caption = msg.caption.unwrap_or_default();
            let text = if caption.trim().is_empty() {
                "(the owner sent an image)".to_string()
            } else {
                caption
            };
            self.queue
                .push(ManagerEvent::owner(chat_id, text, image_path));
            return;
        }

        let Some(text) = msg.text else { return };
        let text = text.trim().to_string();
        if text.starts_with('/') {
            self.handle_command(&text, chat_id).await;
            return;
        }
        self.queue.push(ManagerEvent::owner(chat_id, text, None));
    }

    /// Download an owner-sent photo to a temp path; `None` on failure (turn proceeds as text).
    async fn download_photo(&self, file_id: &str) -> Option<String> {
        let file_path = self.telegram.get_file(file_id).await.ok().flatten()?;
        let bytes = self.telegram.download_file(&file_path).await.ok()?;
        let ext = file_path
            .rsplit('.')
            .next()
            .filter(|e| e.len() <= 5)
            .unwrap_or("jpg");
        let dest = std::env::temp_dir().join(format!(
            "lila-photo-{}.{ext}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&dest, bytes).ok()?;
        Some(dest.to_string_lossy().into_owned())
    }

    async fn handle_command(&mut self, text: &str, chat_id: i64) {
        let cmd = text
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_start_matches('/');
        let cmd = cmd.split('@').next().unwrap_or(cmd).to_lowercase();
        match cmd.as_str() {
            "start" | "help" => self.reply(chat_id, HELP_TEXT).await,
            "status" => {
                let body = format!(
                    "Backend: {}\nWorkers running: {}\nPending events: {}\nMemory: {}",
                    self.cfg.agent_backend,
                    self.orch.running(),
                    self.queue.len(),
                    self.cfg.memory_dir,
                );
                self.reply(chat_id, &body).await;
            }
            "new" => {
                self.driver.reset();
                self.persist();
                self.reply(
                    chat_id,
                    "🆕 Started a fresh manager thread. Long-term memory is untouched.",
                )
                .await;
            }
            "backend" => self.handle_backend(text, chat_id).await,
            other => {
                self.reply(chat_id, &format!("Unknown command: /{other}. Try /help."))
                    .await
            }
        }
    }

    /// `/backend [codex|claude]`: show or switch. A switch persists the choice and restarts cleanly.
    async fn handle_backend(&mut self, text: &str, chat_id: i64) {
        let Some(arg) = text.split_whitespace().nth(1).map(|s| s.to_lowercase()) else {
            self.reply(
                chat_id,
                &format!(
                    "Backend: {}. Switch with /backend codex or /backend claude.",
                    self.cfg.agent_backend
                ),
            )
            .await;
            return;
        };
        let target: AgentBackend = match arg.parse() {
            Ok(b) => b,
            Err(_) => {
                self.reply(
                    chat_id,
                    &format!("Unknown backend \"{arg}\". Options: codex, claude."),
                )
                .await;
                return;
            }
        };
        if target == self.cfg.agent_backend {
            self.reply(chat_id, &format!("Already on the {target} backend."))
                .await;
            return;
        }
        if let Some(err) = crate::config::billing_guard_error(target, &crate::config::process_env())
        {
            self.reply(chat_id, &format!("⛔ Can't switch to {target}: {err}"))
                .await;
            return;
        }
        if let Err(err) = self.persist_backend_choice(target) {
            self.reply(
                chat_id,
                &format!("⛔ Couldn't persist the backend choice: {err}"),
            )
            .await;
            return;
        }
        self.persist();
        self.reply(
            chat_id,
            &format!("🔄 Switching to the {target} backend and restarting — memory is kept, but this starts a fresh thread."),
        )
        .await;
        tracing::info!(from = %self.cfg.agent_backend, to = %target, "backend swap requested");
        self.restart_requested.store(true, Ordering::SeqCst);
        self.shutdown.notify_waiters();
    }

    fn persist_backend_choice(&self, target: AgentBackend) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.cfg.manager_state_dir)?;
        let path = std::path::Path::new(&self.cfg.manager_state_dir).join("backend");
        std::fs::write(path, format!("{target}\n"))
    }

    async fn reply(&self, chat_id: i64, text: &str) {
        if let Err(err) = self.telegram.send_message(chat_id, text).await {
            tracing::warn!(%err, "failed to deliver system reply");
        }
    }

    /// Run exactly one manager turn for `event`, then deliver per the reply gate.
    async fn run_one_turn(&mut self, event: ManagerEvent) {
        self.turn_counter += 1;
        let turn_id = self.turn_counter;
        let chat_id = self.chat_id_for(&event);
        let request = render_request(&event);
        if let Ok(mut t) = self.telemetry.lock() {
            t.begin_turn(turn_id, event.kind_str(), request.clone(), chat_id);
            if let ManagerEvent::OwnerMessage { text, .. } = &event {
                t.record_user_message(text.clone());
            }
        }
        // Stamp the turn on both observability planes: the orchestrator (so worker dispatches are
        // grouped under the launching turn in `lila status`/Inspector) and the trace (eval grading).
        self.orch.set_turn(turn_id);
        self.trace_turn_start(turn_id, &event);

        let header = self
            .mem
            .lock()
            .map(|m| build_context_header(&m))
            .unwrap_or_default();
        let input = self.turn_input(&event, request);

        let telemetry = self.telemetry.clone();
        let trace = self.trace.clone();
        let mut observer = move |ev: &BackendEvent| {
            record_event_telemetry(&telemetry, turn_id, ev);
            trace_backend_event(trace.as_deref(), ev);
        };
        let outcome = self.driver.run_turn(&header, input, &mut observer).await;
        self.deliver_outcome(&event, chat_id, outcome).await;
    }

    /// Stamp the trace with the turn now in flight, and log an owner message as a timeline event.
    fn trace_turn_start(&self, turn_id: u64, event: &ManagerEvent) {
        let Some(trace) = &self.trace else { return };
        trace.set_turn(turn_id);
        if let ManagerEvent::OwnerMessage { text, .. } = event {
            trace.emit(&TraceRecord::OwnerMsg { text: text.clone() });
        }
    }

    fn turn_input(&self, event: &ManagerEvent, request: String) -> TurnInput {
        let image_path = match event {
            ManagerEvent::OwnerMessage { image_path, .. } => image_path.clone(),
            ManagerEvent::WorkerEvent { .. } => None,
        };
        TurnInput {
            text: request,
            image_path,
        }
    }

    fn chat_id_for(&self, event: &ManagerEvent) -> i64 {
        match event {
            ManagerEvent::OwnerMessage { chat_id, .. } => *chat_id,
            ManagerEvent::WorkerEvent { .. } => self.owner_chat,
        }
    }

    /// Deliver a turn outcome, applying the worker-event reply gate.
    async fn deliver_outcome(&self, event: &ManagerEvent, chat_id: i64, outcome: TurnOutcome) {
        match outcome {
            TurnOutcome::Silent => {}
            TurnOutcome::Error(text) => self.reply(chat_id, &text).await,
            TurnOutcome::Reply { text, attachments } => {
                if !self.allow_reply(event) {
                    return; // a worker event while work is still in flight → fold silently
                }
                self.trace.rec(TraceRecord::Delivery { text: text.clone() });
                if let Err(err) = deliver(&self.telegram, chat_id, &text, &attachments).await {
                    tracing::warn!(%err, "failed to deliver manager reply");
                }
            }
        }
    }

    /// The reply gate: owner messages always reply; a worker event only replies once all work has
    /// settled (no runs in flight and no further worker events queued).
    fn allow_reply(&self, event: &ManagerEvent) -> bool {
        match event {
            ManagerEvent::OwnerMessage { .. } => true,
            ManagerEvent::WorkerEvent { .. } => {
                self.orch.running() == 0 && !self.queue.has_worker_event()
            }
        }
    }
}

/// Run the manager: restore, start the poller, then drain the queue one turn at a time.
pub async fn run(mut app: App, mut events_rx: UnboundedReceiver<ManagerEvent>) {
    app.restore();

    // Outbound-only: clear any stale webhook, then long-poll in a background task.
    let _ = app.telegram.delete_webhook().await;
    let (updates_tx, mut updates_rx) = mpsc::unbounded_channel::<TelegramUpdate>();
    let poller = tokio::spawn(poller::run(
        app.telegram.clone(),
        updates_tx,
        app.shutdown.clone(),
    ));

    tracing::info!(backend = %app.cfg.agent_backend, "little-living-apps ready");
    drive_loop(&mut app, &mut updates_rx, &mut events_rx).await;
    app.persist();
    poller.abort();
}

/// The serialized consumer loop: block for input, drain what's ready, process one turn, repeat.
async fn drive_loop(
    app: &mut App,
    updates_rx: &mut UnboundedReceiver<TelegramUpdate>,
    events_rx: &mut UnboundedReceiver<ManagerEvent>,
) {
    let shutdown = app.shutdown.clone();
    loop {
        if app.queue.is_empty() {
            // Quiescent (empty queue, no workers in flight) → mark it so the eval harness can detect
            // the cascade has settled before sending the next owner turn.
            maybe_emit_idle(app);
            if !block_for_input(app, updates_rx, events_rx, &shutdown).await {
                break; // shutdown, or both producers closed
            }
        }
        drain_ready(app, updates_rx, events_rx).await;
        app.persist();
        if let Some(event) = app.queue.pop() {
            app.run_one_turn(event).await;
            app.persist();
        }
        if app.restart_requested.load(Ordering::SeqCst) {
            break;
        }
    }
}

/// Block until one input arrives (or shutdown). Returns `false` to stop the loop.
async fn block_for_input(
    app: &mut App,
    updates_rx: &mut UnboundedReceiver<TelegramUpdate>,
    events_rx: &mut UnboundedReceiver<ManagerEvent>,
    shutdown: &Notify,
) -> bool {
    tokio::select! {
        _ = shutdown.notified() => false,
        u = updates_rx.recv() => match u {
            Some(u) => {
                app.ingest(u).await;
                true
            }
            None => false,
        },
        e = events_rx.recv() => {
            if let Some(e) = e {
                app.queue.push(e);
            }
            true
        }
    }
}

/// Drain everything immediately available from both producers into the queue.
async fn drain_ready(
    app: &mut App,
    updates_rx: &mut UnboundedReceiver<TelegramUpdate>,
    events_rx: &mut UnboundedReceiver<ManagerEvent>,
) {
    while let Ok(u) = updates_rx.try_recv() {
        app.ingest(u).await;
    }
    while let Ok(e) = events_rx.try_recv() {
        app.queue.push(e);
    }
}

/// Emit an `idle` trace marker when the loop is truly quiescent: no queued events AND no worker runs
/// in flight (a pending worker would still wake the loop with its event, so that is not idle).
fn maybe_emit_idle(app: &App) {
    if app.orch.running() == 0 {
        app.trace.rec(TraceRecord::Idle);
    }
}

/// Fold a streamed backend event into the live in-process telemetry (read by `lila status` + the
/// Inspector): usage updates the meter; text/reasoning/tool activity appends to the conversation.
fn record_event_telemetry(telemetry: &Arc<Mutex<Telemetry>>, turn_id: u64, ev: &BackendEvent) {
    let Ok(mut t) = telemetry.lock() else { return };
    if let BackendEvent::Usage(u) = ev {
        t.record_usage(turn_id, *u);
    } else if let Some(blocks) = backend_blocks(ev) {
        t.record_assistant_blocks(blocks);
    }
}

/// Map a streamed backend event onto the trace's manager-conversation view (the model-level log the
/// `choseSilence` grader and the judge read). `Usage` is tagged as manager-tier; `Failed` is handled
/// as the turn outcome, not the conversation.
fn trace_backend_event(trace: Option<&EvalTrace>, ev: &BackendEvent) {
    let Some(trace) = trace else { return };
    if let BackendEvent::Usage(u) = ev {
        return trace.usage("manager", *u);
    }
    if let Some(blocks) = backend_blocks(ev) {
        trace.emit(&TraceRecord::ManagerMsg {
            role: "assistant".into(),
            blocks,
        });
    }
}

/// The conversation blocks a streamed event contributes (shared by the telemetry + trace planes).
/// `Usage`/`Failed` contribute none (usage is metered separately; a failure is the turn outcome).
fn backend_blocks(ev: &BackendEvent) -> Option<Vec<TraceBlock>> {
    match ev {
        BackendEvent::AgentMessage(text) => Some(vec![TraceBlock::Text { text: text.clone() }]),
        BackendEvent::Reasoning(_) => Some(vec![TraceBlock::Thinking]),
        BackendEvent::ToolCall { tool, .. } => {
            Some(vec![TraceBlock::ToolUse { name: tool.clone() }])
        }
        BackendEvent::Usage(_) | BackendEvent::Failed(_) => None,
    }
}

/// A worker event leads with the objective's first line so the event is self-describing.
fn render_request(event: &ManagerEvent) -> String {
    match event {
        ManagerEvent::OwnerMessage { text, .. } => text.clone(),
        ManagerEvent::WorkerEvent {
            status,
            objective,
            summary,
            ..
        } => {
            let first = objective
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim();
            let label: String = first.chars().take(80).collect();
            format!("[subagent {}: {label}]\n{summary}", status.as_str())
        }
    }
}

const HELP_TEXT: &str = "🤖 Manager ready. Tell me what to build and I'll delegate to workers, \
    remember what matters, and report back.\n\nCommands:\n/status — workers + state\n\
    /new — start a fresh manager thread (memory is kept)\n\
    /backend [codex|claude] — show or switch the agent backend (restarts; memory kept)";
