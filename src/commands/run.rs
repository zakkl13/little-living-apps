//! `lila run` — the long-lived manager daemon.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::manager::app::{App, AppConfig, run as run_app};
use crate::manager::backend::ManagerBackend;
use crate::manager::driver::ManagerDriver;
use crate::memory::{MemFs, MemFsOptions};
use crate::runtime::ManagerEvent;
use crate::runtime::telemetry::Telemetry;
use crate::runtime::trace::EvalTrace;
use crate::transport::TelegramClient;
use crate::workers::{Orchestrator, Runner};

pub async fn run() -> i32 {
    // Honor a persisted `/backend` choice (written by the in-chat command) before loading config.
    let cfg = match load_config_with_override() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("CONFIG ERROR: {err}");
            return 1;
        }
    };

    match build_and_run(cfg).await {
        Ok(()) => 0,
        Err(err) => {
            tracing::error!(%err, "manager exited with error");
            eprintln!("ERROR: {err}");
            1
        }
    }
}

async fn build_and_run(cfg: Config) -> anyhow::Result<()> {
    let telegram = TelegramClient::new(&cfg.telegram_api_base_url, &cfg.telegram_bot_token);
    let mem = Arc::new(Mutex::new(MemFs::open(MemFsOptions {
        dir: cfg.memory_dir.clone().into(),
        fts_path: format!("{}.fts.sqlite", cfg.memory_dir),
    })?));
    let telemetry = Arc::new(Mutex::new(Telemetry::new()));
    // Eval/inspector trace: Some(...) only when LILA_EVAL_TRACE is set (the eval harness drives the
    // real binary with it pointed at a per-trial file); None and zero-cost in production.
    let trace = EvalTrace::from_env().map(Arc::new);
    let (events_tx, events_rx) = mpsc::unbounded_channel::<ManagerEvent>();

    let runner = build_runner(&cfg)?;
    let orch = Arc::new(Orchestrator::new(
        runner,
        cfg.workspace_dir.clone().into(),
        events_tx,
        telemetry.clone(),
        trace.clone(),
    ));

    // Build the backend (fake, or real + its Lila MCP server which we keep alive for the run).
    let (backend, mcp_guard) = build_backend(&cfg, &mem, &orch).await?;
    let driver = ManagerDriver::new(backend);

    let shutdown = Arc::new(Notify::new());
    let restart_requested = Arc::new(AtomicBool::new(false));
    install_signal_handlers(shutdown.clone());

    // The read-only Inspector plane (off unless INSPECTOR_ENABLED). It shares the live telemetry +
    // memory handles; the app still owns them (it is moved in below).
    maybe_start_inspector(&cfg, &mem, &telemetry, shutdown.clone()).await;

    let app = App::new(AppConfig {
        cfg,
        driver,
        telegram,
        mem,
        orch,
        telemetry,
        trace,
        shutdown,
        restart_requested,
    });
    run_app(app, events_rx).await;
    if let Some(mcp) = mcp_guard {
        mcp.close();
    }
    Ok(())
}

/// Start the Inspector if `INSPECTOR_ENABLED`; a bind failure is logged, not fatal (the manager runs
/// fine without it). Shares the live telemetry + memory handles read-only.
async fn maybe_start_inspector(
    cfg: &Config,
    mem: &Arc<Mutex<MemFs>>,
    telemetry: &Arc<Mutex<Telemetry>>,
    shutdown: Arc<Notify>,
) {
    if !cfg.inspector_enabled {
        return;
    }
    let icfg = crate::inspector::InspectorConfig {
        port: cfg.inspector_port,
        token: cfg.inspector_token.clone(),
        manager_model: cfg.manager_model.clone().unwrap_or_default(),
        backend: cfg.agent_backend.as_str().to_string(),
        workspace_dir: cfg.workspace_dir.clone(),
        app_public_url: cfg.app_public_url.clone(),
        telemetry: telemetry.clone(),
        mem: mem.clone(),
    };
    match crate::inspector::start(icfg, shutdown).await {
        Ok(port) => tracing::info!(port, "Inspector enabled (read-only)"),
        Err(err) => tracing::warn!(%err, "Inspector failed to start; continuing without it"),
    }
}

/// Build the worker runner for the active backend (or the fake under the `testing` feature).
fn build_runner(cfg: &Config) -> anyhow::Result<Arc<dyn Runner>> {
    if let Some(fake) = fake_runner() {
        return Ok(fake);
    }
    crate::workers::real::build_runner(cfg)
}

/// Build the manager backend. The fake path needs no MCP server; the real path returns the running
/// Lila MCP server alongside the backend so the caller keeps it alive for the run.
async fn build_backend(
    cfg: &Config,
    mem: &Arc<Mutex<MemFs>>,
    orch: &Arc<Orchestrator>,
) -> anyhow::Result<(
    Box<dyn ManagerBackend>,
    Option<crate::manager::mcp::RunningMcp>,
)> {
    if let Some(fake) = fake_backend() {
        return Ok((fake, None));
    }
    let (backend, mcp) = crate::manager::real::build_backend(cfg, mem, orch).await?;
    Ok((backend, Some(mcp)))
}

/// The scripted fake runner, when `LILA_FAKE_BACKEND` is set (integration tests). Inert otherwise.
fn fake_runner() -> Option<Arc<dyn Runner>> {
    std::env::var("LILA_FAKE_BACKEND")
        .is_ok()
        .then(|| Arc::new(crate::workers::fake_runner::FakeRunner::from_env()) as Arc<dyn Runner>)
}

/// The scripted fake backend, when `LILA_FAKE_BACKEND` is set (integration tests). Inert otherwise.
fn fake_backend() -> Option<Box<dyn ManagerBackend>> {
    std::env::var("LILA_FAKE_BACKEND").is_ok().then(|| {
        Box::new(crate::manager::fake_backend::FakeBackend::from_env()) as Box<dyn ManagerBackend>
    })
}

/// Load config, applying any persisted `/backend` override via the env map (no `set_var`, so the
/// `forbid(unsafe_code)` rule holds).
fn load_config_with_override() -> Result<Config, crate::config::ConfigError> {
    let mut env = crate::config::process_env();
    let state_dir = env
        .get("MANAGER_STATE_DIR")
        .cloned()
        .unwrap_or_else(|| "/var/lib/lila/state".into());
    let path = std::path::Path::new(state_dir.trim()).join("backend");
    if let Ok(contents) = std::fs::read_to_string(&path) {
        let choice = contents.trim();
        if choice == "codex" || choice == "claude" {
            env.insert("AGENT_BACKEND".to_string(), choice.to_string());
        }
    }
    Config::from_env(&env)
}

fn install_signal_handlers(shutdown: Arc<Notify>) {
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            match signal(SignalKind::terminate()) {
                Ok(mut term) => tokio::select! {
                    _ = ctrl_c => {},
                    _ = term.recv() => {},
                },
                Err(err) => {
                    tracing::warn!(%err, "could not install SIGTERM handler; using SIGINT only");
                    let _ = ctrl_c.await;
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = ctrl_c.await;
        }
        tracing::info!("shutdown signal received");
        shutdown.notify_waiters();
    });
}
