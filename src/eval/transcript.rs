//! Eval vocabulary: the per-trial transcript a grader reads, the grader (`Check`) shape, and the
//! token-stat record that flows into reports + the baseline. Grade OUTCOMES and final state, not the
//! exact tool path — agents find alternate valid routes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::runtime::UsageMeter;

/// The behavior axes the suite optimizes toward (tagged per scenario for per-axis roll-ups).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Axis {
    /// Hands real work to subagents, scopes them, acks and lets go.
    Delegation,
    /// Claims done only on evidence — workers self-validate and report proof.
    Validation,
    /// NO_REPLY on noise, no narration, one outcome report, matches the owner's register.
    ReplyDiscipline,
    /// Writes durable facts down; recalls them instead of guessing or re-delegating.
    Memory,
    /// Acts on inferable requests; escalates only genuinely owner-only calls.
    Autonomy,
    /// Never fabricates state it cannot know; grounds answers in worker reports.
    Honesty,
}

impl Axis {
    pub fn as_str(self) -> &'static str {
        match self {
            Axis::Delegation => "delegation",
            Axis::Validation => "validation",
            Axis::ReplyDiscipline => "reply-discipline",
            Axis::Memory => "memory",
            Axis::Autonomy => "autonomy",
            Axis::Honesty => "honesty",
        }
    }
}

/// One entry in the seq-stamped master timeline (the ordering checks read this).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TimelineEntry {
    OwnerMsg {
        seq: u64,
        text: String,
    },
    Delivery {
        seq: u64,
        text: String,
    },
    WorkerCall {
        seq: u64,
        call_id: u64,
        prompt: String,
    },
    WorkerNote {
        seq: u64,
        call_id: u64,
        note: String,
    },
    WorkerDone {
        seq: u64,
        call_id: u64,
        ok: bool,
        response: String,
    },
}

impl TimelineEntry {
    pub fn seq(&self) -> u64 {
        match self {
            TimelineEntry::OwnerMsg { seq, .. }
            | TimelineEntry::Delivery { seq, .. }
            | TimelineEntry::WorkerCall { seq, .. }
            | TimelineEntry::WorkerNote { seq, .. }
            | TimelineEntry::WorkerDone { seq, .. } => *seq,
        }
    }
}

/// A worker dispatch as the manager wrote it, stamped with the launching manager turn (the
/// parallel-decomposition join key for `parallel_starts_in_first_turn`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerPrompt {
    pub turn_id: u64,
    pub kind: String,
    pub prompt: String,
}

/// One real worker run, fully attributed (the unit a review UI renders as a lane).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkerSession {
    pub call_id: u64,
    pub prompt: String,
    pub notes: Vec<String>,
    pub ok: bool,
    pub response: String,
}

/// One assistant/tool message in the manager's reconstructed conversation (the model-level view,
/// pre host gating — what `chose_silence` and the judge read). Blocks kept as raw trace blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvMessage {
    pub role: String,
    pub blocks: Vec<crate::runtime::TraceBlock>,
}

/// The token breakdown that is the headline of both `lila status` (prod) and the eval baseline:
/// manager thread vs the work tier, plus turn/run counts. Derived from the cumulative [`UsageMeter`].
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenStats {
    pub manager_tokens: u64,
    pub worker_tokens: u64,
    pub total_tokens: u64,
    pub manager_turns: u64,
    pub worker_runs: u64,
}

impl TokenStats {
    pub fn from_meter(m: &UsageMeter) -> Self {
        Self {
            manager_tokens: m.manager_total_tokens(),
            worker_tokens: m.worker_total_tokens(),
            total_tokens: m.total_tokens(),
            manager_turns: m.manager_turns,
            worker_runs: m.worker_turns,
        }
    }
}

/// Everything a grader can look at after a trial has drained to quiescence.
pub struct EvalTranscript {
    pub scenario: String,
    pub timeline: Vec<TimelineEntry>,
    /// Owner-visible messages, in order (what reached "Telegram").
    pub deliveries: Vec<String>,
    /// The manager's reconstructed conversation (assistant text incl. NO_REPLY, thinking, tools).
    pub conversation: Vec<ConvMessage>,
    /// Every prompt the manager dispatched to workers, turn-stamped.
    pub worker_prompts: Vec<WorkerPrompt>,
    /// Real worker runs in dispatch order, fully attributed.
    pub worker_sessions: Vec<WorkerSession>,
    /// Cumulative usage (manager + worker token split).
    pub usage: UsageMeter,
    /// The trial's real workspace on disk (graders assert the actual end state).
    pub workspace_dir: PathBuf,
    /// The trial's real memory dir on disk (graders assert durable facts landed).
    pub memory_dir: PathBuf,
}

impl EvalTranscript {
    /// Substring search across all memory files (dependency-free analogue of the FTS `memoryContains`
    /// — grading "a durable fact landed" doesn't need the index, just the bytes). Returns the first
    /// matching file's relative path.
    pub fn memory_contains(&self, needle: &str) -> Option<String> {
        let needle = needle.to_lowercase();
        find_in_dir(&self.memory_dir, &self.memory_dir, &needle)
    }
}

/// Recursively scan `dir` for a markdown file whose lowercased body contains `needle`.
fn find_in_dir(root: &Path, dir: &Path, needle: &str) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if let Some(hit) = find_in_dir(root, &path, needle) {
                return Some(hit);
            }
        } else if file_contains(&path, needle) {
            return Some(rel_display(root, &path));
        }
    }
    None
}

fn file_contains(path: &Path, needle: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|body| body.to_lowercase().contains(needle))
}

fn rel_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

// ---- grading ----------------------------------------------------------------

/// The result of running one check against a transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub pass: bool,
    /// Short human-readable evidence (shown in reports; invaluable when triaging a failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl CheckOutcome {
    pub fn pass() -> Self {
        Self {
            pass: true,
            detail: None,
        }
    }
    pub fn pass_with(detail: impl Into<String>) -> Self {
        Self {
            pass: true,
            detail: Some(detail.into()),
        }
    }
    pub fn fail(detail: impl Into<String>) -> Self {
        Self {
            pass: false,
            detail: Some(detail.into()),
        }
    }
}

/// A deterministic grader: a named, weighted predicate over the transcript. `required = false` shaves
/// the score on failure but does not fail the scenario (efficiency budgets are soft).
pub struct Check {
    pub name: String,
    pub weight: f64,
    pub required: bool,
    pub run: Box<dyn Fn(&EvalTranscript) -> CheckOutcome + Send + Sync>,
}

impl Check {
    /// A required, weight-1 check from a name + predicate.
    pub fn new(
        name: impl Into<String>,
        run: impl Fn(&EvalTranscript) -> CheckOutcome + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            weight: 1.0,
            required: true,
            run: Box::new(run),
        }
    }

    /// Mark this check soft (non-gating).
    pub fn soft(mut self) -> Self {
        self.required = false;
        self
    }
}

/// A graded check as persisted in the trial report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedCheck {
    pub name: String,
    pub required: bool,
    pub weight: f64,
    pub pass: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Run every check (catching panics-as-failures is unnecessary here — checks are infallible by
/// construction), returning the serialized outcomes.
pub fn grade(checks: &[Check], t: &EvalTranscript) -> Vec<SerializedCheck> {
    checks
        .iter()
        .map(|c| {
            let outcome = (c.run)(t);
            SerializedCheck {
                name: c.name.clone(),
                required: c.required,
                weight: c.weight,
                pass: outcome.pass,
                detail: outcome.detail,
            }
        })
        .collect()
}

/// Weighted fraction passed (0..1) + the hard pass/fail (every required check passed).
pub fn score(checks: &[SerializedCheck]) -> (f64, bool) {
    let total: f64 = checks.iter().map(|c| c.weight).sum();
    let earned: f64 = checks.iter().filter(|c| c.pass).map(|c| c.weight).sum();
    let pass = checks.iter().all(|c| c.pass || !c.required);
    let frac = if total > 0.0 { earned / total } else { 1.0 };
    (frac, pass)
}
