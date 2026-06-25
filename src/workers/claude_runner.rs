//! The Claude worker runner. Single-shot `query()` with the
//! full default toolset (workers DO real work — the disposable VM is the isolation boundary) and
//! `bypassPermissions` so the worker can act autonomously. Standing rules come from the workspace
//! `AGENTS.md`.

use async_trait::async_trait;
use claude_agent_sdk_rust::types::content::ContentBlock;
use claude_agent_sdk_rust::{ClaudeAgentOptions, Message, PermissionMode, query};
use futures::StreamExt;
use std::path::PathBuf;

use super::runner::{LoginStatus, RunArgs, RunOutcome, Runner, RunnerError, friendly_claude_error};
use crate::backend_cli::resolve_backend_cli_path;
use crate::config::AgentBackend;
use crate::config::{Config, sanitized_env};
use crate::runtime::TokenUsage;

/// Runs single-shot Claude workers.
pub struct ClaudeRunner {
    model: Option<String>,
    cli_path: PathBuf,
}

impl ClaudeRunner {
    pub fn new(cfg: &Config) -> anyhow::Result<Self> {
        Ok(Self {
            model: cfg.worker_model.clone(),
            cli_path: resolve_backend_cli_path(cfg, AgentBackend::Claude)
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        })
    }
}

#[async_trait]
impl Runner for ClaudeRunner {
    async fn run(&self, args: RunArgs) -> Result<RunOutcome, RunnerError> {
        if !args.cwd.is_dir() {
            return Ok(RunOutcome {
                ok: false,
                final_response: format!(
                    "⚠️ The agent run failed: worker cwd does not exist: {}",
                    args.cwd.display()
                ),
                thread_id: None,
                usage: TokenUsage::default(),
            });
        }
        let model = self
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-6".into());
        let options = ClaudeAgentOptions::builder()
            .cwd(args.cwd.clone())
            .permission_mode(PermissionMode::BypassPermissions)
            .cli_path(self.cli_path.clone())
            .env(sanitized_env(&[]))
            .include_partial_messages(false)
            .model(model)
            .build();

        let stream = match query(args.prompt, Some(options)).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(RunOutcome {
                    ok: false,
                    final_response: friendly_claude_error(&e.to_string()),
                    thread_id: None,
                    usage: TokenUsage::default(),
                });
            }
        };
        tokio::pin!(stream);
        let mut acc = WorkerRun::default();
        while let Some(msg) = stream.next().await {
            acc.absorb(msg);
        }
        Ok(RunOutcome {
            ok: acc.ok,
            final_response: acc.text,
            thread_id: acc.session,
            usage: acc.usage,
        })
    }

    async fn login_status(&self) -> LoginStatus {
        LoginStatus {
            ok: true,
            detail: "claude (validated on first run)".into(),
        }
    }
}

/// Accumulates a worker's streamed messages into a final outcome.
struct WorkerRun {
    text: String,
    ok: bool,
    session: Option<String>,
    usage: TokenUsage,
}

impl Default for WorkerRun {
    fn default() -> Self {
        Self {
            text: String::new(),
            ok: true,
            session: None,
            usage: TokenUsage::default(),
        }
    }
}

/// Parse Claude's loosely-typed `usage` JSON blob (input/output/cache fields) into [`TokenUsage`].
/// Claude reports no separate reasoning split, so that stays zero.
///
/// Basis normalization: Anthropic reports `input_tokens` as FRESH (uncached) input only, with cache
/// reads/creation in separate buckets. Codex/OpenAI report `input_tokens` as the GROSS prompt total
/// (cache included, with `cached_input_tokens` a subset within it). We fold cache back into
/// `input_tokens` here so both backends' `input_tokens` mean the same thing — gross context
/// processed — and the telemetry invariant "cached ⊆ input" holds for Claude as it does for Codex.
fn parse_claude_usage(usage: Option<&serde_json::Value>) -> TokenUsage {
    let field = |u: &serde_json::Value, k: &str| u.get(k).and_then(serde_json::Value::as_u64);
    usage.map_or_else(TokenUsage::default, |u| {
        let fresh_input = field(u, "input_tokens").unwrap_or(0);
        let cache_read = field(u, "cache_read_input_tokens").unwrap_or(0);
        let cache_creation = field(u, "cache_creation_input_tokens").unwrap_or(0);
        TokenUsage {
            input_tokens: fresh_input + cache_read + cache_creation,
            output_tokens: field(u, "output_tokens").unwrap_or(0),
            cached_input_tokens: cache_read,
            reasoning_tokens: 0,
        }
    })
}

impl WorkerRun {
    fn absorb(&mut self, msg: Result<Message, claude_agent_sdk_rust::ClaudeSDKError>) {
        match msg {
            Ok(Message::Assistant(am)) => self.absorb_assistant(am),
            Ok(Message::Result(rm)) => self.absorb_result(rm),
            Ok(_) => {}
            Err(e) => {
                self.ok = false;
                self.text = friendly_claude_error(&e.to_string());
            }
        }
    }

    fn absorb_assistant(&mut self, am: claude_agent_sdk_rust::AssistantMessage) {
        for block in am.message.content {
            if let ContentBlock::Text(t) = block {
                if !self.text.is_empty() && !self.text.ends_with('\n') {
                    self.text.push('\n');
                }
                self.text.push_str(&t.text);
            }
        }
    }

    fn absorb_result(&mut self, rm: claude_agent_sdk_rust::ResultMessage) {
        self.ok = !rm.is_error;
        self.session = Some(rm.session_id);
        self.usage = parse_claude_usage(rm.usage.as_ref());
        if let Some(result) = rm.result
            && !result.trim().is_empty()
        {
            self.text = result;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn folds_cache_into_input_for_codex_parity() {
        // Anthropic splits cache out of input_tokens; we fold it back so input_tokens is the gross
        // context processed (matching Codex), keeping cached a subset of input.
        let usage = json!({
            "input_tokens": 2_000,
            "cache_read_input_tokens": 400_000,
            "cache_creation_input_tokens": 18_000,
            "output_tokens": 6_000,
        });
        let got = parse_claude_usage(Some(&usage));
        assert_eq!(
            got.input_tokens, 420_000,
            "fresh + cache_read + cache_creation"
        );
        assert_eq!(
            got.cached_input_tokens, 400_000,
            "cached is the read subset only"
        );
        assert_eq!(got.output_tokens, 6_000);
        // Billable total now reflects gross context, comparable to a Codex worker on the same task.
        assert_eq!(got.total(), 426_000);
    }

    #[test]
    fn missing_cache_fields_default_to_zero() {
        let usage = json!({ "input_tokens": 100, "output_tokens": 20 });
        let got = parse_claude_usage(Some(&usage));
        assert_eq!(got.input_tokens, 100);
        assert_eq!(got.cached_input_tokens, 0);
    }
}
