//! Passive observability recorder (read by the Inspector / `lila status`). Port of
//! `src/runtime/telemetry.ts`. Records TOKEN USAGE only — everything rides one subscription, so there
//! is no metered-dollar plane. The cumulative usage meter is the durable part (folded into the
//! crash snapshot); turn/prompt logs are bounded in-memory ring buffers.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::trace::TraceBlock;

/// The token counters reported per turn. `cached`/`reasoning` default to 0 at partial call sites.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub reasoning_tokens: u64,
}

impl TokenUsage {
    /// Billable tokens for the turn (cached input is a subset of input, so it is not re-added —
    /// this matches the TS eval's `meanTokens = input + output + reasoning`).
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.reasoning_tokens
    }
}

/// Cumulative usage across the manager thread's life. The durable slice of telemetry. Token counters
/// are split MANAGER vs WORKER so prod (`lila status`) and the eval baseline can both show where the
/// budget went — the manager thread's own context vs the real work delegated to ephemeral workers.
/// Backend attribution (codex vs claude) lives one level up: the snapshot records the active backend,
/// and each eval trial is tagged with the backend it ran, so a run can be diffed per backend.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageMeter {
    /// Manager-thread token totals.
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub manager_turns: u64,
    /// Count of worker runs dispatched.
    pub worker_turns: u64,
    /// Worker token totals (summed across every ephemeral worker run).
    #[serde(default)]
    pub worker_input_tokens: u64,
    #[serde(default)]
    pub worker_cached_input_tokens: u64,
    #[serde(default)]
    pub worker_output_tokens: u64,
    #[serde(default)]
    pub worker_reasoning_tokens: u64,
}

impl UsageMeter {
    /// Billable manager-thread tokens (input + output + reasoning).
    pub fn manager_total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.reasoning_tokens
    }

    /// Billable worker tokens, summed across all worker runs.
    pub fn worker_total_tokens(&self) -> u64 {
        self.worker_input_tokens + self.worker_output_tokens + self.worker_reasoning_tokens
    }

    /// Whole-system billable tokens (manager + workers).
    pub fn total_tokens(&self) -> u64 {
        self.manager_total_tokens() + self.worker_total_tokens()
    }
}

/// One manager turn's envelope (metadata + usage), for the trace panel.
#[derive(Debug, Clone, Serialize)]
pub struct TurnRecord {
    pub turn_id: u64,
    pub kind: &'static str,
    pub request: String,
    pub chat_id: i64,
    pub iterations: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// One reconstructed conversation message (the model-level log the Inspector's Conversation tab
/// renders). `role` is "user" | "assistant"; `blocks` are the interleaved text / reasoning / tool
/// activity, reusing the trace block shape so there is one source of truth for the wire format.
#[derive(Debug, Clone, Serialize)]
pub struct ConvMessage {
    pub role: String,
    pub blocks: Vec<TraceBlock>,
}

/// One worker dispatch as the manager wrote it, stamped with the launching manager turn — the
/// Inspector's Workers/Trace tabs render the dispatch history (workers are single-shot, no roster).
#[derive(Debug, Clone, Serialize)]
pub struct WorkerPrompt {
    pub turn_id: u64,
    pub worker_id: String,
    pub kind: String,
    pub prompt: String,
}

const MAX_TURNS: usize = 500;
const MAX_CONVERSATION: usize = 400;
const MAX_PROMPTS: usize = 500;

/// In-memory telemetry. Owned by the app, shared with the MCP server and Inspector via `Arc<Mutex>`.
#[derive(Debug, Default)]
pub struct Telemetry {
    meter: UsageMeter,
    last_context_tokens: u64,
    turns: VecDeque<TurnRecord>,
    conversation: VecDeque<ConvMessage>,
    prompts: VecDeque<WorkerPrompt>,
}

impl Telemetry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a turn: record its envelope and bump the manager-turn counter.
    pub fn begin_turn(&mut self, turn_id: u64, kind: &'static str, request: String, chat_id: i64) {
        self.turns.push_back(TurnRecord {
            turn_id,
            kind,
            request,
            chat_id,
            iterations: 0,
            input_tokens: 0,
            output_tokens: 0,
        });
        self.meter.manager_turns += 1;
        while self.turns.len() > MAX_TURNS {
            self.turns.pop_front();
        }
    }

    /// Record one streamed turn's usage (called per `turn.completed`).
    pub fn record_usage(&mut self, turn_id: u64, usage: TokenUsage) {
        self.meter.input_tokens += usage.input_tokens;
        self.meter.cached_input_tokens += usage.cached_input_tokens;
        self.meter.output_tokens += usage.output_tokens;
        self.meter.reasoning_tokens += usage.reasoning_tokens;
        self.last_context_tokens = usage.input_tokens;
        if let Some(rec) = self.turns.iter_mut().find(|r| r.turn_id == turn_id) {
            rec.iterations += 1;
            rec.input_tokens += usage.input_tokens;
            rec.output_tokens += usage.output_tokens;
        }
    }

    /// Record an owner message as a `user` conversation entry (the Inspector's Conversation tab).
    pub fn record_user_message(&mut self, text: String) {
        self.push_message(ConvMessage {
            role: "user".into(),
            blocks: vec![TraceBlock::Text { text }],
        });
    }

    /// Record the manager's streamed output (text / reasoning / tool activity) as one `assistant`
    /// conversation entry. Empty block lists (e.g. a usage-only event) are dropped.
    pub fn record_assistant_blocks(&mut self, blocks: Vec<TraceBlock>) {
        if blocks.is_empty() {
            return;
        }
        self.push_message(ConvMessage {
            role: "assistant".into(),
            blocks,
        });
    }

    fn push_message(&mut self, msg: ConvMessage) {
        self.conversation.push_back(msg);
        while self.conversation.len() > MAX_CONVERSATION {
            self.conversation.pop_front();
        }
    }

    /// Record that the manager dispatched a worker (counts the launch).
    pub fn record_worker_launch(&mut self) {
        self.meter.worker_turns += 1;
    }

    /// Record a worker dispatch (its prompt + the launching manager turn) for the dispatch history.
    pub fn record_worker_prompt(
        &mut self,
        turn_id: u64,
        worker_id: String,
        kind: &'static str,
        prompt: String,
    ) {
        self.prompts.push_back(WorkerPrompt {
            turn_id,
            worker_id,
            kind: kind.into(),
            prompt,
        });
        while self.prompts.len() > MAX_PROMPTS {
            self.prompts.pop_front();
        }
    }

    /// Fold a finished worker run's token usage into the cumulative worker totals.
    pub fn record_worker_usage(&mut self, usage: TokenUsage) {
        self.meter.worker_input_tokens += usage.input_tokens;
        self.meter.worker_cached_input_tokens += usage.cached_input_tokens;
        self.meter.worker_output_tokens += usage.output_tokens;
        self.meter.worker_reasoning_tokens += usage.reasoning_tokens;
    }

    pub fn meter(&self) -> UsageMeter {
        self.meter
    }

    pub fn context_tokens(&self) -> u64 {
        self.last_context_tokens
    }

    pub fn turns(&self) -> Vec<TurnRecord> {
        self.turns.iter().cloned().collect()
    }

    /// The reconstructed manager conversation (oldest first).
    pub fn conversation(&self) -> Vec<ConvMessage> {
        self.conversation.iter().cloned().collect()
    }

    /// The worker dispatch history (oldest first).
    pub fn prompts(&self) -> Vec<WorkerPrompt> {
        self.prompts.iter().cloned().collect()
    }

    /// The durable usage slice (folded into the crash snapshot).
    pub fn usage_snapshot(&self) -> UsageMeter {
        self.meter
    }

    /// Restore the cumulative meter from a snapshot.
    pub fn load_usage(&mut self, meter: UsageMeter) {
        self.meter = meter;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_usage_and_persists() {
        let mut t = Telemetry::new();
        t.begin_turn(1, "owner_message", "hi".into(), 7);
        t.record_usage(
            1,
            TokenUsage {
                input_tokens: 100,
                output_tokens: 20,
                ..Default::default()
            },
        );
        t.record_worker_launch();
        t.record_worker_usage(TokenUsage {
            input_tokens: 500,
            output_tokens: 80,
            reasoning_tokens: 10,
            ..Default::default()
        });
        let m = t.usage_snapshot();
        assert_eq!(m.input_tokens, 100);
        assert_eq!(m.manager_turns, 1);
        assert_eq!(m.worker_turns, 1);
        // Worker tokens accumulate separately from the manager thread's.
        assert_eq!(m.worker_input_tokens, 500);
        assert_eq!(m.manager_total_tokens(), 120);
        assert_eq!(m.worker_total_tokens(), 590);
        assert_eq!(m.total_tokens(), 710);

        let mut t2 = Telemetry::new();
        t2.load_usage(m);
        assert_eq!(t2.meter(), m);
    }
}
