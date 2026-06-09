// Telemetry — the passive recorder behind the Inspector (read-only observability plane). The runtime
// writes to it at seams that already exist (per-turn token usage, worker-prompt launches); the
// Inspector HTTP server reads from it. It NEVER mutates runtime state and is NEVER a model tool — the
// manager's "no hands" capability boundary stays airtight (registry.ts). When the Inspector is off,
// nothing constructs this and the seams no-op.
//
// History is bounded in-memory ring buffers (the live transcript is the source of truth for the
// current conversation, so we don't keep turns forever). Only the cumulative cost meter is durable:
// app.ts folds costSnapshot() into the crash snapshot so lifetime token/$ totals survive a restart.

import type { ManagerEvent } from "./eventQueue.js";

export type TurnKind = ManagerEvent["kind"];
export type PromptKind = "start" | "send" | "steer" | "cancel";

/** One manager turn's metadata + cost. The conversation content itself is read live from the
 *  transcript; this is the per-turn envelope the trace and cost panels join against by turnId. */
export interface TurnRecord {
  turnId: number;
  kind: TurnKind;
  /** The owner text (owner_message) or the rendered worker event (worker_event) that opened the turn. */
  request: string;
  chatId: number;
  startedAt: number;
  endedAt?: number;
  /** Model round-trips inside the turn (one per createMessage call). */
  iterations: number;
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
}

/** One Codex prompt the manager dispatched, stamped with the originating turn so a user request can
 *  be traced all the way to what a worker actually received (requirements 3 + 4). */
export interface PromptRecord {
  ts: number;
  turnId: number;
  workerId: string;
  kind: PromptKind;
  prompt: string;
}

/** Cumulative spend. Codex workers ride the ChatGPT subscription, so they carry no metered $ — we
 *  track their turn count for visibility but the dollars are all manager (Anthropic) tokens. */
export interface CostMeter {
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  managerTurns: number;
  codexTurns: number;
}

/** The slice of the meter that is persisted across restarts (dollars are recomputed from price). */
export interface CostSnapshot {
  inputTokens: number;
  outputTokens: number;
  managerTurns: number;
  codexTurns: number;
}

export interface Telemetry {
  // ---- recording (called from runtime seams) ----
  beginTurn(turnId: number, kind: TurnKind, request: string, chatId: number): void;
  /** One createMessage round-trip's usage; called per iteration via the manager's onUsage hook. */
  recordUsage(turnId: number, usage: { inputTokens: number; outputTokens: number }): void;
  endTurn(turnId: number): void;
  recordPrompt(rec: Omit<PromptRecord, "ts">): void;

  // ---- reading (called from the Inspector server) ----
  turns(): TurnRecord[];
  prompts(filter?: { turnId?: number; workerId?: string }): PromptRecord[];
  meter(): CostMeter;
  /** Best estimate of "tokens currently in the conversation": the input size of the most recent
   *  model call (what the API last charged to carry the whole context). */
  contextTokens(): number;
  priceInPerMTok: number;
  priceOutPerMTok: number;

  // ---- durability (folded into the crash snapshot by app.ts) ----
  costSnapshot(): CostSnapshot;
  loadCost(snap: CostSnapshot): void;
}

export interface TelemetryOptions {
  /** $ per million input tokens for the manager model (nominal; env-overridable). */
  priceInPerMTok: number;
  /** $ per million output tokens. */
  priceOutPerMTok: number;
  /** Ring-buffer caps. */
  maxTurns?: number;
  maxPrompts?: number;
}

const DEFAULT_MAX_TURNS = 500;
const DEFAULT_MAX_PROMPTS = 1000;

export function createTelemetry(opts: TelemetryOptions): Telemetry {
  const maxTurns = opts.maxTurns ?? DEFAULT_MAX_TURNS;
  const maxPrompts = opts.maxPrompts ?? DEFAULT_MAX_PROMPTS;

  // Insertion-ordered map (JS preserves order) so eviction = delete the first key.
  const turnMap = new Map<number, TurnRecord>();
  const promptLog: PromptRecord[] = [];

  let inputTokens = 0;
  let outputTokens = 0;
  let managerTurns = 0;
  let codexTurns = 0;
  let lastContextTokens = 0;

  const dollars = (inTok: number, outTok: number): number =>
    (inTok / 1_000_000) * opts.priceInPerMTok + (outTok / 1_000_000) * opts.priceOutPerMTok;

  function evictTurns(): void {
    while (turnMap.size > maxTurns) {
      const oldest = turnMap.keys().next().value;
      if (oldest === undefined) break;
      turnMap.delete(oldest);
    }
  }

  return {
    beginTurn(turnId, kind, request, chatId) {
      turnMap.set(turnId, {
        turnId,
        kind,
        request,
        chatId,
        startedAt: Date.now(),
        iterations: 0,
        inputTokens: 0,
        outputTokens: 0,
        costUsd: 0,
      });
      managerTurns += 1;
      evictTurns();
    },

    recordUsage(turnId, usage) {
      inputTokens += usage.inputTokens;
      outputTokens += usage.outputTokens;
      lastContextTokens = usage.inputTokens; // the most recent call's context size
      const rec = turnMap.get(turnId);
      if (rec) {
        rec.iterations += 1;
        rec.inputTokens += usage.inputTokens;
        rec.outputTokens += usage.outputTokens;
        rec.costUsd = dollars(rec.inputTokens, rec.outputTokens);
      }
    },

    endTurn(turnId) {
      const rec = turnMap.get(turnId);
      if (rec) rec.endedAt = Date.now();
    },

    recordPrompt(rec) {
      if (rec.kind !== "cancel") codexTurns += 1;
      promptLog.push({ ...rec, ts: Date.now() });
      if (promptLog.length > maxPrompts) promptLog.splice(0, promptLog.length - maxPrompts);
    },

    turns: () => [...turnMap.values()],
    prompts(filter) {
      let rows = promptLog;
      if (filter?.turnId !== undefined) rows = rows.filter((p) => p.turnId === filter.turnId);
      if (filter?.workerId !== undefined) rows = rows.filter((p) => p.workerId === filter.workerId);
      return [...rows];
    },
    meter: () => ({
      inputTokens,
      outputTokens,
      costUsd: dollars(inputTokens, outputTokens),
      managerTurns,
      codexTurns,
    }),
    contextTokens: () => lastContextTokens,
    priceInPerMTok: opts.priceInPerMTok,
    priceOutPerMTok: opts.priceOutPerMTok,

    costSnapshot: () => ({ inputTokens, outputTokens, managerTurns, codexTurns }),
    loadCost(snap) {
      inputTokens = snap.inputTokens;
      outputTokens = snap.outputTokens;
      managerTurns = snap.managerTurns;
      codexTurns = snap.codexTurns;
    },
  };
}
