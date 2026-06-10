// The ManagerDriver (MIGRATION-CODEX.md §4): one manager turn, now over a long-lived Codex thread
// instead of an Anthropic createMessage loop. Pop an event → prepend the volatile context header →
// runStreamed → stream items: the final `agent_message` is the manager's reply to the owner (honoring
// the NO_REPLY sentinel); `mcp_tool_call` is internal (memory/orchestration) and only logged; `reasoning`
// is private and never delivered. The async worker model is unchanged: subagent_start (an MCP tool)
// returns immediately, the turn ends with an ack, and the worker's completion re-enters as an event.
//
// Durability is simpler than the Anthropic loop: Codex owns the thread's rollout on disk and runs its
// own compaction, so we keep no ModelMessage transcript — only the thread id, to resume after a
// restart. The same thread object is reused across turns in one process; resume() is for cold wake.

import type { ThreadEvent, UserInput } from "@openai/codex-sdk";

import type { ManagerThread, ManagerThreadFactory } from "./managerCodex.js";
import { friendlyError } from "../workers/runner.js";
import { logger } from "../logger.js";

/** Delivers the manager's user-facing text to the owner's chat (wraps Telegram sendMessage). */
export type DeliverFn = (chatId: number, text: string) => Promise<void>;

/** A turn emits this to absorb an event without messaging the owner (reused from the v0.2 loop).
 *  Suppresses the WHOLE message wherever it appears on its own line, so private reasoning that leads
 *  up to a decision to stay silent is never delivered. */
export const NO_REPLY = "NO_REPLY";

/** What opens a turn: an owner message or rendered worker event, optionally with an owner-sent image. */
export interface TurnInput {
  text: string;
  /** Local path to an owner-sent image (view_image is on); becomes a `local_image` input. */
  imagePath?: string;
}

export interface ManagerUsage {
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number;
  reasoningTokens: number;
}

/** A normalized conversation entry for the (off-by-default) Inspector — reconstructed from the Codex
 *  item stream since there is no longer a ModelMessage transcript. */
export interface ConvBlock {
  type: string;
  [key: string]: unknown;
}
export interface ConvMessage {
  role: "user" | "assistant";
  content: ConvBlock[];
}

export interface RunTurnOpts {
  /** Token usage for this turn (from turn.completed). */
  onUsage?: (usage: ManagerUsage) => void;
  /** Observability sink for the reconstructed conversation log (Inspector). */
  onConversation?: (message: ConvMessage) => void;
  /** Host-side delivery gate. Conversation is still recorded; owner-visible sends can be suppressed. */
  allowReply?: () => boolean;
}

export interface ManagerDriverDeps {
  factory: ManagerThreadFactory;
  deliver: DeliverFn;
  /** Builds the volatile per-turn header (core memory + index), prepended to each event's input. */
  buildContextHeader: () => string;
}

export interface ManagerDriver {
  runTurn(input: TurnInput, chatId: number, opts?: RunTurnOpts): Promise<void>;
  /** The current manager thread id, for snapshotting (undefined before the first turn). */
  threadId(): string | undefined;
  /** Seed the resume id from a restored snapshot (used before the first turn). */
  adoptThreadId(id: string | undefined): void;
  /** /new: drop the thread so the next turn starts fresh (working context cleared, memory kept). */
  reset(): void;
}

export function createManagerDriver(deps: ManagerDriverDeps): ManagerDriver {
  let thread: ManagerThread | null = null;
  let pendingResumeId: string | undefined;
  let currentThreadId: string | undefined;

  function ensureThread(): ManagerThread {
    if (!thread) {
      thread = pendingResumeId ? deps.factory.resume(pendingResumeId) : deps.factory.start();
    }
    return thread;
  }

  return {
    threadId: () => currentThreadId,
    adoptThreadId(id) {
      pendingResumeId = id;
      currentThreadId = id;
    },
    reset() {
      thread = null;
      pendingResumeId = undefined;
      currentThreadId = undefined;
    },

    async runTurn(input, chatId, opts) {
      const t = ensureThread();
      const header = deps.buildContextHeader();
      const text = header ? `${header}\n\n---\n\n${input.text}` : input.text;
      const codexInput: UserInput[] = [{ type: "text", text }];
      if (input.imagePath) codexInput.push({ type: "local_image", path: input.imagePath });

      opts?.onConversation?.({
        role: "user",
        content: [
          { type: "text", text: input.text },
          ...(input.imagePath ? [{ type: "image", path: input.imagePath }] : []),
        ],
      });

      let failure: string | undefined;
      let finalReply: string | undefined;
      try {
        const { events } = await t.runStreamed(codexInput);
        for await (const event of events) {
          handleEvent(
            event,
            opts,
            (f) => (failure = f),
            (text) => (finalReply = text),
          );
        }
      } catch (err) {
        failure = (err as Error).message;
      }

      // Capture the thread id once the turn has started it; resume is no longer needed this process.
      if (t.id) {
        currentThreadId = t.id;
        pendingResumeId = undefined;
      }

      if (failure) {
        await deps.deliver(chatId, friendlyError(failure));
        return;
      }

      if (finalReply !== undefined) {
        const reply = applyNoReply(finalReply);
        if (reply && (opts?.allowReply?.() ?? true)) await deps.deliver(chatId, reply);
      }
    },
  };
}

function handleEvent(
  event: ThreadEvent,
  opts: RunTurnOpts | undefined,
  setFailure: (f: string) => void,
  setFinalReply: (text: string) => void,
): void {
  switch (event.type) {
    case "item.completed": {
      const item = event.item;
      if (item.type === "agent_message") {
        opts?.onConversation?.({ role: "assistant", content: [{ type: "text", text: item.text }] });
        // The SDK's buffered `run()` treats the last completed agent message as `finalResponse`.
        // Keep streaming observability, but apply the same owner-delivery rule to avoid progress
        // chatter becoming many Telegram messages from a single turn.
        setFinalReply(item.text);
      } else if (item.type === "mcp_tool_call") {
        logger.debug("Manager tool call", { server: item.server, tool: item.tool, status: item.status });
        opts?.onConversation?.({
          role: "assistant",
          content: [
            {
              type: "tool_use",
              name: `${item.server}.${item.tool}`,
              input: item.arguments,
              status: item.status,
              ...(item.error?.message ? { error: item.error.message } : {}),
            },
          ],
        });
        const resultText = item.error?.message ? `error: ${item.error.message}` : mcpResultText(item.result);
        if (resultText !== undefined) {
          opts?.onConversation?.({ role: "user", content: [{ type: "tool_result", content: resultText }] });
        }
      } else if (item.type === "reasoning") {
        // Private: recorded for the Inspector, never delivered to the owner.
        opts?.onConversation?.({ role: "assistant", content: [{ type: "thinking", thinking: item.text }] });
      } else if (item.type === "error") {
        logger.warn("Manager turn surfaced an error item", { message: item.message });
      }
      break;
    }
    case "turn.completed":
      opts?.onUsage?.({
        inputTokens: event.usage.input_tokens,
        outputTokens: event.usage.output_tokens,
        cachedInputTokens: event.usage.cached_input_tokens,
        reasoningTokens: event.usage.reasoning_output_tokens,
      });
      break;
    case "turn.failed":
      setFailure(event.error?.message ?? "Manager turn failed.");
      break;
    case "error":
      setFailure(event.message);
      break;
    default:
      break;
  }
}

/** The user-facing text of an agent message, or "" when the model signaled silence (a NO_REPLY token
 *  on its own line anywhere in the message). */
export function applyNoReply(text: string): string {
  const silenced = text.split(/\r?\n/).some((line) => line.trim() === NO_REPLY);
  return silenced ? "" : text.trim();
}

function mcpResultText(result: { content: Array<{ type?: string; text?: string }> } | undefined): string | undefined {
  if (!result?.content) return undefined;
  return result.content
    .filter((b) => b.type === "text" && typeof b.text === "string")
    .map((b) => b.text)
    .join("\n");
}
