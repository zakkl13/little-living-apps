// The eval harness: boots the FULL production system — real manager Codex thread (AGENTS.md +
// locked-down thread factory + loopback Lila MCP server) at prod settings, real git+sqlite memory,
// real serialized loop and orchestrator, and REAL Codex workers doing real shell/file/git work on a
// real per-trial workspace (eval/fixture.ts). The single substitution is Telegram: deliveries are
// captured for grading instead of sent. One trial = seed workspace+memory → send the scenario's
// owner turns → drain to quiescence (workers complete, their events re-enter, follow-up turns run)
// → grade the transcript AND the workspace's actual end state.
//
// Division of labor with the test suite: `npm test` covers everything deterministic (the runtime,
// the graders themselves — see eval/graders.test.ts). Evals exist solely to measure the
// NON-deterministic part: how the real manager + real workers behave.

import { mkdirSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";

import { loadConfig } from "../src/config.js";
import { createManagerApp, type ManagerApp } from "../src/app.js";
import { clipSummarizer } from "../src/workers/summarize.js";

import { gitCommitFixture, writeWorkspace } from "./fixture.js";
import { instrumentWorkers } from "./workers.js";
import { TRIAL_SCHEMA } from "./types.js";
import type {
  Check,
  EvalTranscript,
  Scenario,
  SerializedCheck,
  TimelineEntry,
  TimelineEvent,
  TrialReport,
  TrialResult,
} from "./types.js";

export const EVAL_OWNER_ID = 77_000_001;

export interface HarnessOptions {
  /** Wall-clock budget for the whole trial (drain included). */
  timeoutMs: number;
  /** Keep the trial's temp dirs on disk (for debugging). */
  keepTmp?: boolean;
  /** Recorded into the report's settings block (for later interpretability). */
  sandboxMode: string;
}

export interface TrialOutcome {
  result: Omit<TrialResult, "trial" | "transcriptFile">;
  /** The self-contained review record, minus what only the runner knows (runId/trial/judge). */
  report: Omit<TrialReport, "runId" | "trial" | "judge">;
}

export async function runScenarioTrial(scenario: Scenario, opts: HarnessOptions): Promise<TrialOutcome> {
  const dir = mkdtempSync(join(tmpdir(), "lila-eval-"));
  const workspace = join(dir, "workspace");
  mkdirSync(workspace, { recursive: true });

  // Production parity on purpose: no model/effort overrides — the manager runs exactly as deployed
  // (xhigh, SDK-default model) and workers run the production runner with the production sandbox
  // default (danger-full-access; the existing prod knob CODEX_SANDBOX_MODE passes through for
  // anyone uneasy running that on a non-disposable laptop).
  const config = loadConfig({
    TELEGRAM_BOT_TOKEN: "eval-token",
    ALLOWED_USER_IDS: String(EVAL_OWNER_ID),
    WORKSPACE_DIR: workspace,
    MEMORY_DIR: join(dir, "memory"),
    MANAGER_STATE_DIR: join(dir, "state"),
    TELEGRAM_API_BASE_URL: "http://127.0.0.1:1", // never contacted: deliver() is captured below
    ...(process.env.CODEX_SANDBOX_MODE?.trim() ? { CODEX_SANDBOX_MODE: process.env.CODEX_SANDBOX_MODE } : {}),
    ...(process.env.CODEX_BIN?.trim() ? { CODEX_BIN: process.env.CODEX_BIN } : {}),
  });

  // A real codebase for real workers: base fixture + scenario overlay, committed.
  writeWorkspace(workspace, scenario.workspace);
  scenario.setup?.(workspace);
  gitCommitFixture(workspace);

  // ---- trial-scoped capture ----
  const timeline: TimelineEntry[] = [];
  const deliveries: string[] = [];
  let seq = 0;
  const record = (entry: TimelineEvent): void => {
    timeline.push({ ...entry, seq: ++seq, at: Date.now() });
  };

  const workers = instrumentWorkers(config, record);

  const startedAt = Date.now();
  const app: ManagerApp = await createManagerApp({
    config,
    runner: workers,
    deliver: async (_chatId, text) => {
      deliveries.push(text);
      record({ type: "delivery", text });
    },
    summarize: clipSummarizer(),
  });

  let error: string | undefined;
  try {
    // Seed memory fixtures (visible to the manager via the per-turn context header / index).
    for (const [path, body] of Object.entries(scenario.memory ?? {})) {
      app.mem.create({ command: "create", path, file_text: body });
    }

    app.start();
    const deadline = startedAt + opts.timeoutMs;
    for (const text of scenario.turns) {
      record({ type: "owner_msg", text });
      app.enqueueOwner(EVAL_OWNER_ID, text);
      await drainToQuiescence(app, deadline);
    }
  } catch (err) {
    error = (err as Error).message;
  }

  const durationMs = Date.now() - startedAt;
  const transcript: EvalTranscript = {
    scenario: scenario.name,
    timeline,
    deliveries,
    conversation: app.telemetry.conversation(),
    workerPrompts: app.telemetry.prompts(),
    workerSessions: workers.sessions,
    usage: app.telemetry.meter(),
    mem: {
      search: (q) => app.mem.search(q),
      read: (rel) => app.mem.readRelative(rel),
      system: () => app.mem.loadSystem(),
      tree: () => app.mem.treeListing(),
    },
    workspaceDir: workspace,
    durationMs,
  };

  // Grade while memory is still open; a trial that errored is graded on whatever it produced.
  const checks = gradeChecks(scenario.checks, transcript);
  const { score, pass } = scoreChecks(checks);

  const report: Omit<TrialReport, "runId" | "trial" | "judge"> = {
    schema: TRIAL_SCHEMA,
    scenario: {
      name: scenario.name,
      axis: scenario.axis,
      description: scenario.description,
      smoke: scenario.smoke ?? false,
      turns: scenario.turns,
      ...(scenario.rubric ? { rubric: scenario.rubric } : {}),
      workspaceOverlay: Object.keys(scenario.workspace ?? {}),
      memorySeeds: Object.keys(scenario.memory ?? {}),
    },
    settings: {
      model: "(sdk default — prod parity)",
      reasoningEffort: "xhigh (prod parity)",
      sandboxMode: opts.sandboxMode,
    },
    startedAt: new Date(startedAt).toISOString(),
    durationMs,
    pass: pass && !error,
    score: error ? 0 : score,
    checks,
    ...(error ? { error } : {}),
    ownerMessages: timeline.flatMap((e) => (e.type === "owner_msg" ? [e.text] : [])),
    deliveries,
    timeline,
    conversation: transcript.conversation,
    managerTurns: app.telemetry.turns(),
    workerPrompts: transcript.workerPrompts,
    workerSessions: workers.sessions,
    usage: transcript.usage,
    memory: { tree: transcript.mem.tree(), system: transcript.mem.system() },
    workspace: { dir: workspace, files: listFiles(workspace) },
  };

  await closeQuietly(app);
  if (!opts.keepTmp) rmSync(dir, { recursive: true, force: true });

  return {
    result: {
      scenario: scenario.name,
      axis: scenario.axis,
      pass: pass && !error,
      score: error ? 0 : score,
      checks,
      durationMs,
      usage: transcript.usage,
      ...(error ? { error } : {}),
    },
    report,
  };
}

// ---- grading ----------------------------------------------------------------

export function gradeChecks(checks: Check[], t: EvalTranscript): SerializedCheck[] {
  return checks.map((c) => {
    let outcome: { pass: boolean; detail?: string };
    try {
      outcome = c.run(t);
    } catch (err) {
      outcome = { pass: false, detail: `check threw: ${(err as Error).message}` };
    }
    return {
      name: c.name,
      required: c.required ?? true,
      weight: c.weight ?? 1,
      pass: outcome.pass,
      ...(outcome.detail ? { detail: outcome.detail } : {}),
    };
  });
}

export function scoreChecks(checks: SerializedCheck[]): { score: number; pass: boolean } {
  const total = checks.reduce((s, c) => s + c.weight, 0);
  const earned = checks.reduce((s, c) => s + (c.pass ? c.weight : 0), 0);
  const pass = checks.every((c) => c.pass || !c.required);
  return { score: total > 0 ? earned / total : 1, pass };
}

// ---- quiescence -------------------------------------------------------------

/** Resolve once the queue is empty, no turn is running, and no worker is running — i.e. the whole
 *  cascade triggered by the last owner message (worker completions, follow-up turns, re-validation
 *  loops) has settled. Times out at `deadline`. */
async function drainToQuiescence(app: ManagerApp, deadline: number): Promise<void> {
  for (;;) {
    await withDeadline(app.loop.whenIdle(), deadline, "manager turn");
    await withDeadline(app.orchestrator.whenQuiet(), deadline, "worker");
    // A worker completion enqueues an event in a microtask window; give it a beat, then re-check.
    await sleep(30);
    const busy =
      app.queue.size() > 0 || app.orchestrator.list().some((w) => w.status === "running");
    if (!busy) {
      await withDeadline(app.loop.whenIdle(), deadline, "manager turn");
      const settled =
        app.queue.size() === 0 && !app.orchestrator.list().some((w) => w.status === "running");
      if (settled) return;
    }
  }
}

function withDeadline<T>(p: Promise<T>, deadline: number, what: string): Promise<T> {
  const remaining = deadline - Date.now();
  if (remaining <= 0) return Promise.reject(new Error(`trial timed out waiting on ${what}`));
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`trial timed out waiting on ${what} (${Math.round(remaining / 1000)}s budget)`)),
      remaining,
    );
    p.then(
      (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      (e) => {
        clearTimeout(timer);
        reject(e);
      },
    );
  });
}

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

/** Recursive workspace listing for the persisted transcript (skips .git / node_modules). */
function listFiles(root: string): string[] {
  const out: string[] = [];
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.name === ".git" || entry.name === "node_modules") continue;
      const abs = join(dir, entry.name);
      if (entry.isDirectory()) walk(abs);
      else out.push(relative(root, abs));
    }
  };
  try {
    walk(root);
  } catch {
    /* workspace may be gone on an errored trial */
  }
  return out.sort();
}

/** Close without letting a wedged in-flight turn hang the CLI (run.ts hard-exits at the end). */
async function closeQuietly(app: ManagerApp): Promise<void> {
  await Promise.race([app.close().catch(() => undefined), sleep(15_000)]);
}
