use std::collections::HashMap;

use serde_json::{Map, Value};

/// JSON value accepted by `CodexOptions::config`.
pub type CodexConfigValue = Value;
/// Root object type accepted by `CodexOptions::config`.
pub type CodexConfigObject = Map<String, Value>;

/// Options for constructing a [`crate::Codex`] client.
#[derive(Debug, Clone, Default)]
pub struct CodexOptions {
    /// Absolute path to the `codex` executable.
    ///
    /// When omitted, the SDK searches `PATH`, local `node_modules`, vendor
    /// binaries, and common global install locations.
    pub codex_path_override: Option<String>,
    /// Overrides `OPENAI_BASE_URL` for Codex CLI requests.
    pub base_url: Option<String>,
    /// Overrides `CODEX_API_KEY` for Codex CLI requests.
    pub api_key: Option<String>,
    /// Additional `--config key=value` overrides for the Codex CLI.
    pub config: Option<CodexConfigObject>,
    /// Environment variables passed to the Codex CLI process.
    ///
    /// When provided, the SDK does not inherit `std::env`.
    pub env: Option<HashMap<String, String>>,
}
