//! The eval/inspector trace: an append-only JSONL event stream the running binary emits when
//! `LILA_EVAL_TRACE=<path>` is set, and a complete no-op otherwise (one `Option` check on the hot
//! path). It is the structured, machine-readable view a grader (or the deferred Inspector plane)
//! reconstructs a run from: the interleaved timeline (owner messages, deliveries, worker
//! lifecycle), the manager's conversation (assistant text incl. `NO_REPLY`, reasoning, tool calls),
//! per-turn token usage, and an `idle` quiescence marker.
//!
//! Design (no global mutable state): a single [`EvalTrace`] is constructed in the `run` command and
//! shared — `Arc`-cloned — with the app loop and the worker orchestrator, the only two producers.
//! The file handle sits behind a `Mutex` (writes are rare and tiny); `current_turn` is an atomic the
//! app sets at each turn boundary so the orchestrator can stamp worker dispatches with the manager
//! turn that launched them (the `parallelStartsInFirstTurn` join key) without back-references.

use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::telemetry::TokenUsage;

/// One JSONL record. `type` tags the variant; the reader matches on it. Round-trippable so the eval
/// harness deserializes the very records the running binary wrote (one source of truth for the shape).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraceRecord {
    OwnerMsg {
        text: String,
    },
    Delivery {
        text: String,
    },
    /// The manager's conversation, one assistant/tool message at a time.
    ManagerMsg {
        role: String,
        blocks: Vec<TraceBlock>,
    },
    /// A worker dispatch as the manager wrote it, stamped with the launching manager turn.
    WorkerPrompt {
        turn_id: u64,
        kind: String,
        prompt: String,
    },
    WorkerCall {
        call_id: u64,
        prompt: String,
    },
    WorkerNote {
        call_id: u64,
        note: String,
    },
    WorkerDone {
        call_id: u64,
        ok: bool,
        response: String,
    },
    /// Per-turn token usage (tier = "manager" | "worker").
    Usage {
        tier: String,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        reasoning_tokens: u64,
    },
    /// The loop blocked with an empty queue and no workers in flight — the cascade has settled.
    Idle,
}

/// A block within a manager message: plain text, private reasoning, a tool call, or its result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "block", rename_all = "snake_case")]
pub enum TraceBlock {
    Text { text: String },
    Thinking,
    ToolUse { name: String },
    ToolResult { content: String },
}

/// The append-only trace sink. Inert unless built from `LILA_EVAL_TRACE`.
#[derive(Debug)]
pub struct EvalTrace {
    file: Mutex<std::fs::File>,
    current_turn: AtomicU64,
}

impl EvalTrace {
    /// Build a trace writing to `$LILA_EVAL_TRACE`, or `None` when the var is unset/empty (prod).
    pub fn from_env() -> Option<EvalTrace> {
        let path = std::env::var("LILA_EVAL_TRACE")
            .ok()
            .filter(|p| !p.is_empty())?;
        match std::fs::File::create(&path) {
            Ok(file) => Some(Self {
                file: Mutex::new(file),
                current_turn: AtomicU64::new(0),
            }),
            Err(err) => {
                tracing::warn!(%err, %path, "could not open eval trace; tracing disabled");
                None
            }
        }
    }

    /// Record which manager turn is in flight (so worker dispatches can be stamped with it).
    pub fn set_turn(&self, turn_id: u64) {
        self.current_turn.store(turn_id, Ordering::SeqCst);
    }

    /// The manager turn currently in flight.
    pub fn current_turn(&self) -> u64 {
        self.current_turn.load(Ordering::SeqCst)
    }

    /// Append one record as a JSON line. Best-effort: a trace write must never disturb the run.
    pub fn emit(&self, record: &TraceRecord) {
        let Ok(line) = serde_json::to_string(record) else {
            return;
        };
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(file, "{line}");
        }
    }

    /// Emit per-turn usage tagged with the tier that spent it.
    pub fn usage(&self, tier: &str, u: TokenUsage) {
        self.emit(&TraceRecord::Usage {
            tier: tier.to_string(),
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cached_input_tokens: u.cached_input_tokens,
            reasoning_tokens: u.reasoning_tokens,
        });
    }
}

/// Convenience: emit on an optional trace without an `if let` at every call site.
pub trait TraceExt {
    fn rec(&self, record: TraceRecord);
}

impl TraceExt for Option<std::sync::Arc<EvalTrace>> {
    fn rec(&self, record: TraceRecord) {
        if let Some(trace) = self {
            trace.emit(&record);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn writes_jsonl_records() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trace.jsonl");
        // SAFETY-free env set: scope it to this single-threaded test via a guard isn't needed — we
        // read the var only inside from_env, called immediately below.
        let trace = EvalTrace {
            file: Mutex::new(std::fs::File::create(&path).unwrap()),
            current_turn: AtomicU64::new(0),
        };
        trace.set_turn(1);
        trace.emit(&TraceRecord::OwnerMsg {
            text: "hello".into(),
        });
        trace.emit(&TraceRecord::WorkerPrompt {
            turn_id: trace.current_turn(),
            kind: "start".into(),
            prompt: "do the thing".into(),
        });
        trace.usage(
            "manager",
            TokenUsage {
                input_tokens: 10,
                output_tokens: 2,
                ..Default::default()
            },
        );
        drop(trace);

        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("\"type\":\"owner_msg\""));
        assert!(lines[1].contains("\"turn_id\":1"));
        assert!(lines[2].contains("\"tier\":\"manager\""));
    }

    #[test]
    fn option_ext_is_inert_when_none() {
        let none: Option<Arc<EvalTrace>> = None;
        none.rec(TraceRecord::Idle); // must not panic
    }
}
