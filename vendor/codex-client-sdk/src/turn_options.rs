use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Per-turn options for [`crate::Thread::run`] and [`crate::Thread::run_streamed`].
#[derive(Debug, Clone, Default)]
pub struct TurnOptions {
    /// JSON schema describing expected agent output.
    pub output_schema: Option<Value>,
    /// Cancellation token used to abort an in-flight turn.
    pub cancellation_token: Option<CancellationToken>,
}
