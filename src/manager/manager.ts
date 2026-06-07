// One manager turn (DESIGN §3, §4). Pop an event → append to the transcript → call the model in a
// tool loop (execute tool_use, append tool_result, call again) → on end_turn deliver any text to
// the owner. The full response.content is appended VERBATIM every iteration, which is the
// compaction round-trip contract (compaction/thinking blocks pass through untouched).

import {
  textOf,
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

export interface TurnDeps {
  model: ManagerModel;
  modelName: string;
  registry: ToolRegistry;
  transcript: Transcript;
  /** Rebuilt fresh each iteration so memory edits made mid-turn are reflected immediately. */
  buildSystem: () => string;
  /** Fallback delivery for end_turn text (notify_user is the preferred path). */
  deliver: (chatId: number, text: string) => Promise<void>;
  onUsage?: (usage: { inputTokens: number; outputTokens: number }) => void;
  maxIterations?: number;
}

const DEFAULT_MAX_ITERATIONS = 16;

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

    const uses = toolUses(res.content);
    if (uses.length === 0) {
      const text = textOf(res.content);
      if (text) await deps.deliver(chatId, text);
      return;
    }

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
