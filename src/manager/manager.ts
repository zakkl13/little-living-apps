// One manager turn (DESIGN §3, §4). Pop an event → append to the transcript → call the model in a
// tool loop (execute tool_use, append tool_result, call again) → end the turn when the model stops
// calling tools. The manager's plain `text` blocks ARE its reply to the owner: every iteration we
// deliver them straight to Telegram. Private reasoning lives in `thinking` blocks (never delivered);
// a turn stays silent by emitting only the NO_REPLY sentinel. The full response.content is appended
// VERBATIM every iteration, which is the compaction round-trip contract (compaction/thinking blocks
// pass through untouched) — delivery only reads text blocks, it never mutates content.

import {
  toolUses,
  type Block,
  type ManagerModel,
  type ModelMessage,
  type ToolResultBlock,
} from "./anthropic.js";
import type { ToolRegistry } from "./tools/registry.js";
import type { ManagerEvent } from "../runtime/eventQueue.js";
import { logger } from "../logger.js";

export interface Transcript {
  readonly messages: ModelMessage[];
  append(m: ModelMessage): void;
  snapshot(): ModelMessage[];
  load(messages: ModelMessage[]): void;
}

export function createTranscript(initial: ModelMessage[] = []): Transcript {
  const messages: ModelMessage[] = [...initial];
  return {
    messages,
    append(m) {
      messages.push(m);
    },
    snapshot() {
      // Shallow array copy; blocks are never mutated after append so refs are safe to share.
      return messages.map((m) => ({ role: m.role, content: m.content }));
    },
    load(next) {
      messages.length = 0;
      messages.push(...next);
    },
  };
}

/** Delivers the manager's user-facing text to the owner's chat (wraps Telegram sendMessage). */
export type DeliverFn = (chatId: number, text: string) => Promise<void>;

export interface TurnDeps {
  model: ManagerModel;
  modelName: string;
  registry: ToolRegistry;
  transcript: Transcript;
  /** The owner's reply channel: the manager's plain text is sent here (DESIGN §4, §9). */
  deliver: DeliverFn;
  /** Rebuilt fresh each iteration so memory edits made mid-turn are reflected immediately. */
  buildSystem: () => string;
  onUsage?: (usage: { inputTokens: number; outputTokens: number }) => void;
  maxIterations?: number;
}

const DEFAULT_MAX_ITERATIONS = 16;

/** A turn emits this to absorb an event without messaging the owner. It is meant to be emitted
 *  alone, but the model often prepends private reasoning ("…no need to message yet.\n\nNO_REPLY"),
 *  so the sentinel suppresses the WHOLE message wherever it appears as its own line — never deliver
 *  the reasoning that leads up to a decision to stay silent. */
export const NO_REPLY = "NO_REPLY";

/** The user-facing text of an assistant message. Empty when the model signaled silence (a NO_REPLY
 *  token on its own line in any text block). `thinking`/`tool_use`/`compaction` blocks are internal
 *  and never delivered. */
function deliverableText(content: Block[]): string {
  const texts = content
    .filter((b) => b.type === "text")
    .map((b) => String((b as { text?: unknown }).text ?? ""));
  const silenced = texts.some((t) => t.split(/\r?\n/).some((line) => line.trim() === NO_REPLY));
  if (silenced) return "";
  return texts.map((t) => t.trim()).filter(Boolean).join("\n\n");
}

export async function runManagerTurn(
  event: ManagerEvent,
  chatId: number,
  deps: TurnDeps,
): Promise<void> {
  deps.transcript.append(eventToUserMessage(event));

  const maxIterations = deps.maxIterations ?? DEFAULT_MAX_ITERATIONS;
  for (let i = 0; i < maxIterations; i += 1) {
    const res = await deps.model.createMessage({
      model: deps.modelName,
      system: deps.buildSystem(),
      messages: deps.transcript.snapshot(),
      tools: deps.registry.specs(),
    });
    deps.onUsage?.(res.usage);

    // Append the assistant message verbatim — including compaction/thinking blocks (DESIGN §4).
    deps.transcript.append({ role: "assistant", content: res.content });

    // The manager's plain text IS its reply — deliver it now. Done every iteration (not just at
    // end-of-turn) so an acknowledgement can reach the owner before the work it kicked off finishes.
    const reply = deliverableText(res.content);
    if (reply) await deps.deliver(chatId, reply);

    const uses = toolUses(res.content);
    if (uses.length === 0) return; // end of turn — any reply was already delivered above

    const results: Block[] = [];
    for (const use of uses) {
      const result = await deps.registry.dispatch(use.name, use.input, { chatId });
      results.push({
        type: "tool_result",
        tool_use_id: use.id,
        content: result.content,
        ...(result.isError ? { is_error: true } : {}),
      } satisfies ToolResultBlock);
    }
    deps.transcript.append({ role: "user", content: results });
  }

  logger.warn("Manager turn hit max iterations without ending", { chatId, maxIterations });
}

/** Translate a queue event into the user-role message that opens the turn. */
function eventToUserMessage(event: ManagerEvent): ModelMessage {
  const text =
    event.kind === "owner_message"
      ? event.text
      : `[worker ${event.workerId} ${event.status}] ${event.summary}`;
  return { role: "user", content: [{ type: "text", text }] };
}
