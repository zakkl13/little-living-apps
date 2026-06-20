//! Scripted worker runner for tests and the `testing`-feature binary.

use async_trait::async_trait;

use super::runner::{LoginStatus, RunArgs, RunOutcome, Runner, RunnerError};
use crate::runtime::TokenUsage;

/// A runner that returns a canned summary (or a scripted failure).
#[derive(Debug, Clone)]
pub struct FakeRunner {
    pub summary: String,
    pub ok: bool,
}

impl Default for FakeRunner {
    fn default() -> Self {
        Self {
            summary: "### SUMMARY FOR MANAGER\nPASS — fake worker did the thing".into(),
            ok: true,
        }
    }
}

impl FakeRunner {
    /// Build from env (`LILA_FAKE_WORKER_SUMMARY`, `LILA_FAKE_WORKER_OK`).
    pub fn from_env() -> Self {
        let summary = std::env::var("LILA_FAKE_WORKER_SUMMARY")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| Self::default().summary);
        let ok = std::env::var("LILA_FAKE_WORKER_OK")
            .map(|v| v != "0")
            .unwrap_or(true);
        Self { summary, ok }
    }
}

#[async_trait]
impl Runner for FakeRunner {
    async fn run(&self, _args: RunArgs) -> Result<RunOutcome, RunnerError> {
        Ok(RunOutcome {
            ok: self.ok,
            final_response: self.summary.clone(),
            thread_id: Some("fake-thread".into()),
            // A fixed nonzero usage so the worker-token accounting path is exercised hermetically.
            usage: TokenUsage {
                input_tokens: 1_000,
                output_tokens: 100,
                ..Default::default()
            },
        })
    }
    async fn login_status(&self) -> LoginStatus {
        LoginStatus {
            ok: true,
            detail: "fake runner".into(),
        }
    }
}
