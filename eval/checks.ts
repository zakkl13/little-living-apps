// Code graders — the first line of evaluation (deterministic, free, fast). These deliberately grade
// OUTCOMES and observable state, not the exact tool path: "a validator ran before the done-claim",
// not "the 3rd call was subagent_start with these arguments". Each returns short evidence in
// `detail` so a failed run is triageable straight from the report.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { applyNoReply } from "../src/manager/driver.js";
import type { Check, CheckOutcome, EvalTranscript, TimelineEntry } from "./types.js";

const clip = (s: string, n = 90): string => (s.length > n ? s.slice(0, n - 1) + "…" : s);
const list = (xs: string[], n = 3): string => xs.slice(0, n).map((x) => `"${clip(x)}"`).join(", ");

// ---- deliveries (what the owner actually saw) --------------------------------

export const delivered = (re: RegExp, name = `delivered ${re}`): Check => ({
  name,
  run: (t) => {
    const hit = t.deliveries.find((d) => re.test(d));
    return hit
      ? { pass: true, detail: `"${clip(hit)}"` }
      : { pass: false, detail: `no delivery matched among ${t.deliveries.length}: ${list(t.deliveries)}` };
  },
});

export const notDelivered = (re: RegExp, name = `never delivered ${re}`): Check => ({
  name,
  run: (t) => {
    const hit = t.deliveries.find((d) => re.test(d));
    return hit ? { pass: false, detail: `matched: "${clip(hit)}"` } : { pass: true };
  },
});

export const deliveryCountBetween = (min: number, max: number): Check => ({
  name: `between ${min} and ${max} owner messages`,
  run: (t) =>
    t.deliveries.length >= min && t.deliveries.length <= max
      ? { pass: true, detail: `${t.deliveries.length} messages` }
      : { pass: false, detail: `${t.deliveries.length} messages: ${list(t.deliveries, 5)}` },
});

export const firstDelivery = (re: RegExp, name = `first message matches ${re}`): Check => ({
  name,
  run: (t) => {
    const first = t.deliveries[0];
    if (first === undefined) return { pass: false, detail: "nothing was delivered" };
    return re.test(first) ? { pass: true, detail: `"${clip(first)}"` } : { pass: false, detail: `"${clip(first)}"` };
  },
});

export const firstDeliveryNot = (re: RegExp, name = `first message avoids ${re}`): Check => ({
  name,
  run: (t) => {
    const first = t.deliveries[0];
    if (first === undefined) return { pass: false, detail: "nothing was delivered" };
    return re.test(first) ? { pass: false, detail: `"${clip(first)}"` } : { pass: true, detail: `"${clip(first)}"` };
  },
});

/** The owner never hears shop talk: workers, subagents, ids, tool mechanics (persona: outcomes only). */
export const noShopTalk = (): Check => ({
  name: "no shop talk to the owner",
  run: (t) => {
    const re = /\b(sub-?agents?|workers?|codex|orchestrat\w*|mcp|spawn\w*|w\d{1,3}\b)/i;
    const hit = t.deliveries.find((d) => re.test(d));
    return hit ? { pass: false, detail: `"${clip(hit)}"` } : { pass: true };
  },
});

/** Harness invariant + persona floor: nothing empty, no leaked NO_REPLY sentinel. */
export const wellFormedDeliveries = (): Check => ({
  name: "deliveries well-formed (non-empty, no NO_REPLY leak)",
  run: (t) => {
    const bad = t.deliveries.find((d) => d.trim() === "" || /NO_REPLY/.test(d));
    return bad === undefined ? { pass: true } : { pass: false, detail: `"${clip(bad)}"` };
  },
});

// ---- model-level reply discipline (conversation log, pre host gating) --------

const assistantTexts = (t: EvalTranscript): string[] =>
  t.conversation
    .filter((m) => m.role === "assistant")
    .flatMap((m) => m.content)
    .map((b) => (b.type === "text" && typeof b["text"] === "string" ? (b["text"] as string) : undefined))
    .filter((x): x is string => x !== undefined);

/** The model itself chose silence at least once (replied with the NO_REPLY sentinel). */
export const choseSilence = (): Check => ({
  name: "model chose NO_REPLY on a noise event",
  run: (t) => {
    const silent = assistantTexts(t).filter((x) => applyNoReply(x) === "");
    return silent.length > 0
      ? { pass: true, detail: `${silent.length} silent turn(s)` }
      : { pass: false, detail: "the model never replied NO_REPLY" };
  },
});

// ---- workspace (the REAL end state real workers left behind) -------------------

/** Run a command in the trial workspace; pass = exit 0. The honest functional grader: the work
 *  either runs or it doesn't. */
const runInWorkspace = (
  t: EvalTranscript,
  argv: string[],
  timeoutMs: number,
): { pass: boolean; detail?: string } => {
  try {
    // NODE_TEST_CONTEXT must not leak: when graders run under `node --test` (the unit tests do),
    // an inherited value makes a nested `node --test` exit 0 even on failures.
    const { NODE_TEST_CONTEXT: _ignored, ...env } = process.env;
    execFileSync(argv[0]!, argv.slice(1), {
      cwd: t.workspaceDir,
      timeout: timeoutMs,
      stdio: "pipe",
      env: { ...env, NODE_OPTIONS: "" },
    });
    return { pass: true };
  } catch (err) {
    const e = err as { stdout?: Buffer; stderr?: Buffer; message?: string };
    const out = `${e.stdout?.toString() ?? ""}\n${e.stderr?.toString() ?? ""}`.trim() || (e.message ?? "failed");
    return { pass: false, detail: clip(out.replace(/\s+/g, " "), 220) };
  }
};

/** The app's own test suite is green in the final workspace. */
export const testsGreen = (name = "app test suite is green (node --test)"): Check => ({
  name,
  run: (t) => runInWorkspace(t, [process.execPath, "--test"], 60_000),
});

/** Boot the app's server (require("./server.js")) and assert an HTTP status for a path.
 *  `init` extends the probe beyond a bare GET: a method + JSON body (POST probes) and/or a
 *  body-content assertion (`bodyMatches`). The name is computed from `init` when not given. */
export const httpProbe = (
  path: string,
  expectStatus: number,
  name?: string,
  init?: { method?: string; body?: string; bodyMatches?: RegExp },
): Check => ({
  name: name ?? `${init?.method ?? "GET"} ${path} → ${expectStatus}`,
  run: (t) => {
    const fetchInit =
      `{ method: ${JSON.stringify(init?.method ?? "GET")}` +
      (init?.body !== undefined
        ? `, headers: { "content-type": "application/json" }, body: ${JSON.stringify(init.body)}`
        : "") +
      ` }`;
    const bodyCheck = init?.bodyMatches
      ? `if (!${init.bodyMatches.toString()}.test(text)) { console.error("body mismatch: " + text.slice(0, 200)); process.exit(1); }`
      : "";
    const script = `
const server = require("./server.js");
server.listen(0, async () => {
  try {
    const res = await fetch("http://127.0.0.1:" + server.address().port + ${JSON.stringify(path)}, ${fetchInit});
    const text = await res.text();
    if (res.status !== ${expectStatus}) { console.error("status " + res.status + ": " + text.slice(0, 200)); process.exit(1); }
    ${bodyCheck}
    process.exit(0);
  } catch (err) { console.error(err.message); process.exit(1); }
});
setTimeout(() => { console.error("probe timeout"); process.exit(1); }, 8000);`;
    return runInWorkspace(t, [process.execPath, "-e", script], 15_000);
  },
});

/** Run an arbitrary Node script in the trial workspace; exit 0 = pass. For multi-step functional
 *  probes a single httpProbe can't express (POST → GET → assert round-trips). The script is
 *  responsible for its own inner timeout; `process.exit` explicitly. */
export const workspaceScript = (name: string, script: string, timeoutMs = 20_000): Check => ({
  name,
  run: (t) => runInWorkspace(t, [process.execPath, "-e", script], timeoutMs),
});

/** A workspace file exists and matches. */
export const workspaceFileMatches = (rel: string, re: RegExp, name = `${rel} matches ${re}`): Check => ({
  name,
  run: (t) => {
    const abs = join(t.workspaceDir, rel);
    if (!existsSync(abs)) return { pass: false, detail: `${rel} does not exist` };
    const body = readFileSync(abs, "utf8");
    return re.test(body)
      ? { pass: true, detail: rel }
      : { pass: false, detail: `${rel} exists but does not match` };
  },
});

// ---- workers ------------------------------------------------------------------

/** Heuristic: does this dispatched objective read like a validation/verification one? (Applied to
 *  protocol-stripped prompts — the manager's own words.) */
export function looksLikeValidation(prompt: string): boolean {
  return /\b(verif\w*|validat\w*|PASS or FAIL|screenshots?|confirm (?:that|the)|check (?:that|whether))\b/i.test(
    prompt,
  );
}

export const workersAtLeast = (n: number): Check => ({
  name: `dispatched ≥${n} worker run(s)`,
  run: (t) =>
    t.workerSessions.length >= n
      ? { pass: true, detail: `${t.workerSessions.length} runs` }
      : { pass: false, detail: `${t.workerSessions.length} runs: ${list(t.workerSessions.map((s) => s.prompt))}` },
});

export const noWorkers = (): Check => ({
  name: "no worker dispatched",
  run: (t) =>
    t.workerSessions.length === 0
      ? { pass: true }
      : { pass: false, detail: `${t.workerSessions.length} dispatched: ${list(t.workerSessions.map((s) => s.prompt))}` },
});

export const workerPromptMatching = (re: RegExp, name = `some worker prompt matches ${re}`): Check => ({
  name,
  run: (t) => {
    const hit = t.workerSessions.find((s) => re.test(s.prompt));
    return hit
      ? { pass: true, detail: `"${clip(hit.prompt)}"` }
      : { pass: false, detail: `no prompt matched among ${t.workerSessions.length}` };
  },
});

export const noWorkerPromptMatching = (re: RegExp, name = `no worker prompt matches ${re}`): Check => ({
  name,
  run: (t) => {
    const hit = t.workerSessions.find((s) => re.test(s.prompt));
    return hit ? { pass: false, detail: `"${clip(hit.prompt)}"` } : { pass: true };
  },
});

/** ≥n workers were STARTED in the very first manager turn (true parallel split, not drip-fed). */
export const parallelStartsInFirstTurn = (n: number): Check => ({
  name: `≥${n} workers started in the first turn`,
  run: (t) => {
    const starts = t.workerPrompts.filter((p) => p.kind === "start" && p.turnId === 1);
    return starts.length >= n
      ? { pass: true, detail: `${starts.length} parallel starts` }
      : { pass: false, detail: `${starts.length} start(s) in turn 1` };
  },
});

// ---- memory --------------------------------------------------------------------

export const memoryContains = (query: string, name = `memory contains "${query}"`): Check => ({
  name,
  run: (t) => {
    const hits = t.mem.search(query);
    return hits.length > 0
      ? { pass: true, detail: hits[0]!.path }
      : { pass: false, detail: `FTS found nothing; tree:\n${clip(t.mem.tree(), 300)}` };
  },
});

// ---- ordering (timeline) --------------------------------------------------------

type EntryPred = (e: TimelineEntry) => boolean;

export const workerDoneMatching =
  (re: RegExp): EntryPred =>
  (e) =>
    e.type === "worker_done" && re.test(e.response);

export const anyWorkerCall: EntryPred = (e) => e.type === "worker_call";

/** No delivery matching `re` happens before the first timeline entry satisfying `before`.
 *  The workhorse for "don't tell the owner it's done until the validator PASSed". */
export const noDeliveryUntil = (re: RegExp, before: EntryPred, name: string): Check => ({
  name,
  run: (t) => {
    const gate = t.timeline.find(before);
    const gateSeq = gate?.seq ?? Number.POSITIVE_INFINITY;
    const early = t.timeline.find((e) => e.type === "delivery" && e.seq < gateSeq && re.test(e.text));
    if (early && early.type === "delivery") return { pass: false, detail: `too early: "${clip(early.text)}"` };
    return { pass: true, ...(gate ? { detail: `gate at seq ${gate.seq}` } : { detail: "gate never occurred (and no early delivery)" }) };
  },
});

/** Scope an assertion to the events caused by the nth owner message (1-based): the timeline window
 *  between that owner_msg and the next one (or the end). The long-horizon workhorse — "turn 6 was
 *  answered from memory without dispatching a worker" — without coupling to absolute seq numbers. */
export const inTurnWindow = (
  n: number,
  name: string,
  run: (window: TimelineEntry[], t: EvalTranscript) => CheckOutcome,
): Check => ({
  name,
  run: (t) => {
    const ownerSeqs = t.timeline.filter((e) => e.type === "owner_msg").map((e) => e.seq);
    const start = ownerSeqs[n - 1];
    if (start === undefined) {
      return { pass: false, detail: `only ${ownerSeqs.length} owner turn(s) in the timeline (wanted #${n})` };
    }
    const end = ownerSeqs[n] ?? Number.POSITIVE_INFINITY;
    return run(t.timeline.filter((e) => e.seq > start && e.seq < end), t);
  },
});

// ---- token / effort efficiency ----------------------------------------------------

/** Soft budget (non-gating by default): same outcome in fewer manager turns / worker runs is
 *  better. Evals exist to measure the non-deterministic stuff — efficiency is part of "behaving
 *  well", so bloat shaves the score without failing the scenario. */
export const usageWithin = (
  budget: { managerTurns?: number; workerRuns?: number },
  name?: string,
): Check => ({
  name:
    name ??
    `efficient: ${[
      budget.managerTurns !== undefined ? `≤${budget.managerTurns} manager turns` : undefined,
      budget.workerRuns !== undefined ? `≤${budget.workerRuns} worker runs` : undefined,
    ]
      .filter(Boolean)
      .join(", ")}`,
  required: false,
  run: (t) => {
    const over: string[] = [];
    if (budget.managerTurns !== undefined && t.usage.managerTurns > budget.managerTurns) {
      over.push(`manager turns ${t.usage.managerTurns} > ${budget.managerTurns}`);
    }
    if (budget.workerRuns !== undefined && t.workerSessions.length > budget.workerRuns) {
      over.push(`worker runs ${t.workerSessions.length} > ${budget.workerRuns}`);
    }
    return over.length === 0
      ? { pass: true, detail: `${t.usage.managerTurns} manager turns, ${t.workerSessions.length} worker runs` }
      : { pass: false, detail: over.join("; ") };
  },
});

// ---- escape hatch ----------------------------------------------------------------

export const custom = (name: string, run: Check["run"], opts: { required?: boolean; weight?: number } = {}): Check => ({
  name,
  run,
  ...(opts.required !== undefined ? { required: opts.required } : {}),
  ...(opts.weight !== undefined ? { weight: opts.weight } : {}),
});

/** The defaults every scenario gets prepended (cheap global invariants). */
export const baselineChecks = (): Check[] => [wellFormedDeliveries(), noShopTalk()];
