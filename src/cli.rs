//! The `clap` command tree. The binary is CLI-first: `run` is the daemon, the rest are host
//! stand-up / day-2 ops commands that double as the integration-test surface.

use clap::{Parser, Subcommand};

/// little-living-apps agent (Rust).
#[derive(Debug, Parser)]
#[command(name = "lila", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the long-lived manager daemon (Telegram long-poll + serialized loop).
    Run,
    /// Validate the environment configuration (and the billing guard). Exits non-zero on error.
    ConfigCheck,
    /// Probe runtime health: config + backend CLI availability + auth.
    Doctor,
    /// Print the persisted runtime state (manager session, queue, usage) from the snapshot.
    Status,
    /// Show or persist the active agent backend.
    Backend {
        /// `codex` or `claude`. Omit to just show the current backend.
        backend: Option<String>,
    },
    /// Inspect the manager's memory store.
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },
    /// Run the Lila MCP server standalone (debugging).
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Print a stack profile's fields as shell assignments, so `bin/new-app` and `bootstrap.sh` can
    /// read the active stack (`stacks/<name>/stack.toml`) without parsing TOML in bash:
    /// `eval "$(lila stack rails-pwa)"`.
    Stack {
        /// The stack name (a directory under `stacks/`).
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum MemoryAction {
    /// Read a memory file or list a directory.
    View {
        /// A `/memories/...` path.
        path: String,
    },
    /// Full-text search across memory.
    Search {
        /// Query string.
        query: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum McpAction {
    /// Serve the Lila MCP server on a loopback port until interrupted.
    Serve,
}
