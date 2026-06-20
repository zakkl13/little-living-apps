//! The `lila-eval` CLI: select scenarios, run N trials each against the COMPILED binary at production
//! parity, aggregate, and diff against the committed baseline. Everything rides the subscription (no
//! API key — the billing guard applies); there are deliberately no model/effort knobs. Results land
//! in `eval/results/<run-id>/`. The baseline is keyed `scenario@backend` and carries the full token
//! breakdown, so a regression that hides in tokens (flat score, double the spend) still shows.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;

use crate::eval::harness::{HarnessOptions, run_trial};
use crate::eval::judge::{default_judge_backend, judge_trial};
use crate::eval::report::{
    Baseline, JudgeReport, ScenarioSummary, TrialReport, baseline_key, render_markdown, summarize,
};
use crate::eval::scenarios::{Scenario, scenarios, select};

#[derive(Parser, Debug)]
#[command(
    name = "lila-eval",
    about = "Run the lila behavior eval suite against the compiled binary"
)]
pub struct Args {
    /// Only scenarios tagged smoke (a fast, representative subset).
    #[arg(long)]
    pub smoke: bool,
    /// Only scenarios on this axis (delegation, validation, reply-discipline, memory, autonomy, honesty).
    #[arg(long)]
    pub axis: Option<String>,
    /// Only scenarios whose name contains this substring.
    #[arg(long)]
    pub filter: Option<String>,
    /// Trials per scenario (use 2+ when tuning — agents are nondeterministic; pass^k is the signal).
    #[arg(long, default_value_t = 1)]
    pub trials: u32,
    /// Backend to boot (production parity each). Defaults to $AGENT_BACKEND, else codex.
    #[arg(long)]
    pub backend: Option<String>,
    /// Per-trial wall-clock budget in seconds (real workers take minutes).
    #[arg(long, default_value_t = 1800)]
    pub timeout: u64,
    /// Worker sandbox parity (`danger-full-access`) — the default is the safer `workspace-write`.
    #[arg(long)]
    pub prod_sandbox: bool,
    /// Also score scenarios that carry a rubric with an LLM judge (soft qualities only).
    #[arg(long)]
    pub judge: bool,
    /// Judge backend (`codex`|`claude`). Defaults to the OPPOSITE of the manager (less self-bias);
    /// fix it to hold the judge constant while comparing manager backends.
    #[arg(long)]
    pub judge_backend: Option<String>,
    /// Hermetic self-test: drive the scripted fake backend (no subscription) — smoke-tests the harness.
    #[arg(long)]
    pub fake: bool,
    /// Keep each trial's temp dir for forensics.
    #[arg(long)]
    pub keep_tmp: bool,
    /// Write the resulting mean stats to the baseline (the regression reference).
    #[arg(long)]
    pub update_baseline: bool,
    /// Exit non-zero if any scenario regressed vs the baseline.
    #[arg(long)]
    pub strict: bool,
    /// Print the suite and exit.
    #[arg(long)]
    pub list: bool,
}

/// Entry point for the `lila-eval` binary.
pub async fn main(args: Args) -> i32 {
    let backend = args
        .backend
        .clone()
        .or_else(|| {
            std::env::var("AGENT_BACKEND")
                .ok()
                .filter(|b| !b.is_empty())
        })
        .unwrap_or_else(|| "codex".to_string());
    let picked = select(
        scenarios(),
        args.smoke,
        args.axis.as_deref(),
        args.filter.as_deref(),
    );

    if args.list {
        print_list(&picked);
        return 0;
    }
    if picked.is_empty() {
        eprintln!("No scenarios matched.");
        return 1;
    }

    let opts = HarnessOptions {
        backend: backend.clone(),
        sandbox: if args.prod_sandbox {
            "danger-full-access".into()
        } else {
            "workspace-write".into()
        },
        timeout_secs: args.timeout,
        fake: args.fake,
        keep_tmp: args.keep_tmp,
    };

    eprintln!(
        "lila eval — {} scenario(s) × {} trial(s), backend={}, sandbox={}{}",
        picked.len(),
        args.trials,
        backend,
        opts.sandbox,
        if args.fake {
            " [FAKE backend — harness self-test]"
        } else {
            ""
        },
    );

    let judge = JudgeCfg {
        enabled: args.judge,
        backend: args
            .judge_backend
            .clone()
            .unwrap_or_else(|| default_judge_backend(&backend).to_string()),
    };
    if judge.enabled {
        eprintln!("judge: on (backend={})", judge.backend);
    }

    let out_dir = results_dir();
    let _ = std::fs::create_dir_all(&out_dir);
    let summaries = run_all(&picked, &opts, args.trials, &backend, &out_dir, &judge).await;
    report_and_diff(&summaries, &backend, &args, &out_dir)
}

/// Judge configuration for a run.
struct JudgeCfg {
    enabled: bool,
    backend: String,
}

fn print_list(picked: &[Scenario]) {
    for s in picked {
        println!(
            "{:24} {:17}{}",
            s.name,
            s.axis.as_str(),
            if s.smoke { " [smoke]" } else { "" }
        );
    }
}

/// Run every picked scenario × trials, persisting each trial report, returning per-scenario summaries.
async fn run_all(
    picked: &[Scenario],
    opts: &HarnessOptions,
    trials: u32,
    backend: &str,
    out_dir: &std::path::Path,
    judge: &JudgeCfg,
) -> Vec<ScenarioSummary> {
    let mut summaries = Vec::new();
    for scenario in picked {
        let mut rows = Vec::new();
        for trial in 1..=trials {
            eprint!("▶ {} (trial {}/{}) … ", scenario.name, trial, trials);
            let report = run_one(scenario, trial, opts, judge).await;
            print_trial_line(&report);
            persist_trial(out_dir, &report);
            rows.push(report);
        }
        summaries.push(summarize(
            &scenario.name,
            scenario.axis.as_str(),
            backend,
            &rows,
        ));
    }
    summaries
}

async fn run_one(
    scenario: &Scenario,
    trial: u32,
    opts: &HarnessOptions,
    judge: &JudgeCfg,
) -> TrialReport {
    let mut report = match run_trial(scenario, trial, opts).await {
        Ok(report) => report,
        Err(err) => return error_report(scenario, trial, opts, &err.to_string()),
    };
    if judge.enabled
        && report.error.is_none()
        && let Some(rubric) = &scenario.rubric
    {
        report.judge =
            Some(run_judge(&judge.backend, &scenario.description, rubric, &report).await);
    }
    report
}

/// Score one trial with the LLM judge, capturing a failure as a `Failed` verdict (never fatal).
async fn run_judge(
    backend: &str,
    description: &str,
    rubric: &str,
    report: &TrialReport,
) -> JudgeReport {
    match judge_trial(backend, description, rubric, report).await {
        Ok(v) => JudgeReport::Scored {
            score: v.score,
            reasoning: v.reasoning,
        },
        Err(e) => JudgeReport::Failed {
            error: e.to_string(),
        },
    }
}

fn error_report(scenario: &Scenario, trial: u32, opts: &HarnessOptions, err: &str) -> TrialReport {
    TrialReport {
        scenario: scenario.name.clone(),
        axis: scenario.axis.as_str().to_string(),
        backend: opts.backend.clone(),
        trial,
        pass: false,
        score: 0.0,
        stats: Default::default(),
        checks: Vec::new(),
        judge: None,
        error: Some(err.to_string()),
        deliveries: Vec::new(),
        timeline: Vec::new(),
        worker_sessions: Vec::new(),
        conversation: Vec::new(),
        worker_prompts: Vec::new(),
    }
}

/// A `judge=0.NN` suffix for the trial line when the trial was judged.
fn judge_note(r: &TrialReport) -> String {
    match &r.judge {
        Some(JudgeReport::Scored { score, .. }) => format!(" judge={score:.2}"),
        Some(JudgeReport::Failed { .. }) => " judge=ERR".to_string(),
        None => String::new(),
    }
}

fn print_trial_line(r: &TrialReport) {
    let mark = if r.pass { "✅" } else { "❌" };
    eprintln!(
        "{mark} score={:.2}{}  {}mt/{}w/{}tok{}",
        r.score,
        judge_note(r),
        r.stats.manager_turns,
        r.stats.worker_runs,
        r.stats.total_tokens,
        r.error
            .as_ref()
            .map(|e| format!(" — {e}"))
            .unwrap_or_default(),
    );
    for c in r.checks.iter().filter(|c| !c.pass) {
        let opt = if c.required { "" } else { " (optional)" };
        let detail = c
            .detail
            .as_ref()
            .map(|d| format!(" — {d}"))
            .unwrap_or_default();
        eprintln!("    ✗ {}{opt}{detail}", c.name);
    }
}

fn persist_trial(out_dir: &std::path::Path, r: &TrialReport) {
    if let Ok(json) = serde_json::to_string_pretty(r) {
        let _ = std::fs::write(
            out_dir.join(format!("{}.t{}.json", r.scenario, r.trial)),
            json,
        );
    }
}

/// Print the summary table + the baseline diff; optionally bless the baseline; compute the exit code.
fn report_and_diff(
    summaries: &[ScenarioSummary],
    backend: &str,
    args: &Args,
    out_dir: &std::path::Path,
) -> i32 {
    let _ = std::fs::write(out_dir.join("report.md"), render_markdown(summaries));
    if let Ok(json) = serde_json::to_string_pretty(summaries) {
        let _ = std::fs::write(out_dir.join("report.json"), json);
    }
    eprintln!("\n{}", render_markdown(summaries));

    let baseline = load_baseline();
    let regressed = diff_baseline(summaries, &baseline, backend);
    if args.update_baseline {
        update_baseline(summaries, baseline, backend);
    }
    eprintln!("results → {}", out_dir.display());
    if args.strict && regressed { 1 } else { 0 }
}

/// Flag scenarios whose mean score dropped > 0.05 below baseline; returns whether any regressed.
fn diff_baseline(summaries: &[ScenarioSummary], baseline: &Baseline, backend: &str) -> bool {
    let mut regressed = false;
    for s in summaries {
        let Some(base) = baseline.get(&baseline_key(&s.scenario, backend)) else {
            continue;
        };
        if s.mean_score < base.mean_score - 0.05 {
            regressed = true;
            eprintln!(
                "📉 regression: {} {:.2} → {:.2}",
                s.scenario, base.mean_score, s.mean_score
            );
        }
        let tok_delta = s.mean_total_tokens - base.mean_total_tokens;
        if base.mean_total_tokens > 0.0 && tok_delta > base.mean_total_tokens * 0.25 {
            eprintln!(
                "📈 token bloat: {} {:.0} → {:.0} tokens (+{:.0}%)",
                s.scenario,
                base.mean_total_tokens,
                s.mean_total_tokens,
                tok_delta / base.mean_total_tokens * 100.0,
            );
        }
    }
    regressed
}

fn update_baseline(summaries: &[ScenarioSummary], mut baseline: Baseline, backend: &str) {
    for s in summaries {
        baseline.insert(baseline_key(&s.scenario, backend), s.baseline_entry());
    }
    if let Ok(json) = serde_json::to_string_pretty(&baseline) {
        let _ = std::fs::write(baseline_path(), json + "\n");
        eprintln!("baseline updated → {}", baseline_path().display());
    }
}

fn load_baseline() -> Baseline {
    std::fs::read_to_string(baseline_path())
        .ok()
        .and_then(|b| serde_json::from_str::<Baseline>(&b).ok())
        .unwrap_or_default()
}

fn baseline_path() -> PathBuf {
    PathBuf::from("eval/baseline.json")
}

fn results_dir() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    PathBuf::from("eval/results").join(stamp.to_string())
}
