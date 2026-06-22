//! The `lila-eval` CLI: select scenarios, run N trials each against the COMPILED binary at production
//! parity, aggregate, and diff against the committed baseline. Everything rides the subscription (no
//! API key — the billing guard applies); there are deliberately no model/effort knobs. Results land
//! in `eval/results/<run-id>/`. The baseline is keyed `scenario@backend` and carries the full token
//! breakdown, so a regression that hides in tokens (flat score, double the spend) still shows.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
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
    /// Write per-trial results into this dir instead of a fresh `eval/results/<timestamp>/`. Point
    /// several runs at the SAME dir to accumulate trials across runs, then bless the lot with
    /// `--from-results <dir> --update-baseline`. Files are run-stamped, so runs never clobber.
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
    /// Re-aggregate a prior run's saved results dir and run the report/diff/bless path WITHOUT
    /// spending a token. The safe way to establish a baseline: launch the run withOUT
    /// `--update-baseline` (each trial is persisted as it completes), then pass that
    /// `eval/results/<id>/` dir here with `--update-baseline` to bless it. Reads the per-trial JSONs,
    /// so it works even if a usage limit interrupted the original run before the aggregate was written.
    #[arg(long)]
    pub from_results: Option<PathBuf>,
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

    if let Some(dir) = args.from_results.clone() {
        return bless_from_results(&dir, &args);
    }

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

    let run_id = run_stamp();
    let out_dir = args.out_dir.clone().unwrap_or_else(|| results_dir(&run_id));
    if let Err(err) = std::fs::create_dir_all(&out_dir) {
        eprintln!("could not create results dir {}: {err}", out_dir.display());
        return 1;
    }
    eprintln!("results → {}", out_dir.display());
    // Stamp every per-trial file with a per-run id so repeated runs into the SAME --out-dir
    // accumulate instead of clobbering (`load_results` then aggregates the lot).
    let summaries = match run_all(
        &picked,
        &opts,
        args.trials,
        &backend,
        &out_dir,
        &run_id,
        &judge,
    )
    .await
    {
        Ok(summaries) => summaries,
        Err(err) => {
            eprintln!("could not persist eval results: {err:#}");
            return 1;
        }
    };
    report_and_diff(&summaries, &args, &out_dir)
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
    run_id: &str,
    judge: &JudgeCfg,
) -> anyhow::Result<Vec<ScenarioSummary>> {
    let mut summaries = Vec::new();
    for scenario in picked {
        let mut rows = Vec::new();
        for trial in 1..=trials {
            eprint!("▶ {} (trial {}/{}) … ", scenario.name, trial, trials);
            let report = run_one(scenario, trial, opts, judge).await;
            print_trial_line(&report);
            persist_trial(out_dir, run_id, &report).with_context(|| {
                format!("{} trial {trial} in {}", scenario.name, out_dir.display())
            })?;
            rows.push(report);
        }
        summaries.push(summarize(
            &scenario.name,
            scenario.axis.as_str(),
            backend,
            &rows,
        ));
    }
    Ok(summaries)
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

fn persist_trial(
    out_dir: &std::path::Path,
    run_id: &str,
    r: &TrialReport,
) -> anyhow::Result<PathBuf> {
    let path = out_dir.join(format!("{run_id}.{}.t{}.json", r.scenario, r.trial));
    let json = serde_json::to_string_pretty(r)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

/// Re-aggregate a prior run's saved per-trial JSONs and run the report/diff/bless path — no tokens
/// spent. The complement to a token-spending run launched WITHOUT `--update-baseline`: bank results
/// safely first (each trial is persisted as it completes), then bless the baseline here. Reads the
/// per-trial files (not the aggregate `report.json`), so it works even when a usage limit interrupted
/// the original run before the aggregate was written.
fn bless_from_results(dir: &std::path::Path, args: &Args) -> i32 {
    let summaries = load_results(dir);
    if summaries.is_empty() {
        eprintln!(
            "no per-trial results (`<scenario>.t<n>.json`) found in {}",
            dir.display()
        );
        return 1;
    }
    let backends = summaries
        .iter()
        .map(|s| s.backend.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    eprintln!(
        "loaded {} scenario result(s) from {} (backend={})",
        summaries.len(),
        dir.display(),
        backends,
    );
    report_and_diff(&summaries, args, dir)
}

/// Group every saved trial in `dir` by scenario+backend and summarize each (means + pass^k). Files
/// that don't parse as a `TrialReport` (e.g. the aggregate `report.json`) are skipped.
fn load_results(dir: &std::path::Path) -> Vec<ScenarioSummary> {
    let mut groups: BTreeMap<(String, String), Vec<TrialReport>> = Default::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(report) = serde_json::from_str::<TrialReport>(&body) {
            if setup_failure_before_agent_started(&report) {
                continue;
            }
            groups
                .entry((report.scenario.clone(), report.backend.clone()))
                .or_default()
                .push(report);
        }
    }
    groups
        .into_iter()
        .map(|((scenario, backend), mut rows)| {
            rows.sort_by_key(|r| r.trial);
            summarize(&scenario, &rows[0].axis, &backend, &rows)
        })
        .collect()
}

fn setup_failure_before_agent_started(r: &TrialReport) -> bool {
    r.error.is_some()
        && r.checks.is_empty()
        && r.deliveries.is_empty()
        && r.timeline.is_empty()
        && r.worker_sessions.is_empty()
        && r.conversation.is_empty()
        && r.worker_prompts.is_empty()
        && r.stats.total_tokens == 0
        && r.stats.manager_turns == 0
        && r.stats.worker_runs == 0
}

/// Print the summary table + the baseline diff; optionally bless the baseline; compute the exit code.
fn report_and_diff(summaries: &[ScenarioSummary], args: &Args, out_dir: &std::path::Path) -> i32 {
    let _ = std::fs::write(out_dir.join("report.md"), render_markdown(summaries));
    if let Ok(json) = serde_json::to_string_pretty(summaries) {
        let _ = std::fs::write(out_dir.join("report.json"), json);
    }
    eprintln!("\n{}", render_markdown(summaries));

    let baseline = load_baseline();
    let regressed = diff_baseline(summaries, &baseline);
    if args.update_baseline {
        if let Err(err) = update_baseline(summaries, baseline) {
            eprintln!("could not update baseline: {err:#}");
            return 1;
        }
    }
    eprintln!("results → {}", out_dir.display());
    if args.strict && regressed { 1 } else { 0 }
}

/// Flag scenarios whose mean score dropped > 0.05 below baseline; returns whether any regressed.
fn diff_baseline(summaries: &[ScenarioSummary], baseline: &Baseline) -> bool {
    let mut regressed = false;
    for s in summaries {
        let Some(base) = baseline.get(&baseline_key(&s.scenario, &s.backend)) else {
            continue;
        };
        if s.mean_score < base.mean_score - 0.05 {
            regressed = true;
            eprintln!(
                "📉 regression: {}@{} {:.2} → {:.2}",
                s.scenario, s.backend, base.mean_score, s.mean_score
            );
        }
        let tok_delta = s.mean_total_tokens - base.mean_total_tokens;
        if base.mean_total_tokens > 0.0 && tok_delta > base.mean_total_tokens * 0.25 {
            eprintln!(
                "📈 token bloat: {}@{} {:.0} → {:.0} tokens (+{:.0}%)",
                s.scenario,
                s.backend,
                base.mean_total_tokens,
                s.mean_total_tokens,
                tok_delta / base.mean_total_tokens * 100.0,
            );
        }
    }
    regressed
}

fn update_baseline(summaries: &[ScenarioSummary], mut baseline: Baseline) -> anyhow::Result<()> {
    for s in summaries {
        baseline.insert(baseline_key(&s.scenario, &s.backend), s.baseline_entry());
    }
    let path = baseline_path();
    let json = serde_json::to_string_pretty(&baseline)?;
    std::fs::write(&path, json + "\n")?;
    eprintln!("baseline updated → {}", path.display());
    Ok(())
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

fn results_dir(run_id: &str) -> PathBuf {
    PathBuf::from("eval/results").join(run_id)
}

fn run_stamp() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{stamp}-{}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::report::BaselineEntry;
    use crate::eval::transcript::TokenStats;

    fn trial(scenario: &str, backend: &str, score: f64, pass: bool, tokens: u64) -> TrialReport {
        TrialReport {
            scenario: scenario.to_string(),
            axis: "memory".to_string(),
            backend: backend.to_string(),
            trial: 1,
            pass,
            score,
            stats: TokenStats {
                manager_tokens: tokens,
                worker_tokens: 0,
                total_tokens: tokens,
                manager_turns: 1,
                worker_runs: 0,
            },
            checks: Vec::new(),
            judge: None,
            error: None,
            deliveries: Vec::new(),
            timeline: Vec::new(),
            worker_sessions: Vec::new(),
            conversation: Vec::new(),
            worker_prompts: Vec::new(),
        }
    }

    fn setup_failure(scenario: &str, backend: &str) -> TrialReport {
        TrialReport {
            scenario: scenario.to_string(),
            axis: "reply-discipline".to_string(),
            backend: backend.to_string(),
            trial: 1,
            pass: false,
            score: 0.0,
            stats: TokenStats::default(),
            checks: Vec::new(),
            judge: None,
            error: Some("touch failed".to_string()),
            deliveries: Vec::new(),
            timeline: Vec::new(),
            worker_sessions: Vec::new(),
            conversation: Vec::new(),
            worker_prompts: Vec::new(),
        }
    }

    fn baseline_entry(mean_score: f64) -> BaselineEntry {
        BaselineEntry {
            mean_score,
            pass_rate: 1.0,
            mean_manager_tokens: 0.0,
            mean_worker_tokens: 0.0,
            mean_total_tokens: 0.0,
            mean_manager_turns: 0.0,
            mean_worker_runs: 0.0,
        }
    }

    #[test]
    fn load_results_accumulates_chunks_and_keeps_backends_separate() {
        let tmp = tempfile::tempdir().unwrap();
        persist_trial(
            tmp.path(),
            "chunk-a",
            &trial("remember-fact", "codex", 1.0, true, 100),
        )
        .unwrap();
        persist_trial(
            tmp.path(),
            "chunk-b",
            &trial("remember-fact", "codex", 0.5, false, 300),
        )
        .unwrap();
        persist_trial(
            tmp.path(),
            "chunk-c",
            &trial("remember-fact", "claude", 0.8, true, 50),
        )
        .unwrap();
        persist_trial(
            tmp.path(),
            "bad-setup",
            &setup_failure("remember-fact", "codex"),
        )
        .unwrap();
        std::fs::write(tmp.path().join("report.json"), "[]").unwrap();

        let summaries = load_results(tmp.path());
        assert_eq!(summaries.len(), 2);

        let codex = summaries
            .iter()
            .find(|s| s.scenario == "remember-fact" && s.backend == "codex")
            .expect("codex summary");
        assert_eq!(codex.trials, 2);
        assert!(!codex.pass);
        assert_eq!(codex.pass_rate, 0.5);
        assert_eq!(codex.mean_score, 0.75);
        assert_eq!(codex.mean_total_tokens, 200.0);

        let claude = summaries
            .iter()
            .find(|s| s.scenario == "remember-fact" && s.backend == "claude")
            .expect("claude summary");
        assert_eq!(claude.trials, 1);
        assert!(claude.pass);
        assert_eq!(claude.mean_score, 0.8);
    }

    #[test]
    fn baseline_diff_uses_each_summary_backend() {
        let summary = summarize(
            "remember-fact",
            "memory",
            "claude",
            &[trial("remember-fact", "claude", 0.8, true, 10)],
        );
        let mut baseline = Baseline::default();
        baseline.insert(baseline_key("remember-fact", "codex"), baseline_entry(1.0));
        baseline.insert(baseline_key("remember-fact", "claude"), baseline_entry(0.8));

        assert!(!diff_baseline(&[summary], &baseline));
    }
}
