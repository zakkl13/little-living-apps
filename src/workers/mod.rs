//! The worker tier: ephemeral single-shot subagents driven by the orchestrator.

pub mod agents;
pub mod claude_runner;
pub mod codex_runner;
pub mod orchestrator;
pub mod protocol;
pub mod real;
pub mod runner;

// Always compiled, inert unless `LILA_FAKE_BACKEND` is set (the integration-test seam).
pub mod fake_runner;

pub use agents::WORKER_AGENTS_MD;
pub use orchestrator::Orchestrator;
pub use runner::{LoginStatus, RunArgs, RunOutcome, Runner, RunnerError};
