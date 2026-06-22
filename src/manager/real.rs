//! Wire the real manager backend: seed the standing-rules files (worker rules as BOTH `AGENTS.md`
//! for Codex and `CLAUDE.md` for Claude Code), start the Lila MCP server, and build the
//! Codex/Claude backend pointed at it.

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::config::{AgentBackend, Config};
use crate::manager::backend::ManagerBackend;
use crate::manager::claude::ClaudeBackend;
use crate::manager::codex::CodexBackend;
use crate::manager::mcp::{self, RunningMcp};
use crate::manager::prompt::{RuntimeFacts, build_agents_md};
use crate::memory::MemFs;
use crate::workers::{Orchestrator, WORKER_AGENTS_MD};

/// Build the real backend + the running MCP server it talks to (caller keeps the handle alive).
pub async fn build_backend(
    cfg: &Config,
    mem: &Arc<Mutex<MemFs>>,
    orch: &Arc<Orchestrator>,
) -> anyhow::Result<(Box<dyn ManagerBackend>, RunningMcp)> {
    let agents_md = seed_agents_md(cfg)?;

    let token = cfg
        .lila_mcp_token
        .clone()
        .unwrap_or_else(|| format!("lila-{}", uuid::Uuid::new_v4().simple()));
    let port = cfg.lila_mcp_port.unwrap_or(0);
    let mcp = mcp::start(mem.clone(), orch.clone(), token.clone(), port).await?;

    let backend: Box<dyn ManagerBackend> = match cfg.agent_backend {
        AgentBackend::Codex => Box::new(CodexBackend::new(cfg, &mcp.url, &token)?),
        AgentBackend::Claude => Box::new(ClaudeBackend::new(cfg, &mcp.url, &token, agents_md)),
    };
    Ok((backend, mcp))
}

/// Write the manager + worker standing rules to disk; return the manager AGENTS.md body (Claude
/// needs it as its system prompt).
fn seed_agents_md(cfg: &Config) -> anyhow::Result<String> {
    let runtime = RuntimeFacts {
        app_public_url: cfg.app_public_url.clone(),
        workspace_dir: cfg.workspace_dir.clone(),
        app_service_name: cfg.app_service_name.clone(),
    };
    let agents_md = build_agents_md(&runtime);
    std::fs::create_dir_all(&cfg.manager_dir)?;
    std::fs::write(
        Path::new(&cfg.manager_dir).join("AGENTS.md"),
        format!("{agents_md}\n"),
    )?;
    seed_worker_rules(Path::new(&cfg.workspace_dir))?;
    Ok(agents_md)
}

/// Seed the worker standing rules into the workspace under BOTH filenames the worker CLIs read:
/// Codex reads `AGENTS.md` natively, while Claude Code reads `CLAUDE.md`. Writing both keeps the
/// worker contract (summary block, browser self-validation, scope discipline) in force regardless
/// of the active backend — and across a `/backend` swap.
fn seed_worker_rules(workspace_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(workspace_dir)?;
    let body = format!("{WORKER_AGENTS_MD}\n");
    for name in ["AGENTS.md", "CLAUDE.md"] {
        std::fs::write(workspace_dir.join(name), &body)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_worker_rules_for_both_codex_and_claude() {
        let tmp = tempfile::tempdir().unwrap();
        seed_worker_rules(tmp.path()).unwrap();
        // Codex reads AGENTS.md; Claude Code reads CLAUDE.md — both must carry the worker contract.
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let body = std::fs::read_to_string(tmp.path().join(name)).unwrap();
            assert!(
                body.contains("SUMMARY FOR MANAGER"),
                "{name} missing the worker contract"
            );
        }
    }
}
