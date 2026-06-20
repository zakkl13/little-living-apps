//! Run aggregation + the enriched baseline. The baseline is keyed by `scenario@backend` (so codex
//! and claude are tracked independently — diffing one against the other is the point), and every
//! entry carries not just the mean score but the token breakdown (manager vs worker vs total) and
//! turn/run counts: a prompt change that holds the score flat but doubles tokens is a regression too.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::eval::transcript::{
    ConvMessage, SerializedCheck, TimelineEntry, TokenStats, WorkerPrompt, WorkerSession,
};

/// The judge's verdict as persisted: a scored review or a failure (`None` = not judged).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JudgeReport {
    Scored { score: f64, reasoning: String },
    Failed { error: String },
}

/// One trial's self-contained record (persisted as `<scenario>.t<n>.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialReport {
    pub scenario: String,
    pub axis: String,
    pub backend: String,
    pub trial: u32,
    pub pass: bool,
    pub score: f64,
    pub stats: TokenStats,
    pub checks: Vec<SerializedCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<JudgeReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub deliveries: Vec<String>,
    pub timeline: Vec<TimelineEntry>,
    pub worker_sessions: Vec<WorkerSession>,
    /// The manager's reconstructed conversation + its worker dispatches — the judge's view, and part
    /// of the self-contained review record.
    #[serde(default)]
    pub conversation: Vec<ConvMessage>,
    #[serde(default)]
    pub worker_prompts: Vec<WorkerPrompt>,
}

/// Per-scenario roll-up across trials (pass^k + means), tagged with the backend it ran on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioSummary {
    pub scenario: String,
    pub axis: String,
    pub backend: String,
    pub trials: usize,
    /// All trials passed (pass^k — the honest signal for nondeterministic agents).
    pub pass: bool,
    pub pass_rate: f64,
    pub mean_score: f64,
    pub mean_judge_score: Option<f64>,
    pub mean_manager_turns: f64,
    pub mean_worker_runs: f64,
    pub mean_manager_tokens: f64,
    pub mean_worker_tokens: f64,
    pub mean_total_tokens: f64,
}

/// The committed regression reference: `scenario@backend` → its blessed stats.
pub type Baseline = BTreeMap<String, BaselineEntry>;

/// One baseline row — score gate plus the efficiency dimensions a regression can hide in.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BaselineEntry {
    pub mean_score: f64,
    pub pass_rate: f64,
    pub mean_manager_tokens: f64,
    pub mean_worker_tokens: f64,
    pub mean_total_tokens: f64,
    pub mean_manager_turns: f64,
    pub mean_worker_runs: f64,
}

/// The baseline key for a scenario on a given backend.
pub fn baseline_key(scenario: &str, backend: &str) -> String {
    format!("{scenario}@{backend}")
}

impl ScenarioSummary {
    pub fn baseline_entry(&self) -> BaselineEntry {
        BaselineEntry {
            mean_score: round2(self.mean_score),
            pass_rate: round2(self.pass_rate),
            mean_manager_tokens: self.mean_manager_tokens.round(),
            mean_worker_tokens: self.mean_worker_tokens.round(),
            mean_total_tokens: self.mean_total_tokens.round(),
            mean_manager_turns: round2(self.mean_manager_turns),
            mean_worker_runs: round2(self.mean_worker_runs),
        }
    }
}

/// Aggregate a scenario's trials into a summary (all trials share scenario/axis/backend).
pub fn summarize(
    scenario: &str,
    axis: &str,
    backend: &str,
    trials: &[TrialReport],
) -> ScenarioSummary {
    let n = trials.len().max(1) as f64;
    let mean = |f: &dyn Fn(&TrialReport) -> f64| trials.iter().map(f).sum::<f64>() / n;
    let judged: Vec<f64> = trials.iter().filter_map(judge_score).collect();
    ScenarioSummary {
        scenario: scenario.to_string(),
        axis: axis.to_string(),
        backend: backend.to_string(),
        trials: trials.len(),
        pass: !trials.is_empty() && trials.iter().all(|t| t.pass),
        pass_rate: trials.iter().filter(|t| t.pass).count() as f64 / n,
        mean_score: mean(&|t| t.score),
        mean_judge_score: mean_opt(&judged),
        mean_manager_turns: mean(&|t| t.stats.manager_turns as f64),
        mean_worker_runs: mean(&|t| t.stats.worker_runs as f64),
        mean_manager_tokens: mean(&|t| t.stats.manager_tokens as f64),
        mean_worker_tokens: mean(&|t| t.stats.worker_tokens as f64),
        mean_total_tokens: mean(&|t| t.stats.total_tokens as f64),
    }
}

fn judge_score(t: &TrialReport) -> Option<f64> {
    match &t.judge {
        Some(JudgeReport::Scored { score, .. }) => Some(*score),
        _ => None,
    }
}

fn mean_opt(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        None
    } else {
        Some(xs.iter().sum::<f64>() / xs.len() as f64)
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Render a human summary table (one row per scenario) with the full token breakdown.
pub fn render_markdown(summaries: &[ScenarioSummary]) -> String {
    let mut lines = vec![
        "| scenario | axis | backend | score | pass | judge | mgr turns | wkr runs | mgr tok | wkr tok | total tok |".to_string(),
        "|---|---|---|---|---|---|---|---|---|---|---|".to_string(),
    ];
    for s in summaries {
        lines.push(format!(
            "| {} | {} | {} | {:.2} | {:.0}% | {} | {:.1} | {:.1} | {:.0} | {:.0} | {:.0} |",
            s.scenario,
            s.axis,
            s.backend,
            s.mean_score,
            s.pass_rate * 100.0,
            s.mean_judge_score
                .map_or("—".to_string(), |j| format!("{j:.2}")),
            s.mean_manager_turns,
            s.mean_worker_runs,
            s.mean_manager_tokens,
            s.mean_worker_tokens,
            s.mean_total_tokens,
        ));
    }
    lines.join("\n") + "\n"
}
