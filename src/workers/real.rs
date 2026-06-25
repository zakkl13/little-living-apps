//! Dispatch to the real worker runner for the active backend.

use std::sync::Arc;

use crate::config::{AgentBackend, Config};
use crate::workers::Runner;
use crate::workers::claude_runner::ClaudeRunner;
use crate::workers::codex_runner::CodexRunner;

pub fn build_runner(cfg: &Config) -> anyhow::Result<Arc<dyn Runner>> {
    match cfg.agent_backend {
        AgentBackend::Codex => Ok(Arc::new(CodexRunner::new(cfg)?)),
        AgentBackend::Claude => Ok(Arc::new(ClaudeRunner::new(cfg)?)),
    }
}
