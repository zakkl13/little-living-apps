//! In-process scripted backend for tests and the `testing`-feature binary. Stands in for "model +
//! MCP", so the loop/queue/app/transport run end-to-end without a real codex/claude subscription.

use async_trait::async_trait;

use super::backend::{BackendError, BackendEvent, ManagerBackend, ManagerThread, TurnInput};
use crate::runtime::TokenUsage;

/// Deterministic scripted behavior.
#[derive(Debug, Clone)]
pub struct FakeBackend {
    /// Reply emitted for an owner message.
    pub reply: String,
    /// Reply emitted for a worker event (default `NO_REPLY` — silently folded).
    pub worker_reply: String,
    /// If the input contains this substring, the turn fails with it.
    pub fail_on: Option<String>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self {
            reply: "ack".into(),
            worker_reply: "NO_REPLY".into(),
            fail_on: None,
        }
    }
}

impl FakeBackend {
    /// Build a fake from env (used by the `testing`-feature binary):
    /// `LILA_FAKE_REPLY`, `LILA_FAKE_WORKER_REPLY`, `LILA_FAKE_FAIL_ON`.
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            reply: get("LILA_FAKE_REPLY").unwrap_or_else(|| "ack".into()),
            worker_reply: get("LILA_FAKE_WORKER_REPLY").unwrap_or_else(|| "NO_REPLY".into()),
            fail_on: get("LILA_FAKE_FAIL_ON"),
        }
    }
}

impl ManagerBackend for FakeBackend {
    fn thread(&self, resume: Option<String>) -> Box<dyn ManagerThread> {
        Box::new(FakeThread {
            cfg: self.clone(),
            session: resume.or_else(|| Some("fake-session-1".into())),
        })
    }

    fn format_error(&self, detail: &str) -> String {
        format!("⚠️ {detail}")
    }
}

struct FakeThread {
    cfg: FakeBackend,
    session: Option<String>,
}

#[async_trait]
impl ManagerThread for FakeThread {
    fn session_id(&self) -> Option<String> {
        self.session.clone()
    }

    async fn run_turn(
        &mut self,
        input: TurnInput,
        on_event: &mut (dyn FnMut(BackendEvent) + Send),
    ) -> Result<(), BackendError> {
        if let Some(marker) = &self.cfg.fail_on
            && input.text.contains(marker.as_str())
        {
            on_event(BackendEvent::Failed(format!("scripted failure: {marker}")));
            return Ok(());
        }
        on_event(BackendEvent::Usage(TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        }));
        // A rendered worker event leads with "[subagent …" (see app.rs); reply quietly to those.
        let is_worker_event = input.text.contains("[subagent ");
        let reply = if is_worker_event {
            &self.cfg.worker_reply
        } else {
            &self.cfg.reply
        };
        on_event(BackendEvent::AgentMessage(reply.clone()));
        Ok(())
    }
}
