//! The Codex worker runner. Port of `src/workers/runner.ts`. Workers are purely ephemeral: every
//! run is a FRESH Codex thread that runs one objective and is never resumed. Unlike the manager,
//! workers get the FULL toolset (shell/files/net) — the disposable VM is the isolation boundary —
//! and read their standing rules from the workspace `AGENTS.md`.

use async_trait::async_trait;
use codex::{ApprovalMode, Codex, CodexOptions, SandboxMode as CxSandbox, ThreadOptions};

use super::runner::{LoginStatus, RunArgs, RunOutcome, Runner, RunnerError, friendly_error};
use crate::config::{Config, sanitized_env};
use crate::manager::codex::to_sandbox;
use crate::runtime::TokenUsage;

/// Map the Codex SDK's per-turn usage onto our [`TokenUsage`] (Codex reports no reasoning split).
fn worker_usage(usage: Option<codex::Usage>) -> TokenUsage {
    usage.map_or_else(TokenUsage::default, |u| TokenUsage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        cached_input_tokens: u.cached_input_tokens,
        reasoning_tokens: 0,
    })
}

/// Runs single-shot Codex workers.
pub struct CodexRunner {
    codex: Codex,
    model: Option<String>,
    sandbox: CxSandbox,
}

impl CodexRunner {
    pub fn new(cfg: &Config) -> anyhow::Result<Self> {
        let options = CodexOptions {
            env: Some(sanitized_env(&[])),
            codex_path_override: cfg.codex_path_override.clone(),
            ..Default::default()
        };
        let codex = Codex::new(Some(options)).map_err(|e| anyhow::anyhow!("codex init: {e}"))?;
        Ok(Self {
            codex,
            model: cfg.worker_model.clone(),
            sandbox: to_sandbox(cfg.sandbox_mode),
        })
    }
}

#[async_trait]
impl Runner for CodexRunner {
    async fn run(&self, args: RunArgs) -> Result<RunOutcome, RunnerError> {
        let opts = ThreadOptions {
            model: self.model.clone(),
            sandbox_mode: Some(self.sandbox),
            working_directory: Some(args.cwd.to_string_lossy().into_owned()),
            skip_git_repo_check: Some(true),
            network_access_enabled: Some(true),
            approval_policy: Some(ApprovalMode::Never),
            ..Default::default()
        };
        let thread = self.codex.start_thread(Some(opts));
        match thread.run(args.prompt, None).await {
            Ok(turn) => Ok(RunOutcome {
                ok: true,
                final_response: turn.final_response,
                thread_id: thread.id(),
                usage: worker_usage(turn.usage),
            }),
            // Surface the failure as the worker's summary (friendly wording) rather than an opaque err.
            Err(e) => Ok(RunOutcome {
                ok: false,
                final_response: friendly_error(&e.to_string()),
                thread_id: thread.id(),
                usage: TokenUsage::default(),
            }),
        }
    }

    async fn login_status(&self) -> LoginStatus {
        LoginStatus {
            ok: true,
            detail: "codex (validated on first run)".into(),
        }
    }
}
