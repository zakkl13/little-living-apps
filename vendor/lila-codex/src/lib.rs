//! # Codex SDK for Rust
//!
//! Embed Codex in Rust applications by wrapping the `codex` CLI and exchanging
//! JSONL events over stdin/stdout.
//!
//! ## When To Use Which API
//!
//! - Use [`Thread::run`] when you only need the final turn result.
//! - Use [`Thread::run_streamed`] when you need progress events while the turn runs.
//! - Use [`Codex::resume_thread`] when continuing an existing saved thread.
//!
//! ## Quickstart
//!
//! ```rust,no_run
//! use codex::Codex;
//!
//! # async fn example() -> codex::Result<()> {
//! let codex = Codex::new(None)?;
//! let thread = codex.start_thread(None);
//! let turn = thread
//!     .run("Diagnose the test failure and propose a fix", None)
//!     .await?;
//!
//! println!("response: {}", turn.final_response);
//! # Ok(())
//! # }
//! ```
//!
//! ## Continue The Same Thread
//!
//! ```rust,no_run
//! use codex::Codex;
//!
//! # async fn example() -> codex::Result<()> {
//! let codex = Codex::new(None)?;
//! let thread = codex.start_thread(None);
//! let _first = thread.run("Inspect failing tests", None).await?;
//! let second = thread.run("Apply a fix", None).await?;
//! println!("{}", second.final_response);
//! # Ok(())
//! # }
//! ```
//!
//! ## Stream Events
//!
//! ```rust,no_run
//! use codex::{Codex, ThreadEvent};
//! use futures::StreamExt;
//!
//! # async fn example() -> codex::Result<()> {
//! let codex = Codex::new(None)?;
//! let thread = codex.start_thread(None);
//! let mut events = thread.run_streamed("Analyze repository state", None).await?.events;
//!
//! while let Some(event) = events.next().await {
//!     if let ThreadEvent::TurnCompleted { usage } = event? {
//!         println!("usage: {:?}", usage);
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Structured Output
//!
//! ```rust,no_run
//! use codex::{Codex, TurnOptions};
//! use serde_json::json;
//!
//! # async fn example() -> codex::Result<()> {
//! let codex = Codex::new(None)?;
//! let thread = codex.start_thread(None);
//! let schema = json!({
//!     "type": "object",
//!     "properties": { "summary": { "type": "string" } },
//!     "required": ["summary"],
//!     "additionalProperties": false
//! });
//!
//! let turn = thread
//!     .run(
//!         "Summarize the repository status",
//!         Some(TurnOptions {
//!             output_schema: Some(schema),
//!             ..Default::default()
//!         }),
//!     )
//!     .await?;
//! println!("{}", turn.final_response);
//! # Ok(())
//! # }
//! ```

/// High-level client used to start and resume Codex threads.
pub mod codex;
/// Client-level options, environment configuration, and `--config` overrides.
pub mod codex_options;
/// Shared error types and `Result` alias.
pub mod errors;
/// Stream event payloads emitted by `codex exec --experimental-json`.
pub mod events;
/// Low-level subprocess execution layer for invoking the Codex CLI.
pub mod exec;
/// Canonical item payloads produced inside a thread turn.
pub mod items;
/// Temporary output-schema file helpers for structured output turns.
pub mod output_schema_file;
/// Thread and turn execution APIs (`run` and `run_streamed`).
pub mod thread;
/// Per-thread execution options mapped to Codex CLI flags/config.
pub mod thread_options;
/// Per-turn options such as output schema and cancellation support.
pub mod turn_options;

pub use codex::Codex;
pub use codex_options::{CodexConfigObject, CodexConfigValue, CodexOptions};
pub use errors::{Error, Result};
pub use events::{
    ItemCompletedEvent, ItemStartedEvent, ItemUpdatedEvent, ThreadError, ThreadErrorEvent,
    ThreadEvent, ThreadStartedEvent, TurnCompletedEvent, TurnFailedEvent, TurnStartedEvent, Usage,
};
pub use exec::{CodexExec, CodexExecArgs};
pub use items::{
    AgentMessageItem, CommandExecutionItem, CommandExecutionStatus, ErrorItem, FileChangeItem,
    FileUpdateChange, McpToolCallError, McpToolCallItem, McpToolCallResult, McpToolCallStatus,
    PatchApplyStatus, PatchChangeKind, ReasoningItem, ThreadItem, TodoItem, TodoListItem,
    WebSearchItem,
};
pub use thread::{Input, RunResult, RunStreamedResult, Thread, Turn, UserInput};
pub use thread_options::{
    ApprovalMode, ModelReasoningEffort, SandboxMode, ThreadOptions, WebSearchMode,
};
pub use turn_options::TurnOptions;

/// The version of the Codex Rust SDK, sourced from `Cargo.toml`.
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");
