// Fake manager backend — the seam the real Codex thread sits behind. Since a fake can't make a real
// Codex thread call our MCP server, we instead let each scripted "turn" act DIRECTLY against the live
// MemFs + Orchestrator through the real Lila MCP tool handlers (ctx.call), and reply to the owner via
// ctx.say (which honors NO_REPLY and records the conversation, exactly like a real agent_message).
// This drives the full runtime — loop, queue, persistence, orchestration, telemetry, delivery — with
// everything real except the model itself. The ManagerDriver against a *real* fake thread is tested
// separately in driver.test.ts.

import { applyNoReply, type ConvMessage, type DeliverFn, type ManagerDriver, type ManagerUsage, type TurnInput } from "../../src/manager/driver.js";
import type { ManagerBackend, ManagerBackendCtx, ManagerBackendFactory } from "../../src/manager/backend.js";
import { lilaTools } from "../../src/manager/mcp/tools.js";
import type { MemFs } from "../../src/memory/memfs.js";
import type { Orchestrator } from "../../src/workers/types.js";

export interface ManagerStepCtx {
  input: TurnInput;
  chatId: number;
  turnId: number;
  mem: MemFs;
  orchestrator: Orchestrator;
  /** Reply to the owner (honors NO_REPLY; records the conversation) — mirrors an agent_message. */
  say(text: string): Promise<void>;
  /** Invoke a real Lila MCP tool handler (memory or subagent); returns the joined text result. */
  call(tool: string, args?: Record<string, unknown>): Promise<string>;
  /** Override the per-turn token usage (defaults are recorded automatically otherwise). */
  recordUsage(usage: Partial<ManagerUsage>): void;
}

export type ManagerStep = (ctx: ManagerStepCtx) => void | Promise<void>;

export interface FakeManager {
  factory: ManagerBackendFactory;
  push(...steps: ManagerStep[]): void;
  readonly turns: number;
}

const DEFAULT_USAGE: ManagerUsage = {
  inputTokens: 100,
  outputTokens: 20,
  cachedInputTokens: 0,
  reasoningTokens: 0,
};

export function makeFakeManager(initial: ManagerStep[] = []): FakeManager {
  const steps: ManagerStep[] = [...initial];
  let turns = 0;

  const factory: ManagerBackendFactory = async (backendCtx: ManagerBackendCtx): Promise<ManagerBackend> => {
    const { mem, orchestrator, telemetry, deliver } = backendCtx;
    let turnId = 0;
    let pendingResumeId: string | undefined;
    let currentThreadId: string | undefined;
    let counter = 0;

    const tools = lilaTools({
      mem,
      orchestrator: orchestrator as Orchestrator,
      telemetry,
      currentTurnId: () => turnId,
    });
    const toolByName = new Map(tools.map((t) => [t.name, t]));

    const driver: ManagerDriver = {
      threadId: () => currentThreadId,
      adoptThreadId: (id) => {
        pendingResumeId = id;
        currentThreadId = id;
      },
      reset: () => {
        pendingResumeId = undefined;
        currentThreadId = undefined;
      },
      async runTurn(input: TurnInput, chatId, opts) {
        // Lazily "start" or "resume" a thread id, mirroring the real driver.
        if (!currentThreadId) currentThreadId = pendingResumeId ?? `thread-fake-${++counter}`;
        pendingResumeId = undefined;
        turns += 1;

        // Record the user turn opener (the real driver does this from the input).
        opts?.onConversation?.({
          role: "user",
          content: [
            { type: "text", text: input.text },
            ...(input.imagePath ? [{ type: "image", path: input.imagePath }] : []),
          ],
        });

        let usage = DEFAULT_USAGE;
        const ctx: ManagerStepCtx = {
          input,
          chatId,
          turnId,
          mem,
          orchestrator: orchestrator as Orchestrator,
          async say(text) {
            opts?.onConversation?.({ role: "assistant", content: [{ type: "text", text }] });
            const reply = applyNoReply(text);
            if (reply && (opts?.allowReply?.() ?? true)) await deliver(chatId, reply);
          },
          async call(tool, args = {}) {
            const t = toolByName.get(tool);
            if (!t) throw new Error(`fakeManager: unknown tool ${tool}`);
            const reply = await t.handler(args);
            opts?.onConversation?.({
              role: "assistant",
              content: [{ type: "tool_use", name: `lila.${tool}`, input: args }],
            });
            const resultText = reply.content.map((c) => c.text).join("\n");
            opts?.onConversation?.({ role: "user", content: [{ type: "tool_result", content: resultText }] });
            return resultText;
          },
          recordUsage(partial) {
            usage = { ...usage, ...partial };
          },
        };

        const step = steps.shift();
        if (step) await step(ctx);
        opts?.onUsage?.(usage); // mirror turn.completed always firing
      },
    };

    return { driver, setActiveTurn: (id) => (turnId = id), close: async () => {} };
  };

  return {
    factory,
    push: (...more) => steps.push(...more),
    get turns() {
      return turns;
    },
  };
}

// ---- tiny step builders (read like the old resp/toolUse DSL) ----------------

/** A turn that simply replies to the owner with `text` (or stays silent on NO_REPLY). */
export const say = (text: string): ManagerStep => (ctx) => ctx.say(text);

/** A turn that spawns one or more scoped workers, then acks. */
export const startWorkers = (
  specs: Array<{ objective: string; project?: string }>,
  ack?: string,
): ManagerStep => async (ctx) => {
  for (const s of specs) await ctx.call("subagent_start", s as Record<string, unknown>);
  if (ack) await ctx.say(ack);
};
