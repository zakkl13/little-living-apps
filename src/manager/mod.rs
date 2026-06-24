//! The manager tier: the backend seam, the turn driver, the static/volatile prompts, and (later)
//! the app composition root + the Lila MCP server.

pub mod app;
pub mod backend;
pub mod claude;
pub mod codex;
pub mod driver;
pub mod mcp;
pub mod prompt;
pub mod real;
pub mod settings;

// The scripted fake backend is always compiled but inert unless `LILA_FAKE_BACKEND` is set at
// runtime — it is the seam the binary-driven integration tests drive.
pub mod fake_backend;

pub use backend::{BackendError, BackendEvent, ManagerBackend, ManagerThread, TurnInput};
pub use driver::{ManagerDriver, TurnOutcome};
pub use prompt::{RuntimeFacts, build_agents_md, build_context_header};
