// The manager's model seam (DESIGN §4). We own the agent loop and talk to Anthropic through this
// single interface — the same pattern CodexRunner uses for the worker side. Tests inject a scripted
// fake (test/fakes/fakeAnthropic.ts); production injects the real @anthropic-ai/sdk wrapper below.
//
// Content blocks are modeled as opaque `{ type, ... }` records. The loop only *interprets* `text`
// and `tool_use`; every other block (notably `compaction` and `thinking`) is passed back verbatim,
// which is exactly the compaction round-trip contract: "append the full response.content back each
// turn so the API can replace compacted history" (DESIGN §4).

import Anthropic from "@anthropic-ai/sdk";

/** A single content block. Open shape so unknown block types survive a round-trip untouched. */
export type Block = { type: string; [key: string]: unknown };

export interface ModelMessage {
  role: "user" | "assistant";
  content: Block[];
}

export interface ToolResultBlock extends Block {
  type: "tool_result";
  tool_use_id: string;
  content: string;
  is_error?: boolean;
}

/** Tool specs we advertise to the model: our custom tools + the native memory tool. */
export type ToolSpec =
  | { kind: "custom"; name: string; description: string; input_schema: Record<string, unknown> }
  | { kind: "memory" };

export interface ManagerRequest {
  model: string;
  system: string;
  messages: ModelMessage[];
  tools: ToolSpec[];
  maxTokens?: number;
}

export interface ManagerResponse {
  content: Block[];
  /** "tool_use" | "end_turn" | "max_tokens" | "pause_turn" | ... */
  stopReason: string | null;
  usage: { inputTokens: number; outputTokens: number };
}

export interface ManagerModel {
  createMessage(req: ManagerRequest): Promise<ManagerResponse>;
}

// ---- block helpers (shared by the loop, the fake, and tests) ----------------

export interface ToolUseBlock extends Block {
  type: "tool_use";
  id: string;
  name: string;
  input: Record<string, unknown>;
}

export function isToolUse(b: Block): b is ToolUseBlock {
  return b.type === "tool_use" && typeof (b as ToolUseBlock).id === "string";
}

export function toolUses(content: Block[]): ToolUseBlock[] {
  return content.filter(isToolUse);
}

/** Concatenate all `text` blocks (the model's natural-language output for a turn). */
export function textOf(content: Block[]): string {
  return content
    .filter((b) => b.type === "text" && typeof (b as { text?: unknown }).text === "string")
    .map((b) => (b as unknown as { text: string }).text)
    .join("")
    .trim();
}

// ---- the real wrapper (typechecks against the SDK; not exercised in tests) ---

export interface AnthropicModelOptions {
  apiKey: string;
  baseUrl?: string;
  /** Beta headers carrying the hard parts server-side (DESIGN §4, §10). */
  betas?: string[];
}

const DEFAULT_BETAS = ["compact-2026-01-12", "context-management-2025-06-27"];

export function createAnthropicModel(opts: AnthropicModelOptions): ManagerModel {
  const client = new Anthropic({
    apiKey: opts.apiKey,
    ...(opts.baseUrl ? { baseURL: opts.baseUrl } : {}),
  });
  const betas = opts.betas ?? DEFAULT_BETAS;

  return {
    async createMessage(req: ManagerRequest): Promise<ManagerResponse> {
      const params = {
        model: req.model,
        // Adaptive thinking can spend output tokens on reasoning, so give headroom; 16k stays under
        // the SDK's non-streaming HTTP-timeout guard while leaving room to think + reply + call tools.
        max_tokens: req.maxTokens ?? 16000,
        system: req.system,
        messages: req.messages as unknown as Anthropic.Beta.BetaMessageParam[],
        tools: req.tools.map(toBetaTool),
        // Adaptive thinking gives the manager a PRIVATE reasoning channel (thinking blocks), so it
        // stops reasoning out loud in `text` — which the loop now delivers straight to the owner
        // (DESIGN §4). `adaptive` is the only on-mode for opus-4-8; it auto-enables interleaved
        // thinking between tool calls (no beta header). `display:"summarized"` populates the block
        // text so reasoning is visible in snapshots for debugging — it is never delivered (the loop
        // reads only `text` blocks). Thinking blocks round-trip verbatim with the rest of
        // response.content, preserving their signature (the same append-verbatim contract as compaction).
        thinking: { type: "adaptive", display: "summarized" },
        // Effort lives inside output_config (not top-level). `high` is the agentic sweet spot.
        output_config: { effort: "high" },
        // Server-side context management: compaction + stale tool-result pruning (DESIGN §4).
        context_management: {
          edits: [{ type: "compact_20260112" }, { type: "clear_tool_uses_20250919" }],
        },
        betas,
      } as unknown as Anthropic.Beta.Messages.MessageCreateParamsNonStreaming;

      const res = await client.beta.messages.create(params);
      return {
        content: res.content as unknown as Block[],
        stopReason: res.stop_reason,
        usage: {
          inputTokens: res.usage.input_tokens,
          outputTokens: res.usage.output_tokens,
        },
      };
    },
  };
}

function toBetaTool(spec: ToolSpec): Anthropic.Beta.BetaToolUnion {
  if (spec.kind === "memory") {
    return { type: "memory_20250818", name: "memory" } as unknown as Anthropic.Beta.BetaToolUnion;
  }
  return {
    name: spec.name,
    description: spec.description,
    input_schema: spec.input_schema,
  } as unknown as Anthropic.Beta.BetaToolUnion;
}
