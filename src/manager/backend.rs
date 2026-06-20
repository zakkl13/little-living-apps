//! The backend seam: a backend-agnostic interface the [`super::driver::ManagerDriver`] drives, so
//! the loop is backend-blind and a `FakeBackend` can run the whole system in tests. Port of the
//! `ManagerThread` / `ManagerThreadFactory` seam in `src/manager/managerCodex.ts`.

use async_trait::async_trait;
use thiserror::Error;

use crate::runtime::TokenUsage;

/// What opens a turn: text, optionally with an owner-sent image (vision).
#[derive(Debug, Clone)]
pub struct TurnInput {
    pub text: String,
    pub image_path: Option<String>,
}

/// A normalized event streamed out of a backend turn (the union of what Codex and Claude emit).
#[derive(Debug, Clone)]
pub enum BackendEvent {
    /// An assistant message. The last one of a turn is the manager's reply to the owner.
    AgentMessage(String),
    /// An MCP tool call (internal: memory/orchestration). Logged, never delivered.
    ToolCall {
        server: String,
        tool: String,
        status: String,
        error: Option<String>,
    },
    /// Private chain-of-thought. Recorded for the Inspector, never delivered.
    Reasoning(String),
    /// Token usage for the turn (from `turn.completed`).
    Usage(TokenUsage),
    /// The turn failed; carries the owner-facing-able reason.
    Failed(String),
}

/// Backend failures (spawn/protocol/auth errors).
#[derive(Debug, Error)]
pub enum BackendError {
    #[error("{0}")]
    Run(String),
}

/// A backend-agnostic factory for manager threads, plus owner-facing error formatting.
#[async_trait]
pub trait ManagerBackend: Send + Sync {
    /// Create a manager thread — fresh, or resuming `resume` (a prior session id).
    fn thread(&self, resume: Option<String>) -> Box<dyn ManagerThread>;
    /// Turn a raw failure detail into owner-facing text (backend-specific wording).
    fn format_error(&self, detail: &str) -> String;
}

/// One long-lived manager thread. `run_turn` streams events to `on_event` until the turn ends.
#[async_trait]
pub trait ManagerThread: Send {
    /// The session id once the thread has started one (for snapshot/resume).
    fn session_id(&self) -> Option<String>;
    /// Run one turn, invoking `on_event` for each streamed event.
    async fn run_turn(
        &mut self,
        input: TurnInput,
        on_event: &mut (dyn FnMut(BackendEvent) + Send),
    ) -> Result<(), BackendError>;
}
