// Eval vocabulary. An eval runs the FULL production system — the real manager Codex thread (same
// AGENTS.md + context header + Lila MCP server + xhigh effort), real git+sqlite memory, and REAL
// Codex workers doing real work on a real per-trial workspace. The manager⇄worker interplay is the
// thing under evaluation, so nothing in that loop is scripted or tuned. The only substitution is
// Telegram (a captured delivery sink), because messages must be graded, not sent.
//
// Grading philosophy (see eval/README.md for sources):
//   - grade OUTCOMES and final state, not the exact tool path — agents find alternate valid routes
//   - code graders first (deterministic, free); an LLM judge only for soft qualities, behind a rubric
//   - scenarios are nondeterministic → support N trials and report mean score + pass^k
//   - every trial's full transcript is written to disk for human review

import type { ConvMessage } from "../src/manager/driver.js";
import type { PromptRecord, TurnRecord, UsageMeter } from "../src/runtime/telemetry.js";
import type { SearchHit } from "../src/memory/fts.js";

/** The behavior axes we optimize toward. Scenarios are tagged so the report can roll up per axis
 *  and `--axis` can focus a run while tuning one behavior. */
export type Axis =
  | "delegation" // hands real work to subagents, scopes them, acks and lets go
  | "validation" // independently verifies user-visible work before calling it done
  | "reply-discipline" // NO_REPLY on noise, no narration, outcome language to the owner
  | "memory" // writes durable facts down; recalls them instead of guessing or delegating
  | "autonomy" // acts on inferable decisions; escalates only genuinely owner-only calls
  | "honesty"; // never fabricates state it cannot know; grounds answers in worker reports

export const ALL_AXES: readonly Axis[] = [
  "delegation",
  "validation",
  "reply-discipline",
  "memory",
  "autonomy",
  "honesty",
];

/** An ordered record of everything externally observable during a trial. Ordering checks
 *  (e.g. "no done-claim delivered before the validator PASSed") read this. `worker_note` entries
 *  are the real worker's live progress lines (commands run, files changed) — debugging gold.
 *  Worker events carry the `callId` of their WorkerSession so parallel workers' interleaved
 *  notes stay attributable (a review UI can lane-split the timeline by callId). */
export type TimelineEvent =
  | { type: "owner_msg"; text: string }
  | { type: "delivery"; text: string }
  | { type: "worker_call"; callId: number; prompt: string; resumeThreadId?: string }
  | { type: "worker_note"; callId: number; note: string }
  | { type: "worker_done"; callId: number; ok: boolean; response: string };

export type TimelineEntry = TimelineEvent & { seq: number; at: number };

/** One real worker run, fully attributed: what it was told, what it said while working, what it
 *  reported, and the Codex thread identity (resume chains share a threadId). The unit a review UI
 *  renders as a worker lane. (Known gap: the Codex SDK does not report per-worker token usage.) */
export interface WorkerSession {
  /** Ordinal (1-based) dispatch id; timeline worker_* events reference it. */
  callId: number;
  /** The manager's objective — protocol preamble stripped (its own words; what graders read). */
  prompt: string;
  /** Exactly what the worker received (standing protocol + objective). */
  promptFull: string;
  /** Set when the manager resumed an existing worker thread (steer / follow-up). */
  resumeThreadId?: string;
  /** The worker's Codex thread id (links resumes into one session chain). */
  threadId?: string;
  /** Live progress lines the worker emitted while running. */
  notes: Array<{ at: number; note: string }>;
  /** Unset only if the trial errored mid-run. */
  ok?: boolean;
  response?: string;
  startedAt: number;
  endedAt?: number;
}

/** Read-only view of final memory state (live MemFs handles, valid while checks run). */
export interface MemView {
  /** FTS across all memory files. */
  search(query: string): SearchHit[];
  /** Read one file by repo-relative path (e.g. "system/owner.md"); undefined if absent. */
  read(relPath: string): string | undefined;
  /** The always-loaded system/ bodies. */
  system(): string;
  /** The archival/recall index listing. */
  tree(): string;
}

/** Everything a grader can look at after a trial has drained to quiescence. */
export interface EvalTranscript {
  scenario: string;
  timeline: TimelineEntry[];
  /** Owner-visible messages, in order (what actually reached "Telegram"). */
  deliveries: string[];
  /** The manager's reconstructed conversation log (assistant text incl. NO_REPLY choices,
   *  tool_use/tool_result, thinking) — the model-level view, pre host gating. */
  conversation: ConvMessage[];
  /** Every prompt the manager dispatched to workers, stamped with turn ids. */
  workerPrompts: PromptRecord[];
  /** Real worker runs in dispatch order (including resumes), fully attributed. */
  workerSessions: WorkerSession[];
  usage: UsageMeter;
  mem: MemView;
  /** The trial's real workspace on disk (still present while checks run) — graders assert the
   *  actual end state: file contents, `node --test`, live HTTP probes. */
  workspaceDir: string;
  durationMs: number;
}

export interface CheckOutcome {
  pass: boolean;
  /** Short human-readable evidence (shown in reports; invaluable when triaging a failure). */
  detail?: string;
}

export interface Check {
  name: string;
  /** Weight toward the scenario score (default 1). */
  weight?: number;
  /** When false, a failure lowers the score but does not fail the scenario (default true). */
  required?: boolean;
  run(t: EvalTranscript): CheckOutcome;
}

export interface Scenario {
  name: string;
  axis: Axis;
  /** What desired behavior this scenario exercises — also shown to the judge. */
  description: string;
  /** Include in `--smoke` (a fast, representative subset). */
  smoke?: boolean;
  /** Seed memory files before the first turn: "/memories/..." path → file body. */
  memory?: Record<string, string>;
  /** Workspace file overlay applied on top of the base fixture (eval/fixture.ts) before git
   *  commit — how a scenario plants a real bug or a red test for real workers to fix. */
  workspace?: Record<string, string>;
  /** Imperative fixture mutations (set mtimes, create binaries…) after files are written,
   *  before the fixture commit. */
  setup?: (workspaceDir: string) => void;
  /** Owner messages, sent one at a time; the harness drains to quiescence after each. */
  turns: string[];
  /** Minimum per-trial wall-clock budget in ms; the runner takes max(--timeout, this). Long-horizon
   *  scenarios (many turns, many workers) set it so the global default can't strangle them. */
  timeoutMs?: number;
  checks: Check[];
  /** Soft-quality rubric for the Codex judge (only consulted with --judge). */
  rubric?: string;
}

// ---- results ----------------------------------------------------------------

export interface SerializedCheck extends CheckOutcome {
  name: string;
  required: boolean;
  weight: number;
}

/** The judge's verdict as persisted: a scored review, a failure, or null (not judged). */
export type JudgeReport = { score: number; reasoning: string } | { error: string };

export const TRIAL_SCHEMA = "lila-eval-trial@1";

/** THE per-trial artifact (`<scenario>.t<n>.json`): one self-contained, versioned record of
 *  everything that happened — owner messages, the manager's full conversation (thinking, tool
 *  calls, tool results), per-turn token envelopes, every worker session (prompts, live notes,
 *  reports, thread ids), the interleaved timeline, grading, and the judge's review. Built so a
 *  review UI can render a trial from this file alone, with no access to the repo or the run. */
export interface TrialReport {
  schema: typeof TRIAL_SCHEMA;
  runId: string;
  trial: number;

  /** The scenario as configured (inputs included, so the file stands alone). */
  scenario: {
    name: string;
    axis: Axis;
    description: string;
    smoke: boolean;
    /** The owner messages sent, in order. */
    turns: string[];
    rubric?: string;
    /** Workspace files the scenario overlaid on the base fixture (planted bugs / red tests). */
    workspaceOverlay: string[];
    /** Memory paths seeded before the first turn. */
    memorySeeds: string[];
  };

  /** Always production parity; recorded so old reports stay interpretable. */
  settings: { model: string; reasoningEffort: string; sandboxMode: string };

  startedAt: string; // ISO 8601
  durationMs: number;

  // ---- grading ----
  pass: boolean;
  score: number;
  checks: SerializedCheck[];
  judge: JudgeReport | null;
  error?: string;

  // ---- what happened ----
  /** Owner → manager messages, in order (also present in the timeline as owner_msg). */
  ownerMessages: string[];
  /** Manager → owner messages that passed the reply gate (what "Telegram" would have sent). */
  deliveries: string[];
  /** The interleaved event stream, seq/at-stamped: owner_msg | delivery | worker_call(callId) |
   *  worker_note(callId) | worker_done(callId). The review UI's master ordering. */
  timeline: TimelineEntry[];
  /** The manager's reconstructed conversation: assistant text (incl. NO_REPLY choices), thinking
   *  blocks, tool_use / tool_result — the model-level view, pre host gating. */
  conversation: ConvMessage[];
  /** Per-manager-turn envelopes: what opened the turn, iterations, and the four token counters. */
  managerTurns: TurnRecord[];
  /** Worker dispatches as the manager wrote them (raw objective, recorded at the tool seam before
   *  the protocol preamble is prepended), turn- and worker-stamped. Join to workerSessions via
   *  `prompt` (telemetry's raw objective === the session's protocol-stripped prompt). */
  workerPrompts: PromptRecord[];
  /** One entry per real worker run: stripped + full prompt, live notes, report, thread id. */
  workerSessions: WorkerSession[];
  /** Cumulative manager-thread token usage + turn/worker counts. */
  usage: UsageMeter;

  // ---- end state ----
  memory: { tree: string; system: string };
  workspace: { dir: string; files: string[] };
}

export interface TrialResult {
  scenario: string;
  axis: Axis;
  trial: number;
  pass: boolean;
  /** Weighted fraction of code checks passed, 0..1. */
  score: number;
  /** Judge rubric score 0..1 (only with --judge and a rubric). */
  judgeScore?: number;
  judgeReasoning?: string;
  checks: SerializedCheck[];
  durationMs: number;
  usage: UsageMeter;
  error?: string;
  /** Relative path of the persisted transcript JSON. */
  transcriptFile?: string;
}

export interface ScenarioSummary {
  scenario: string;
  axis: Axis;
  trials: number;
  /** All trials passed (pass^k). */
  pass: boolean;
  /** Fraction of trials that passed (pass@1 estimate). */
  passRate: number;
  meanScore: number;
  meanJudgeScore?: number;
  // Token efficiency is a first-class signal (the same outcome in 3 manager turns beats 9):
  meanManagerTurns: number;
  meanWorkerRuns: number;
  /** Mean total tokens (input + output + reasoning) across manager and workers. */
  meanTokens: number;
}

export interface RunReport {
  startedAt: string;
  /** Always production parity: SDK-default model, xhigh manager effort. Recorded for the report. */
  model: string;
  reasoningEffort: string;
  sandboxMode: string;
  trials: number;
  judge: boolean;
  scenarios: ScenarioSummary[];
  trialsDetail: TrialResult[];
  /** scenario → meanScore, the shape persisted as eval/baseline.json. */
  scores: Record<string, number>;
}
