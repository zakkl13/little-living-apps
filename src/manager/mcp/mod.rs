//! The Lila MCP server — the manager's entire capability surface, exposed over loopback HTTP.

pub mod server;

pub use server::{RunningMcp, start};
