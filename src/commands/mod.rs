//! Thin subcommand handlers. Each parses args, builds owned state, and calls into the library —
//! no behavior lives only here. Returns an exit code so `main` can `process::exit`.

mod config_check;
mod doctor;
mod memory_cmd;
mod run;
mod status;

use crate::cli::{Command, McpAction, MemoryAction};

/// Dispatch a parsed command to its handler. Returns the process exit code.
pub async fn dispatch(command: Command) -> i32 {
    match command {
        Command::ConfigCheck => config_check::run(),
        Command::Doctor => doctor::run().await,
        Command::Status => status::run(),
        Command::Backend { backend } => backend_cmd(backend),
        Command::Run => run::run().await,
        Command::Memory { action } => memory_cmd(action),
        Command::Mcp { action } => match action {
            McpAction::Serve => {
                eprintln!("`lila mcp serve` is wired up in M3 (Lila MCP server).");
                1
            }
        },
    }
}

fn backend_cmd(_backend: Option<String>) -> i32 {
    eprintln!("`lila backend` is wired up in M8.");
    1
}

fn memory_cmd(action: MemoryAction) -> i32 {
    memory_cmd::run(action)
}
