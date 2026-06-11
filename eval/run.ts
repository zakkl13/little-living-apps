// Eval runner CLI. Examples:
//
//   npm run eval -- --smoke                # representative subset (real manager + real workers)
//   npm run eval                           # full suite — slow and real: budget hours, not minutes
//   npm run eval -- --axis validation --trials 3 --judge
//   npm run eval -- --update-baseline      # bless the current scores as the regression baseline
//
// Everything runs at FULL PRODUCTION PARITY on your ChatGPT-subscription login (no API key — this
// repo refuses metered billing): the real manager thread at xhigh on the SDK-default model, and
// REAL Codex workers running real shell/file/git work on a throwaway per-trial workspace. There are
// deliberately no model/effort knobs — an eval of a tuned-down system measures a system you don't
// ship. Evals measure ONLY the non-deterministic part (behavior quality + token efficiency);
// everything deterministic, including the graders, is covered by `npm test`. Results land in
// eval/results/<run-id>/ with one transcript JSON per trial; report.md is the human summary;
// baseline diffing flags regressions.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { runScenarioTrial, type HarnessOptions } from "./harness.js";
import { judgeTrial } from "./judge.js";
import { selectScenarios, scenarios as allScenarios } from "./scenarios.js";
import {
  ALL_AXES,
  type JudgeReport,
  type RunReport,
  type ScenarioSummary,
  type TrialReport,
  type TrialResult,
} from "./types.js";

const EVAL_DIR = dirname(fileURLToPath(import.meta.url));
const BASELINE_PATH = join(EVAL_DIR, "baseline.json");

const { values: args } = parseArgs({
  options: {
    filter: { type: "string" },
    axis: { type: "string" },
    trials: { type: "string", default: "1" },
    judge: { type: "boolean", default: false },
    smoke: { type: "boolean", default: false },
    timeout: { type: "string", default: "1800" }, // seconds per trial (real workers take minutes)
    "update-baseline": { type: "boolean", default: false },
    "keep-tmp": { type: "boolean", default: false },
    strict: { type: "boolean", default: false }, // exit 1 on regression vs baseline
    list: { type: "boolean", default: false },
    help: { type: "boolean", default: false },
  },
});

function usage(): void {
  console.log(`lila eval runner — the FULL production system (real manager + real workers) against the behavior suite

  --smoke              only scenarios tagged smoke (representative subset)
  --filter <substr>    scenarios whose name contains <substr>
  --axis <axis>        one of: ${ALL_AXES.join(", ")}
  --trials <n>         trials per scenario (default 1; use 3+ when tuning — agents are nondeterministic)
  --judge              also score scenarios that carry a rubric with the Codex judge
  --timeout <sec>      per-trial wall clock budget (default 1800 — real workers take minutes)
  --update-baseline    write mean scores to eval/baseline.json (the regression reference)
  --strict             exit non-zero if any scenario regressed vs the baseline
  --keep-tmp           keep each trial's temp dirs (workspace included) for debugging
  --list               print the suite and exit

  No --effort/--model knobs on purpose: evals run at production parity (xhigh, SDK-default model,
  production worker sandbox). CODEX_SANDBOX_MODE=workspace-write tames workers on a non-disposable
  laptop (prod default is danger-full-access).`);
}

const sandboxMode = (): string => process.env.CODEX_SANDBOX_MODE?.trim() || "danger-full-access";

function preflight(): void {
  for (const key of ["OPENAI_API_KEY", "CODEX_API_KEY"] as const) {
    if (process.env[key]?.trim()) {
      console.error(
        `✋ ${key} is set in your shell. The harness strips it from every Codex call (subscription ` +
          `billing only), but unset it to be safe.`,
      );
    }
  }
  const codexHome = process.env.CODEX_HOME?.trim() || join(homedir(), ".codex");
  if (!existsSync(join(codexHome, "auth.json"))) {
    console.error(
      `⚠️  No Codex auth found at ${codexHome}/auth.json — evals drive real Codex threads on ` +
        `your ChatGPT subscription. Run \`codex login\` first (or set CODEX_HOME). Continuing; ` +
        `expect auth failures.`,
    );
  }
  const confined = sandboxMode() !== "danger-full-access";
  console.error(
    `⚠️  Workers are REAL Codex agents running shell commands in a throwaway temp workspace, ` +
      `sandbox=${sandboxMode()}` +
      (confined
        ? ` (overridden — prod default is danger-full-access).`
        : ` (prod parity). Set CODEX_SANDBOX_MODE=workspace-write to confine them on this machine.`),
  );
}

const pct = (x: number): string => `${Math.round(x * 100)}%`;
const bar = (x: number): string => {
  const n = Math.round(x * 10);
  return "█".repeat(n) + "░".repeat(10 - n);
};

async function main(): Promise<number> {
  if (args.help) {
    usage();
    return 0;
  }

  const picked = selectScenarios({
    ...(args.filter ? { filter: args.filter } : {}),
    ...(args.axis ? { axis: args.axis } : {}),
    smoke: args.smoke,
  });

  if (args.list) {
    for (const s of allScenarios) {
      const mark = picked.includes(s) ? "•" : " ";
      console.log(
        `${mark} ${s.name.padEnd(24)} ${s.axis.padEnd(17)}${s.smoke ? " [smoke]" : ""}${s.rubric ? " [rubric]" : ""}`,
      );
    }
    return 0;
  }
  if (picked.length === 0) {
    console.error("No scenarios matched.");
    return 1;
  }
  if (args.axis && !ALL_AXES.includes(args.axis as (typeof ALL_AXES)[number])) {
    console.error(`Unknown axis "${args.axis}". Axes: ${ALL_AXES.join(", ")}`);
    return 1;
  }

  const trials = Math.max(1, Number(args.trials) || 1);
  const timeoutMs = Math.max(10, Number(args.timeout) || 1800) * 1000;
  preflight();

  const runId = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  const outDir = join(EVAL_DIR, "results", runId);
  mkdirSync(outDir, { recursive: true });

  console.log(
    `\nlila eval — ${picked.length} scenario(s) × ${trials} trial(s), ` +
      `prod parity (manager xhigh / sdk-default model, real workers, sandbox=${sandboxMode()})` +
      (args.judge ? ", judge on" : "") +
      `\nresults → ${outDir}\n`,
  );

  const harnessOpts: HarnessOptions = {
    timeoutMs,
    sandboxMode: sandboxMode(),
    ...(args["keep-tmp"] ? { keepTmp: true } : {}),
  };

  const trialResults: TrialResult[] = [];
  for (const scenario of picked) {
    for (let trial = 1; trial <= trials; trial++) {
      process.stdout.write(`▶ ${scenario.name} (trial ${trial}/${trials}) … `);
      const t0 = Date.now();
      let result: TrialResult;
      try {
        const outcome = await runScenarioTrial(scenario, {
          ...harnessOpts,
          // A scenario may declare a larger floor (long-horizon trials outlast the global default).
          timeoutMs: Math.max(harnessOpts.timeoutMs, scenario.timeoutMs ?? 0),
        });
        const transcriptFile = `${scenario.name}.t${trial}.json`;
        result = { ...outcome.result, trial, transcriptFile };

        let judge: JudgeReport | null = null;
        if (args.judge && scenario.rubric && !outcome.result.error) {
          try {
            const verdict = await judgeTrial(scenario.description, scenario.rubric, outcome.report);
            judge = verdict;
            result.judgeScore = verdict.score;
            result.judgeReasoning = verdict.reasoning;
          } catch (err) {
            judge = { error: (err as Error).message };
            result.judgeReasoning = `judge failed: ${(err as Error).message}`;
          }
        }

        const trialReport: TrialReport = { ...outcome.report, runId, trial, judge };
        writeFileSync(join(outDir, transcriptFile), JSON.stringify(trialReport, null, 2));
      } catch (err) {
        result = {
          scenario: scenario.name,
          axis: scenario.axis,
          trial,
          pass: false,
          score: 0,
          checks: [],
          durationMs: Date.now() - t0,
          usage: { inputTokens: 0, cachedInputTokens: 0, outputTokens: 0, reasoningTokens: 0, managerTurns: 0, codexTurns: 0 },
          error: (err as Error).message,
        };
      }
      trialResults.push(result);

      const judgeNote = result.judgeScore !== undefined ? ` judge=${result.judgeScore.toFixed(2)}` : "";
      console.log(
        `${result.pass ? "✅" : "❌"} score=${result.score.toFixed(2)}${judgeNote} (${Math.round(result.durationMs / 1000)}s)` +
          (result.error ? ` — ${result.error}` : ""),
      );
      for (const c of result.checks.filter((c) => !c.pass)) {
        console.log(`    ✗ ${c.name}${c.required ? "" : " (optional)"}${c.detail ? ` — ${c.detail}` : ""}`);
      }
    }
  }

  // ---- aggregate ----
  const summaries: ScenarioSummary[] = picked.map((s) => {
    const rows = trialResults.filter((r) => r.scenario === s.name);
    const judged = rows.filter((r) => r.judgeScore !== undefined);
    const mean = (f: (r: TrialResult) => number): number => rows.reduce((a, r) => a + f(r), 0) / rows.length;
    return {
      scenario: s.name,
      axis: s.axis,
      trials: rows.length,
      pass: rows.every((r) => r.pass),
      passRate: rows.filter((r) => r.pass).length / rows.length,
      meanScore: mean((r) => r.score),
      ...(judged.length > 0
        ? { meanJudgeScore: judged.reduce((a, r) => a + r.judgeScore!, 0) / judged.length }
        : {}),
      meanManagerTurns: mean((r) => r.usage.managerTurns),
      meanWorkerRuns: mean((r) => r.usage.codexTurns),
      meanTokens: mean((r) => r.usage.inputTokens + r.usage.outputTokens + r.usage.reasoningTokens),
    };
  });

  const report: RunReport = {
    startedAt: runId,
    model: "(sdk default — prod parity)",
    reasoningEffort: "xhigh (prod parity)",
    sandboxMode: sandboxMode(),
    trials,
    judge: args.judge,
    scenarios: summaries,
    trialsDetail: trialResults,
    scores: Object.fromEntries(summaries.map((s) => [s.scenario, round2(s.meanScore)])),
  };
  writeFileSync(join(outDir, "report.json"), JSON.stringify(report, null, 2));
  writeFileSync(join(outDir, "report.md"), renderMarkdown(report));

  // ---- print summary + baseline diff ----
  const baseline = loadBaseline();
  console.log("\n── summary ──────────────────────────────────────────────────");
  for (const s of summaries) {
    const base = baseline?.[s.scenario];
    const delta =
      base !== undefined ? ` (baseline ${base.toFixed(2)} ${diffMark(s.meanScore - base)})` : "";
    const judge = s.meanJudgeScore !== undefined ? `  judge ${s.meanJudgeScore.toFixed(2)}` : "";
    const eff = `  ${s.meanManagerTurns.toFixed(1)}mt/${s.meanWorkerRuns.toFixed(1)}w/${kTokens(s.meanTokens)}`;
    console.log(
      `${s.pass ? "✅" : "❌"} ${s.scenario.padEnd(24)} ${bar(s.meanScore)} ${s.meanScore.toFixed(2)}` +
        `  pass ${pct(s.passRate)}${judge}${eff}${delta}`,
    );
  }
  for (const axis of ALL_AXES) {
    const rows = summaries.filter((s) => s.axis === axis);
    if (rows.length === 0) continue;
    const mean = rows.reduce((a, r) => a + r.meanScore, 0) / rows.length;
    console.log(`   axis ${axis.padEnd(17)} ${bar(mean)} ${mean.toFixed(2)} over ${rows.length}`);
  }
  const overall = summaries.reduce((a, s) => a + s.meanScore, 0) / summaries.length;
  console.log(`   OVERALL ${"".padEnd(15)} ${bar(overall)} ${overall.toFixed(2)}\n`);

  let regressed = false;
  if (baseline) {
    for (const s of summaries) {
      const base = baseline[s.scenario];
      if (base !== undefined && s.meanScore < base - 0.05) {
        regressed = true;
        console.log(`📉 regression: ${s.scenario} ${base.toFixed(2)} → ${s.meanScore.toFixed(2)}`);
      }
    }
  }

  if (args["update-baseline"]) {
    const merged = { ...(baseline ?? {}), ...report.scores };
    writeFileSync(BASELINE_PATH, JSON.stringify(merged, null, 2) + "\n");
    console.log(`baseline updated → ${BASELINE_PATH}`);
  } else if (!baseline) {
    console.log("no baseline yet — rerun with --update-baseline to bless these scores");
  }

  return args.strict && regressed ? 1 : 0;
}

function loadBaseline(): Record<string, number> | undefined {
  if (!existsSync(BASELINE_PATH)) return undefined;
  try {
    return JSON.parse(readFileSync(BASELINE_PATH, "utf8")) as Record<string, number>;
  } catch {
    return undefined;
  }
}

const round2 = (x: number): number => Math.round(x * 100) / 100;
const diffMark = (d: number): string => (d > 0.05 ? "📈" : d < -0.05 ? "📉" : "≈");
const kTokens = (n: number): string => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${Math.round(n)}`) + "tok";

function renderMarkdown(r: RunReport): string {
  const lines = [
    `# lila eval — ${r.startedAt}`,
    "",
    `model **${r.model}** · effort **${r.reasoningEffort}** · sandbox **${r.sandboxMode}** · trials **${r.trials}** · judge **${r.judge ? "on" : "off"}**`,
    "",
    "| scenario | axis | score | pass rate | judge | mgr turns | worker runs | tokens |",
    "|---|---|---|---|---|---|---|---|",
    ...r.scenarios.map(
      (s) =>
        `| ${s.scenario} | ${s.axis} | ${s.meanScore.toFixed(2)} | ${pct(s.passRate)} | ${s.meanJudgeScore?.toFixed(2) ?? "—"} | ${s.meanManagerTurns.toFixed(1)} | ${s.meanWorkerRuns.toFixed(1)} | ${kTokens(s.meanTokens)} |`,
    ),
    "",
    "## Failed checks",
  ];
  const failures = r.trialsDetail.flatMap((t) =>
    t.checks
      .filter((c) => !c.pass)
      .map((c) => `- **${t.scenario}** t${t.trial}: ${c.name}${c.detail ? ` — ${c.detail}` : ""}`),
  );
  const errors = r.trialsDetail.filter((t) => t.error).map((t) => `- **${t.scenario}** t${t.trial}: ⚠️ ${t.error}`);
  lines.push(...(failures.length || errors.length ? [...errors, ...failures] : ["(none)"]));
  return lines.join("\n") + "\n";
}

main().then(
  (code) => process.exit(code),
  (err) => {
    console.error(err);
    process.exit(1);
  },
);
