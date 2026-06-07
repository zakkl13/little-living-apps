// Scripted ManagerModel (DESIGN §13): returns predetermined tool_use / text / compaction blocks so
// we drive deterministic manager behavior and assert the loop's contracts — notably that the full
// assistant content (including `compaction` blocks) round-trips back into the next request.
//
// Every request the manager builds is recorded (deep-copied) so tests can inspect exactly what was
// sent to the model. No network, no real SDK.

import type {
  Block,
  ManagerModel,
  ManagerRequest,
  ManagerResponse,
} from "../../src/manager/anthropic.js";

export type ScriptStep =
  | ManagerResponse
  | ((req: ManagerRequest, index: number) => ManagerResponse);

export interface FakeAnthropic extends ManagerModel {
  /** Deep-copied snapshot of every request the manager sent. */
  readonly requests: ManagerRequest[];
  /** Append more scripted steps (consumed FIFO, one per createMessage call). */
  push(...steps: ScriptStep[]): void;
  readonly pending: number;
}

export function makeFakeAnthropic(initial: ScriptStep[] = []): FakeAnthropic {
  const steps: ScriptStep[] = [...initial];
  const requests: ManagerRequest[] = [];
  let calls = 0;

  const fake: FakeAnthropic = {
    requests,
    push(...more) {
      steps.push(...more);
    },
    get pending() {
      return steps.length;
    },
    async createMessage(req) {
      requests.push(JSON.parse(JSON.stringify(req)) as ManagerRequest);
      const step = steps.shift();
      if (step === undefined) {
        throw new Error(`fakeAnthropic: no scripted response for call #${calls + 1}`);
      }
      const out = typeof step === "function" ? step(req, calls) : step;
      calls += 1;
      return out;
    },
  };
  return fake;
}

// ---- block + response builders ---------------------------------------------

let idSeq = 0;
function nextId(prefix: string): string {
  idSeq += 1;
  return `${prefix}_${idSeq}`;
}

export function text(s: string): Block {
  return { type: "text", text: s };
}

export function toolUse(name: string, input: Record<string, unknown> = {}, id?: string): Block {
  return { type: "tool_use", id: id ?? nextId("toolu"), name, input };
}

/** An opaque compaction block — the manager must pass it back verbatim (DESIGN §4). */
export function compaction(id?: string): Block {
  return { type: "compaction", id: id ?? nextId("cmp"), summary: "…earlier context summarized…" };
}

/** Build a response; stop_reason is inferred (tool_use if any tool_use block, else end_turn). */
export function resp(blocks: Block[], stopReason?: string): ManagerResponse {
  const hasToolUse = blocks.some((b) => b.type === "tool_use");
  return {
    content: blocks,
    stopReason: stopReason ?? (hasToolUse ? "tool_use" : "end_turn"),
    usage: { inputTokens: 100, outputTokens: 20 },
  };
}
