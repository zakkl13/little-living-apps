use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Lifecycle status for a command execution item.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandExecutionStatus {
    /// Command started and is still running.
    InProgress,
    /// Command finished successfully or with a captured exit code.
    Completed,
    /// Command failed before producing a completed state.
    Failed,
}

/// Command execution output item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandExecutionItem {
    /// Unique item id.
    pub id: String,
    /// Shell command text executed by the agent.
    pub command: String,
    /// Aggregated stdout/stderr text emitted so far.
    pub aggregated_output: String,
    /// Process exit code when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Current command lifecycle status.
    pub status: CommandExecutionStatus,
}

/// File patch change kind.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PatchChangeKind {
    /// File was added.
    Add,
    /// File was deleted.
    Delete,
    /// File was modified in place.
    Update,
}

/// One changed file within a patch item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileUpdateChange {
    /// File path relative to the thread working directory.
    pub path: String,
    /// Type of change applied to this file.
    pub kind: PatchChangeKind,
}

/// Patch application lifecycle status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PatchApplyStatus {
    /// Patch is still being applied (emitted by codex CLI ≥ ~0.141; absent in the 0.107 upstream
    /// enum, which is why an unpatched SDK fails worker runs with `unknown variant 'in_progress'`).
    InProgress,
    /// Patch application completed.
    Completed,
    /// Patch application failed.
    Failed,
}

/// File change item containing patch metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileChangeItem {
    /// Unique item id.
    pub id: String,
    /// Files affected by the patch.
    pub changes: Vec<FileUpdateChange>,
    /// Patch apply status.
    pub status: PatchApplyStatus,
}

/// Lifecycle status for MCP tool call items.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpToolCallStatus {
    /// MCP tool call is still running.
    InProgress,
    /// MCP tool call completed successfully.
    Completed,
    /// MCP tool call failed.
    Failed,
}

/// Successful MCP tool call result payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpToolCallResult {
    /// Content blocks returned by MCP tool execution.
    pub content: Vec<Value>,
    /// Structured result payload returned by MCP tool execution.
    pub structured_content: Value,
}

/// Failed MCP tool call payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolCallError {
    /// Human-readable error message.
    pub message: String,
}

/// MCP tool call item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpToolCallItem {
    /// Unique item id.
    pub id: String,
    /// MCP server name.
    pub server: String,
    /// MCP tool name.
    pub tool: String,
    /// JSON arguments passed to the tool.
    pub arguments: Value,
    /// Successful result payload when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<McpToolCallResult>,
    /// Error payload when the tool call fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpToolCallError>,
    /// Current MCP tool call status.
    pub status: McpToolCallStatus,
}

/// Final assistant text response item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMessageItem {
    /// Unique item id.
    pub id: String,
    /// Assistant message text.
    pub text: String,
}

/// Model reasoning item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReasoningItem {
    /// Unique item id.
    pub id: String,
    /// Reasoning content text.
    pub text: String,
}

/// Web search item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchItem {
    /// Unique item id.
    pub id: String,
    /// Search query string.
    pub query: String,
}

/// Item representing an error generated inside the turn flow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorItem {
    /// Unique item id.
    pub id: String,
    /// Error message.
    pub message: String,
}

/// One todo entry tracked by the agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoItem {
    /// Todo text content.
    pub text: String,
    /// Whether this todo has been completed.
    pub completed: bool,
}

/// Todo list item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoListItem {
    /// Unique item id.
    pub id: String,
    /// Todo entries tracked in this list.
    pub items: Vec<TodoItem>,
}

/// Canonical union of thread items and their type-specific payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThreadItem {
    /// Assistant message payload.
    AgentMessage(AgentMessageItem),
    /// Reasoning payload.
    Reasoning(ReasoningItem),
    /// Command execution payload.
    CommandExecution(CommandExecutionItem),
    /// File change payload.
    FileChange(FileChangeItem),
    /// MCP tool call payload.
    McpToolCall(McpToolCallItem),
    /// Web search payload.
    WebSearch(WebSearchItem),
    /// Todo list payload.
    TodoList(TodoListItem),
    /// Error payload.
    Error(ErrorItem),
}
