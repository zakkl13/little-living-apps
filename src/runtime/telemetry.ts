// Telemetry — the passive recorder behind the Inspector (read-only observability plane). The runtime
// writes to it at seams that already exist (per-turn token usage, worker-prompt launches); the
// Inspector HTTP server reads from it. It NEVER mutates runtime state and is NEVER a model tool — the
// manager's "no hands" capability boundary stays airtight. When the Inspector is off, nothing
// constructs this and the seams no-op.
//
// Everything now rides the one ChatGPT subscription (manager thread + workers), so there is no metered
// dollar plane to track — we record TOKEN USAGE only (input / cached-input / output / reasoning, the
// four counters Codex reports per turn). History is bounded in-memory ring buffers (the live thread is
// the source of truth for the current conversation). Only the cumulative usage meter is durable:
// app.ts folds usageSnapshot() into the crash snapshot so lifetime token totals survive a restart.

import type { ManagerEvent } from "./eventQueue.js";
import type { ConvMessage } from "../manager/driver.js";

export type TurnKind = ManagerEvent["kind"];
export type PromptKind = "start" | "send" | "steer" | "cancel";

/** The four token counters Codex reports for a turn (turn.completed.usage). cached/reasoning are
 *  optional at the call site so a partial usage still records cleanly. */
export interface TokenUsage {
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens?: number;
  reasoningTokens?: number;
}

/** One manager turn's metadata + token usage. The conversation content itself is read live from the
 *  reconstructed log; this is the per-turn envelope the trace panel joins against by turnId. */
export interface TurnRecord {
  turnId: number;
  kind: TurnKind;
  /** The owner text (owner_message) or the rendered worker event (worker_event) that opened the turn. */
  request: string;
  chatId: number;
  startedAt: number;
  endedAt?: number;
  /** Model round-trips inside the turn (one turn.completed per streamed Codex turn). */
  iterations: number;
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number;
  reasoningTokens: number;
}

/** One Codex prompt the manager dispatched, stamped with the originating turn so a user request can
 *  be traced all the way to what a worker actually received. */
export interface PromptRecord {
  ts: number;
  turnId: number;
  workerId: string;
  kind: PromptKind;
  prompt: string;
}

/** Cumulative token usage across the manager thread's life. No dollars — it's all the subscription;
 *  codexTurns is a plain count of worker launches. */
export interface UsageMeter {
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reasoningTokens: number;
  managerTurns: number;
  codexTurns: number;
}

/** The slice of the meter persisted across restarts (identical to the meter — all counters are durable). */
export type UsageSnapshot = UsageMeter;

export interface Telemetry {
  // ---- recording (called from runtime seams) ----
  beginTurn(turnId: number, kind: TurnKind, request: string, chatId: number): void;
  /** One streamed turn's usage; called per turn.completed via the driver's onUsage hook. */
  recordUsage(turnId: number, usage: TokenUsage): void;
  endTurn(turnId: number): void;
  recordPrompt(rec: Omit<PromptRecord, "ts">): void;
  /** Append a reconstructed conversation entry (from the Codex item stream) for the Inspector. */
  recordConversation(message: ConvMessage): void;

  // ---- reading (called from the Inspector server) ----
  turns(): TurnRecord[];
  /** The reconstructed conversation log (bounded ring buffer; not persisted). */
  conversation(): ConvMessage[];
  prompts(filter?: { turnId?: number; workerId?: string }): PromptRecord[];
  meter(): UsageMeter;
  /** Best estimate of "tokens currently in the conversation": the input size of the most recent
   *  model call (what Codex last carried as context). */
  contextTokens(): number;

  // ---- durability (folded into the crash snapshot by app.ts) ----
  usageSnapshot(): UsageSnapshot;
  loadUsage(snap: UsageSnapshot): void;
}

export interface TelemetryOptions {
  /** Ring-buffer caps. */
  maxTurns?: number;
  maxPrompts?: number;
  maxConversation?: number;
}

const DEFAULT_MAX_TURNS = 500;
const DEFAULT_MAX_PROMPTS = 1000;
const DEFAULT_MAX_CONVERSATION = 400;

export function createTelemetry(opts: TelemetryOptions = {}): Telemetry {
  const maxTurns = opts.maxTurns ?? DEFAULT_MAX_TURNS;
  const maxPrompts = opts.maxPrompts ?? DEFAULT_MAX_PROMPTS;
  const maxConversation = opts.maxConversation ?? DEFAULT_MAX_CONVERSATION;

  // Insertion-ordered map (JS preserves order) so eviction = delete the first key.
  const turnMap = new Map<number, TurnRecord>();
  const promptLog: PromptRecord[] = [];
  const convLog: ConvMessage[] = [];

  let inputTokens = 0;
  let cachedInputTokens = 0;
  let outputTokens = 0;
  let reasoningTokens = 0;
  let managerTurns = 0;
  let codexTurns = 0;
  let lastContextTokens = 0;

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
        cachedInputTokens: 0,
        reasoningTokens: 0,
      });
      managerTurns += 1;
      evictTurns();
    },

    recordUsage(turnId, usage) {
      const cached = usage.cachedInputTokens ?? 0;
      const reasoning = usage.reasoningTokens ?? 0;
      inputTokens += usage.inputTokens;
      cachedInputTokens += cached;
      outputTokens += usage.outputTokens;
      reasoningTokens += reasoning;
      lastContextTokens = usage.inputTokens; // the most recent call's context size
      const rec = turnMap.get(turnId);
      if (rec) {
        rec.iterations += 1;
        rec.inputTokens += usage.inputTokens;
        rec.outputTokens += usage.outputTokens;
        rec.cachedInputTokens += cached;
        rec.reasoningTokens += reasoning;
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

    recordConversation(message) {
      convLog.push(message);
      if (convLog.length > maxConversation) convLog.splice(0, convLog.length - maxConversation);
    },

    turns: () => [...turnMap.values()],
    conversation: () => [...convLog],
    prompts(filter) {
      let rows = promptLog;
      if (filter?.turnId !== undefined) rows = rows.filter((p) => p.turnId === filter.turnId);
      if (filter?.workerId !== undefined) rows = rows.filter((p) => p.workerId === filter.workerId);
      return [...rows];
    },
    meter: () => ({
      inputTokens,
      cachedInputTokens,
      outputTokens,
      reasoningTokens,
      managerTurns,
      codexTurns,
    }),
    contextTokens: () => lastContextTokens,

    usageSnapshot: () => ({
      inputTokens,
      cachedInputTokens,
      outputTokens,
      reasoningTokens,
      managerTurns,
      codexTurns,
    }),
    loadUsage(snap) {
      inputTokens = snap.inputTokens;
      cachedInputTokens = snap.cachedInputTokens;
      outputTokens = snap.outputTokens;
      reasoningTokens = snap.reasoningTokens;
      managerTurns = snap.managerTurns;
      codexTurns = snap.codexTurns;
    },
  };
}
