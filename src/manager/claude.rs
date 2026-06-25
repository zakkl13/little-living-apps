//! The Claude manager backend, on `claude-agent-sdk-rust`.
//! Capability boundary — the manager's "no hands": built-in tools off, only the Lila MCP server's
//! tools allowed (http + bearer). `setting_sources: []` isolates it from host settings. Auth rides
//! the cached Claude Pro/Max subscription (no ANTHROPIC_API_KEY — stripped from the env).

use std::collections::HashMap;

use async_trait::async_trait;
use claude_agent_sdk_rust::types::content::ContentBlock;
use claude_agent_sdk_rust::types::mcp::{McpHttpConfig, McpServerConfig};
use claude_agent_sdk_rust::{
    ClaudeAgentOptions, ClaudeSDKClient, Effort, Message, SettingSource, SystemPrompt,
};
use futures::StreamExt;
use std::path::PathBuf;

use super::backend::{BackendError, BackendEvent, ManagerBackend, ManagerThread, TurnInput};
use crate::backend_cli::resolve_backend_cli_path;
use crate::config::AgentBackend;
use crate::config::{Config, ReasoningEffort, sanitized_env};
use crate::runtime::TokenUsage;
use crate::workers::runner::friendly_claude_error;

const LILA_TOOLS: &[&str] = &[
    "memory_view",
    "memory_create",
    "memory_str_replace",
    "memory_insert",
    "memory_delete",
    "memory_rename",
    "memory_search",
    "recall_search",
    "subagent_start",
    "settings_get",
    "settings_set",
];

/// Map our reasoning effort onto the SDK's `Effort` (xhigh has no analog → Max).
pub fn to_effort(e: ReasoningEffort) -> Effort {
    match e {
        ReasoningEffort::Minimal | ReasoningEffort::Low => Effort::Low,
        ReasoningEffort::Medium => Effort::Medium,
        ReasoningEffort::High => Effort::High,
        ReasoningEffort::XHigh => Effort::Max,
    }
}

/// The Claude manager backend.
pub struct ClaudeBackend {
    model: String,
    effort: Effort,
    system_prompt: String,
    mcp_url: String,
    mcp_token: String,
    cli_path: PathBuf,
}

impl ClaudeBackend {
    pub fn new(
        cfg: &Config,
        mcp_url: &str,
        mcp_token: &str,
        system_prompt: String,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            model: cfg
                .manager_model
                .clone()
                .unwrap_or_else(|| "claude-opus-4-8".into()),
            effort: to_effort(cfg.manager_reasoning_effort),
            system_prompt,
            mcp_url: mcp_url.to_string(),
            mcp_token: mcp_token.to_string(),
            cli_path: resolve_backend_cli_path(cfg, AgentBackend::Claude)
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        })
    }

    fn options(&self, resume: Option<String>) -> ClaudeAgentOptions {
        let mut servers = HashMap::new();
        servers.insert(
            "lila".to_string(),
            McpServerConfig::Http(McpHttpConfig {
                url: self.mcp_url.clone(),
                headers: Some(HashMap::from([(
                    "Authorization".to_string(),
                    format!("Bearer {}", self.mcp_token),
                )])),
            }),
        );
        let allowed: Vec<String> = LILA_TOOLS
            .iter()
            .map(|t| format!("mcp__lila__{t}"))
            .collect();
        ClaudeAgentOptions::builder()
            .system_prompt(SystemPrompt::Text(self.system_prompt.clone()))
            .model(self.model.clone())
            .effort(self.effort)
            .allowed_tools(allowed)
            .mcp_servers(servers)
            .cli_path(self.cli_path.clone())
            .setting_sources(Vec::<SettingSource>::new())
            .env(sanitized_env(&[]))
            .include_partial_messages(false)
            .resume(resume)
            .build()
    }
}

impl ManagerBackend for ClaudeBackend {
    fn thread(&self, resume: Option<String>) -> Box<dyn ManagerThread> {
        Box::new(ClaudeThread {
            client: ClaudeSDKClient::new(self.options(resume)),
            connected: false,
            session: None,
        })
    }

    fn format_error(&self, detail: &str) -> String {
        friendly_claude_error(detail)
    }
}

struct ClaudeThread {
    client: ClaudeSDKClient,
    connected: bool,
    session: Option<String>,
}

#[async_trait]
impl ManagerThread for ClaudeThread {
    fn session_id(&self) -> Option<String> {
        self.session.clone()
    }

    async fn run_turn(
        &mut self,
        input: TurnInput,
        on_event: &mut (dyn FnMut(BackendEvent) + Send),
    ) -> Result<(), BackendError> {
        self.ensure_connected().await?;
        self.client
            .query(input.text)
            .await
            .map_err(|e| BackendError::Run(e.to_string()))?;

        let stream = self
            .client
            .receive_response()
            .map_err(|e| BackendError::Run(e.to_string()))?;
        drain_response(stream, on_event).await;
        self.session = self.client.get_session_id();
        Ok(())
    }
}

impl ClaudeThread {
    /// Connect the client once (the session continues across turns thereafter).
    async fn ensure_connected(&mut self) -> Result<(), BackendError> {
        if !self.connected {
            self.client
                .connect(None)
                .await
                .map_err(|e| BackendError::Run(e.to_string()))?;
            self.connected = true;
        }
        Ok(())
    }
}

/// Drain the response stream, forwarding assistant messages, tool calls, reasoning, and usage.
async fn drain_response(
    stream: impl futures::Stream<Item = Result<Message, claude_agent_sdk_rust::ClaudeSDKError>>,
    on_event: &mut (dyn FnMut(BackendEvent) + Send),
) {
    tokio::pin!(stream);
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(message) => collect_message(message, on_event),
            Err(e) => on_event(BackendEvent::Failed(e.to_string())),
        }
    }
}

fn collect_message(message: Message, on_event: &mut (dyn FnMut(BackendEvent) + Send)) {
    match message {
        Message::Assistant(am) => collect_assistant(am, on_event),
        Message::Result(rm) => collect_result(rm, on_event),
        _ => {}
    }
}

fn collect_assistant(
    am: claude_agent_sdk_rust::AssistantMessage,
    on_event: &mut (dyn FnMut(BackendEvent) + Send),
) {
    if let Some(err) = am.error {
        on_event(BackendEvent::Failed(err));
    }
    let mut text_blocks = Vec::new();
    for block in am.message.content {
        match block {
            ContentBlock::Text(t) => text_blocks.push(t.text),
            ContentBlock::Thinking(th) => on_event(BackendEvent::Reasoning(th.thinking)),
            ContentBlock::ToolUse(tu) => on_event(BackendEvent::ToolCall {
                server: "lila".into(),
                tool: tu.name,
                status: "completed".into(),
                error: None,
            }),
            ContentBlock::ToolResult(_) => {}
        }
    }
    let text = text_blocks
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if !text.trim().is_empty() {
        on_event(BackendEvent::AgentMessage(text));
    }
}

fn collect_result(
    rm: claude_agent_sdk_rust::ResultMessage,
    on_event: &mut (dyn FnMut(BackendEvent) + Send),
) {
    if let Some(usage) = rm.usage {
        on_event(BackendEvent::Usage(parse_usage(&usage)));
    }
    if rm.is_error {
        on_event(BackendEvent::Failed(
            rm.result.unwrap_or_else(|| "Claude turn failed".into()),
        ));
    }
}

/// Parse Claude's `usage` JSON into [`TokenUsage`], normalizing the token basis to match Codex.
///
/// Anthropic reports `input_tokens` as FRESH (uncached) input only, with cache reads/creation in
/// separate buckets; Codex/OpenAI report `input_tokens` as the GROSS prompt total (cache included).
/// We fold cache back into `input_tokens` so both backends measure gross context processed and the
/// telemetry invariant "cached ⊆ input" holds for Claude as it does for Codex.
fn parse_usage(usage: &serde_json::Value) -> TokenUsage {
    let get = |k: &str| {
        usage
            .get(k)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let cache_read = get("cache_read_input_tokens");
    TokenUsage {
        input_tokens: get("input_tokens") + cache_read + get("cache_creation_input_tokens"),
        output_tokens: get("output_tokens"),
        cached_input_tokens: cache_read,
        reasoning_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_usage_folds_cache_into_input() {
        let usage = json!({
            "input_tokens": 1_500,
            "cache_read_input_tokens": 90_000,
            "cache_creation_input_tokens": 8_500,
            "output_tokens": 4_000,
        });
        let got = parse_usage(&usage);
        assert_eq!(
            got.input_tokens, 100_000,
            "fresh + cache_read + cache_creation"
        );
        assert_eq!(got.cached_input_tokens, 90_000);
        assert_eq!(got.output_tokens, 4_000);
    }

    #[test]
    fn emits_assistant_text_per_claude_message() {
        use claude_agent_sdk_rust::types::content::TextBlock;
        use claude_agent_sdk_rust::types::messages::AssistantMessageInner;

        fn msg(text: &str) -> claude_agent_sdk_rust::AssistantMessage {
            claude_agent_sdk_rust::AssistantMessage {
                message: AssistantMessageInner {
                    content: vec![ContentBlock::Text(TextBlock {
                        text: text.to_string(),
                    })],
                    id: None,
                    model: "claude-test".into(),
                    role: Some("assistant".into()),
                    stop_reason: None,
                    stop_sequence: None,
                    message_type: None,
                    usage: None,
                },
                parent_tool_use_id: None,
                session_id: None,
                error: None,
            }
        }

        let mut events = Vec::new();
        collect_assistant(msg("Got it."), &mut |ev| events.push(ev));
        collect_assistant(msg("NO_REPLY"), &mut |ev| events.push(ev));
        let texts = events
            .into_iter()
            .filter_map(|ev| match ev {
                BackendEvent::AgentMessage(text) => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(texts, vec!["Got it.", "NO_REPLY"]);
    }
}
