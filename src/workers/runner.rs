//! The worker runner seam: a single-shot backend run. Workers are purely ephemeral — every `run`
//! is a FRESH session that runs
//! one objective and is never resumed.

use std::path::PathBuf;

use async_trait::async_trait;
use thiserror::Error;

use crate::runtime::TokenUsage;

/// Inputs for a single worker run.
#[derive(Debug, Clone)]
pub struct RunArgs {
    /// The bare objective (standing rules come from the workspace `AGENTS.md`).
    pub prompt: String,
    /// Working directory for the run (the app workspace, or a subproject within it).
    pub cwd: PathBuf,
}

/// The result of a single worker run.
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub ok: bool,
    /// Final agent message (the worker's summary, per the contract).
    pub final_response: String,
    /// The single-use session id — a trace artifact, never used to resume.
    pub thread_id: Option<String>,
    /// Token usage the worker's backend reported for the run (zero if the CLI reported none). Folded
    /// into the cumulative worker totals so prod + eval can see what the work tier actually cost.
    pub usage: TokenUsage,
}

/// Auth/login probe result.
#[derive(Debug, Clone)]
pub struct LoginStatus {
    pub ok: bool,
    pub detail: String,
}

/// Worker run errors (spawn/protocol/auth).
#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("{0}")]
    Run(String),
}

/// Runs a single-shot worker objective.
#[async_trait]
pub trait Runner: Send + Sync {
    /// Run one objective to completion and return its summary.
    async fn run(&self, args: RunArgs) -> Result<RunOutcome, RunnerError>;
    /// Probe whether the backend is authenticated (subscription login present).
    async fn login_status(&self) -> LoginStatus;
}

/// Turn a non-zero-exit detail into owner-facing text (Codex wording).
pub fn friendly_error(detail: &str) -> String {
    let clipped: String = detail.chars().take(1500).collect();
    let lower = clipped.to_lowercase();
    if [
        "usage limit",
        "rate limit",
        "quota",
        "too many requests",
        "429",
        "purchase more credits",
    ]
    .iter()
    .any(|n| lower.contains(n))
    {
        return "⚠️ The agent hit a usage/rate limit. Wait for the quota to reset, or check \
                `codex login`."
            .to_string();
    }
    if lower.contains("login") || lower.contains("auth") || lower.contains("unauthorized") {
        return "⚠️ The agent isn't authenticated. Run `codex login` on the host.".to_string();
    }
    format!("⚠️ The agent run failed: {clipped}")
}

/// Claude-wording variant of [`friendly_error`].
pub fn friendly_claude_error(detail: &str) -> String {
    let clipped: String = detail.chars().take(1500).collect();
    let lower = clipped.to_lowercase();
    if [
        "usage limit",
        "rate limit",
        "quota",
        "too many requests",
        "429",
    ]
    .iter()
    .any(|n| lower.contains(n))
    {
        return "⚠️ Claude hit a usage/rate limit. Wait for the quota to reset, or check \
                `claude setup-token`."
            .to_string();
    }
    if lower.contains("login") || lower.contains("auth") || lower.contains("unauthorized") {
        return "⚠️ Claude isn't authenticated. Run `claude setup-token` on the host.".to_string();
    }
    format!("⚠️ The agent run failed: {clipped}")
}
