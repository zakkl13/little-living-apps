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
    /// Resolve / render the vendored design-system catalog. The scaffold draws a system and renders
    /// its tokens through this command so the draw/lock logic stays in one (tested) place.
    Design {
        #[command(subcommand)]
        action: DesignAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum DesignAction {
    /// Resolve `LILA_DESIGN` against the catalog and print the draw as `LILA_DESIGN_*` shell
    /// assignments (brand, the `DESIGN.md` path, pool, source, seed, commit): `eval "$(lila design
    /// draw random:7)"`. A blind `random` is bounded to the safe default pool.
    Draw {
        /// `random`, `random:<seed>`, or a `<brand>` pin.
        choice: String,
    },
    /// List catalog systems (brand · category · voice) for guided selection. Defaults to the
    /// `browsable` pool (what the design skill offers on request).
    List {
        /// `default`, `browsable` (the default), or `full`.
        pool: Option<String>,
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
