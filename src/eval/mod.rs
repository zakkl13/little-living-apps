//! The eval framework: measures the NON-deterministic part of the agent — how the real manager and
//! real workers behave on ambiguous asks (quality, persona adherence, token/turn efficiency). Port
//! of the TS `eval/` suite, redesigned to drive the COMPILED `lila` binary (the project's
//! integration-test principle) rather than an in-process app: a trial spawns `lila run` against a
//! fake Telegram server with `LILA_EVAL_TRACE` set, sends the scenario's owner turns, drains to
//! quiescence via the trace's `idle` marker, then grades the captured transcript + the real
//! on-disk workspace and memory the workers left behind. See `eval/DESIGN.md`.
//!
//! Layering (mirrors the TS division of labor): everything here that is DETERMINISTIC — the grader
//! library, the workspace fixture's planted realities, the trace reader, the stat aggregation — is
//! covered by `cargo test`, so a live eval run only ever spends model time on the live question.

pub mod checks;
pub mod fake_telegram;
pub mod fixture;
pub mod harness;
pub mod judge;
pub mod report;
pub mod run;
pub mod scenarios;
pub mod trace_read;
pub mod transcript;

pub use report::{
    Baseline, BaselineEntry, JudgeReport, ScenarioSummary, TrialReport, baseline_key, summarize,
};
pub use transcript::{
    Axis, Check, CheckOutcome, EvalTranscript, SerializedCheck, TimelineEntry, TokenStats,
    WorkerSession, grade, score,
};
