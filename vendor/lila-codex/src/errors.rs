use thiserror::Error;

/// A specialized `Result` type for Codex SDK operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Unified error type for all Codex SDK operations.
#[derive(Debug, Error)]
pub enum Error {
    /// The `codex` executable could not be resolved.
    #[error("Codex CLI not found: {0}")]
    CliNotFound(String),

    /// Spawning the Codex subprocess failed.
    #[error("Failed to spawn Codex CLI: {0}")]
    Spawn(String),

    /// The Codex subprocess exited unsuccessfully.
    #[error("Codex process exited with {detail}: {stderr}")]
    Process {
        /// Human-readable exit detail such as `code 2` or signal termination.
        detail: String,
        /// Full stderr output captured from the Codex subprocess.
        stderr: String,
        /// Numeric exit code when available.
        code: Option<i32>,
    },

    /// A line from stdout could not be decoded as JSON.
    #[error("Failed to parse JSON event: {0}")]
    JsonParse(String),

    /// A thread turn failed according to Codex turn-level error events.
    #[error("Thread run failed: {0}")]
    ThreadRun(String),

    /// The provided output schema is invalid.
    #[error("Invalid output schema: {0}")]
    InvalidOutputSchema(String),

    /// A `config` override cannot be represented as Codex CLI TOML literal flags.
    #[error("Invalid config override: {0}")]
    InvalidConfig(String),

    /// The caller cancelled an in-flight turn.
    #[error("Turn cancelled")]
    Cancelled,

    /// Wrapper for I/O errors while interacting with the subprocess or temp files.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Wrapper for JSON serialization/deserialization errors.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
