//! The Claude worker runner. Port of `src/workers/runnerClaude.ts`. Single-shot `query()` with the
//! full default toolset (workers DO real work — the disposable VM is the isolation boundary) and
//! `bypassPermissions` so the worker can act autonomously. Standing rules come from the workspace
//! `AGENTS.md`.

use async_trait::async_trait;
use claude_agent_sdk_rust::types::content::ContentBlock;
use claude_agent_sdk_rust::{ClaudeAgentOptions, Message, PermissionMode, query};
use futures::StreamExt;

use super::runner::{LoginStatus, RunArgs, RunOutcome, Runner, RunnerError, friendly_claude_error};
use crate::config::{Config, sanitized_env};
use crate::runtime::TokenUsage;

/// Runs single-shot Claude workers.
pub struct ClaudeRunner {
    model: Option<String>,
}

impl ClaudeRunner {
    pub fn new(cfg: &Config) -> Self {
        Self {
            model: cfg.worker_model.clone(),
        }
    }
}

#[async_trait]
impl Runner for ClaudeRunner {
    async fn run(&self, args: RunArgs) -> Result<RunOutcome, RunnerError> {
        let model = self
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-6".into());
        let options = ClaudeAgentOptions::builder()
            .cwd(args.cwd.clone())
            .permission_mode(PermissionMode::BypassPermissions)
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
fn parse_claude_usage(usage: Option<&serde_json::Value>) -> TokenUsage {
    let field = |u: &serde_json::Value, k: &str| u.get(k).and_then(serde_json::Value::as_u64);
    usage.map_or_else(TokenUsage::default, |u| TokenUsage {
        input_tokens: field(u, "input_tokens").unwrap_or(0),
        output_tokens: field(u, "output_tokens").unwrap_or(0),
        cached_input_tokens: field(u, "cache_read_input_tokens").unwrap_or(0),
        reasoning_tokens: 0,
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
