//! The Codex manager backend. Port of `src/manager/managerCodex.ts` onto `codex-client-sdk`.
//! Capability boundary — the manager's "no hands": shell off, web off, read-only sandbox, network
//! off. Its ONLY tools are the Lila MCP server's. Auth rides the cached ChatGPT-subscription login;
//! billing-flip keys are stripped from the CLI env.

use async_trait::async_trait;
use codex::{
    ApprovalMode, Codex, CodexOptions, Input, ModelReasoningEffort, SandboxMode as CxSandbox,
    ThreadEvent, ThreadItem, ThreadOptions, UserInput, WebSearchMode,
};
use futures::StreamExt;
use serde_json::json;

use super::backend::{BackendError, BackendEvent, ManagerBackend, ManagerThread, TurnInput};
use crate::config::{Config, ReasoningEffort, sanitized_env};
use crate::runtime::TokenUsage;
use crate::workers::runner::friendly_error;

const MCP_TOKEN_ENV: &str = "LILA_MCP_TOKEN";

/// Map our reasoning effort onto the Codex SDK's.
pub fn to_effort(e: ReasoningEffort) -> ModelReasoningEffort {
    match e {
        ReasoningEffort::Minimal => ModelReasoningEffort::Minimal,
        ReasoningEffort::Low => ModelReasoningEffort::Low,
        ReasoningEffort::Medium => ModelReasoningEffort::Medium,
        ReasoningEffort::High => ModelReasoningEffort::High,
        ReasoningEffort::XHigh => ModelReasoningEffort::XHigh,
    }
}

/// Map our sandbox mode onto the Codex SDK's.
pub fn to_sandbox(s: crate::config::SandboxMode) -> CxSandbox {
    match s {
        crate::config::SandboxMode::ReadOnly => CxSandbox::ReadOnly,
        crate::config::SandboxMode::WorkspaceWrite => CxSandbox::WorkspaceWrite,
        crate::config::SandboxMode::DangerFullAccess => CxSandbox::DangerFullAccess,
    }
}

/// The manager's "no hands" Codex config: attach ONLY the Lila MCP server.
fn manager_config(mcp_url: &str) -> serde_json::Map<String, serde_json::Value> {
    let v = json!({
        "features": { "shell_tool": false },
        "tools": { "web_search": false, "view_image": true },
        "web_search": "disabled",
        "mcp_servers": {
            "lila": {
                "url": mcp_url,
                "bearer_token_env_var": MCP_TOKEN_ENV,
                "default_tools_approval_mode": "approve",
            }
        }
    });
    match v {
        serde_json::Value::Object(map) => map,
        _ => unreachable!("object literal"),
    }
}

/// The Codex manager backend.
pub struct CodexBackend {
    codex: Codex,
    thread_options: ThreadOptions,
}

impl CodexBackend {
    pub fn new(cfg: &Config, mcp_url: &str, mcp_token: &str) -> anyhow::Result<Self> {
        let options = CodexOptions {
            env: Some(sanitized_env(&[(MCP_TOKEN_ENV, mcp_token)])),
            config: Some(manager_config(mcp_url)),
            codex_path_override: cfg.codex_path_override.clone(),
            ..Default::default()
        };
        let codex = Codex::new(Some(options)).map_err(|e| anyhow::anyhow!("codex init: {e}"))?;
        let thread_options = ThreadOptions {
            model: cfg.manager_model.clone(),
            sandbox_mode: Some(CxSandbox::ReadOnly),
            working_directory: Some(cfg.manager_dir.clone()),
            skip_git_repo_check: Some(true),
            model_reasoning_effort: Some(to_effort(cfg.manager_reasoning_effort)),
            network_access_enabled: Some(false),
            web_search_mode: Some(WebSearchMode::Disabled),
            web_search_enabled: Some(false),
            approval_policy: Some(ApprovalMode::Never),
            additional_directories: None,
        };
        Ok(Self {
            codex,
            thread_options,
        })
    }
}

impl ManagerBackend for CodexBackend {
    fn thread(&self, resume: Option<String>) -> Box<dyn ManagerThread> {
        let opts = self.thread_options.clone();
        let thread = match resume {
            Some(id) => self.codex.resume_thread(id, Some(opts)),
            None => self.codex.start_thread(Some(opts)),
        };
        Box::new(CodexThread { thread })
    }

    fn format_error(&self, detail: &str) -> String {
        friendly_error(detail)
    }
}

struct CodexThread {
    thread: codex::Thread,
}

#[async_trait]
impl ManagerThread for CodexThread {
    fn session_id(&self) -> Option<String> {
        self.thread.id()
    }

    async fn run_turn(
        &mut self,
        input: TurnInput,
        on_event: &mut (dyn FnMut(BackendEvent) + Send),
    ) -> Result<(), BackendError> {
        let cx_input = build_input(input);
        let mut events = self
            .thread
            .run_streamed(cx_input, None)
            .await
            .map_err(|e| BackendError::Run(e.to_string()))?
            .events;
        while let Some(event) = events.next().await {
            match event {
                Ok(ev) => emit(ev, on_event),
                Err(e) => on_event(BackendEvent::Failed(e.to_string())),
            }
        }
        Ok(())
    }
}

fn build_input(input: TurnInput) -> Input {
    match input.image_path {
        Some(path) => Input::Entries(vec![
            UserInput::Text { text: input.text },
            UserInput::LocalImage { path: path.into() },
        ]),
        None => Input::Text(input.text),
    }
}

fn emit(event: ThreadEvent, on_event: &mut (dyn FnMut(BackendEvent) + Send)) {
    match event {
        ThreadEvent::ItemCompleted { item } => emit_item(item, on_event),
        ThreadEvent::TurnCompleted { usage } => on_event(BackendEvent::Usage(TokenUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            reasoning_tokens: 0,
        })),
        ThreadEvent::TurnFailed { error } => on_event(BackendEvent::Failed(error.message)),
        ThreadEvent::Error { message } => on_event(BackendEvent::Failed(message)),
        _ => {}
    }
}

fn emit_item(item: ThreadItem, on_event: &mut (dyn FnMut(BackendEvent) + Send)) {
    match item {
        ThreadItem::AgentMessage(m) => on_event(BackendEvent::AgentMessage(m.text)),
        ThreadItem::Reasoning(r) => on_event(BackendEvent::Reasoning(r.text)),
        ThreadItem::McpToolCall(c) => on_event(BackendEvent::ToolCall {
            server: c.server,
            tool: c.tool,
            status: format!("{:?}", c.status),
            error: c.error.map(|e| format!("{e:?}")),
        }),
        ThreadItem::Error(e) => on_event(BackendEvent::Failed(e.message)),
        _ => {}
    }
}
