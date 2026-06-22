use serde::{Deserialize, Serialize};

/// Approval policy used by Codex tool execution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Never prompt and never escalate automatically.
    #[serde(rename = "never")]
    Never,
    /// Ask for approval only when explicitly requested by the model/tool.
    #[serde(rename = "on-request")]
    OnRequest,
    /// Ask for approval when a tool action fails and escalation is needed.
    #[serde(rename = "on-failure")]
    OnFailure,
    /// Treat operations as untrusted and require strict approval behavior.
    #[serde(rename = "untrusted")]
    Untrusted,
}

/// Filesystem sandbox level for a thread.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SandboxMode {
    /// Read-only sandbox.
    #[serde(rename = "read-only")]
    ReadOnly,
    /// Writable sandbox scoped to the workspace.
    #[serde(rename = "workspace-write")]
    WorkspaceWrite,
    /// Full filesystem access without sandbox restrictions.
    #[serde(rename = "danger-full-access")]
    DangerFullAccess,
}

/// Reasoning effort level requested from the model.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelReasoningEffort {
    /// Minimal reasoning.
    #[serde(rename = "minimal")]
    Minimal,
    /// Low reasoning effort.
    #[serde(rename = "low")]
    Low,
    /// Medium reasoning effort.
    #[serde(rename = "medium")]
    Medium,
    /// High reasoning effort.
    #[serde(rename = "high")]
    High,
    /// Extra-high reasoning effort.
    #[serde(rename = "xhigh")]
    XHigh,
}

/// Web search mode for a thread.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WebSearchMode {
    /// Disable web search.
    #[serde(rename = "disabled")]
    Disabled,
    /// Use cached web search results when available.
    #[serde(rename = "cached")]
    Cached,
    /// Enable live web search.
    #[serde(rename = "live")]
    Live,
}

/// Per-thread options passed to `codex exec`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadOptions {
    /// Model name used for turns in this thread.
    pub model: Option<String>,
    /// Filesystem sandbox mode.
    pub sandbox_mode: Option<SandboxMode>,
    /// Working directory used by Codex.
    pub working_directory: Option<String>,
    /// Whether to skip the Git repository check.
    pub skip_git_repo_check: Option<bool>,
    /// Model reasoning effort override.
    pub model_reasoning_effort: Option<ModelReasoningEffort>,
    /// Enables/disables network access within workspace-write sandbox mode.
    pub network_access_enabled: Option<bool>,
    /// Preferred web search mode.
    pub web_search_mode: Option<WebSearchMode>,
    /// Legacy boolean web search toggle used when `web_search_mode` is unset.
    pub web_search_enabled: Option<bool>,
    /// Approval policy override.
    pub approval_policy: Option<ApprovalMode>,
    /// Additional directories to expose to the agent.
    pub additional_directories: Option<Vec<String>>,
}
