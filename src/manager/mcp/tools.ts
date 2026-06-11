// The Lila MCP tool set (MIGRATION-CODEX.md §5): the manager Codex thread's ENTIRE capability,
// exposed over MCP. Memory tools map onto MemFs; orchestration tools map onto the live Orchestrator.
// There is deliberately no shell/file/net tool — the manager's "no hands" boundary is preserved,
// now as the tool list of this one loopback server.
//
// Each tool is a flat record (name + zod input shape + handler) so server.ts can register it on an
// McpServer AND tests can invoke the handler directly against fakes. The handler logic here is the
// verbatim re-homing of the old Anthropic tool modules; only the envelope changed.

import { z, type ZodRawShape } from "zod";

import type { MemFs, MemoryCommand } from "../../memory/memfs.js";
import type { SearchHit } from "../../memory/fts.js";
import type { Orchestrator, PromptRecorder } from "../../workers/types.js";

export interface ToolReply {
  content: Array<{ type: "text"; text: string }>;
  isError?: boolean;
}

export interface LilaTool {
  name: string;
  description: string;
  inputSchema: ZodRawShape;
  handler: (args: Record<string, unknown>) => ToolReply | Promise<ToolReply>;
}

export interface LilaToolDeps {
  mem: MemFs;
  orchestrator: Orchestrator;
  telemetry?: PromptRecorder;
  /** The active manager turn id, read at call time so worker prompts are traced to their turn. */
  currentTurnId: () => number;
}

const ok = (text: string): ToolReply => ({ content: [{ type: "text", text }] });
const fail = (text: string): ToolReply => ({ content: [{ type: "text", text }], isError: true });

/** Wrap a handler so a thrown MemoryError (bad path, missing file, …) surfaces as an is_error
 *  result the model can recover from, never a crashed tool call. */
function guard(fn: LilaTool["handler"]): LilaTool["handler"] {
  return async (args) => {
    try {
      return await fn(args);
    } catch (err) {
      return fail(`error: ${(err as Error).message}`);
    }
  };
}

function formatHits(hits: SearchHit[]): string {
  if (hits.length === 0) return "(no matches)";
  return hits.map((h) => `${h.path}\n    ${h.snippet}`).join("\n");
}

export function lilaTools(deps: LilaToolDeps): LilaTool[] {
  const { mem, orchestrator: orch, telemetry } = deps;
  const path = z.string().describe("a /memories/... path");

  // ---- memory tools (discrete; map onto the MemoryCommand union) ----
  const memoryTools: LilaTool[] = [
    {
      name: "memory_view",
      description:
        "Read a memory file, or list a directory. Paths are under /memories (e.g. " +
        "/memories/archival/decisions/stack.md).",
      inputSchema: { path, view_range: z.array(z.number()).length(2).optional() },
      handler: guard((a) =>
        ok(
          mem.view({
            command: "view",
            path: String(a.path),
            ...(Array.isArray(a.view_range) ? { view_range: a.view_range as number[] } : {}),
          }),
        ),
      ),
    },
    {
      name: "memory_create",
      description: "Create or overwrite a memory file with the given text.",
      inputSchema: { path, file_text: z.string() },
      handler: guard((a) =>
        ok(mem.create({ command: "create", path: String(a.path), file_text: String(a.file_text) })),
      ),
    },
    {
      name: "memory_str_replace",
      description: "Replace a unique substring in a memory file (add context if it isn't unique).",
      inputSchema: { path, old_str: z.string(), new_str: z.string() },
      handler: guard((a) =>
        ok(
          mem.str_replace({
            command: "str_replace",
            path: String(a.path),
            old_str: String(a.old_str),
            new_str: String(a.new_str),
          }),
        ),
      ),
    },
    {
      name: "memory_insert",
      description: "Insert a line of text into a memory file at the given 0-based line index.",
      inputSchema: { path, insert_line: z.number(), insert_text: z.string() },
      handler: guard((a) =>
        ok(
          mem.insert({
            command: "insert",
            path: String(a.path),
            insert_line: Number(a.insert_line),
            insert_text: String(a.insert_text),
          }),
        ),
      ),
    },
    {
      name: "memory_delete",
      description: "Delete a memory file or directory.",
      inputSchema: { path },
      handler: guard((a) => ok(mem.delete({ command: "delete", path: String(a.path) }))),
    },
    {
      name: "memory_rename",
      description: "Rename or move a memory file or directory.",
      inputSchema: { old_path: path, new_path: path },
      handler: guard((a) =>
        ok(mem.rename({ command: "rename", old_path: String(a.old_path), new_path: String(a.new_path) })),
      ),
    },
    {
      name: "memory_search",
      description:
        "Full-text search across ALL memory files (system, archival, recall). Returns paths + " +
        "snippets; use memory_view to read a hit in full.",
      inputSchema: { query: z.string(), limit: z.number().optional() },
      handler: guard((a) => ok(formatHits(mem.search(String(a.query ?? ""), numOr(a.limit, 10))))),
    },
    {
      name: "recall_search",
      description: "Full-text search restricted to recall/ (summarized past conversations).",
      inputSchema: { query: z.string(), limit: z.number().optional() },
      handler: guard((a) => ok(formatHits(mem.recallSearch(String(a.query ?? ""), numOr(a.limit, 10))))),
    },
  ];

  // ---- orchestration (async; maps onto the live Orchestrator) ----
  // Subagents are single-shot: there is deliberately no send/steer/cancel/list. A worker runs its
  // one objective, reports back once, and is gone; continuity lives in the workspace + git + memory.
  const orchestrationTools: LilaTool[] = [
    {
      name: "subagent_start",
      description:
        "Spawn a single-use Codex worker on an objective. It runs in the background, reports back " +
        "once as an event, and is then gone — there is no follow-up channel, so put everything it " +
        "needs (context, scope, how to verify) in the objective. It starts cold, with only the " +
        "workspace, the git history, and what you wrote.",
      inputSchema: {
        objective: z.string(),
        project: z.string().optional().describe("project dir under the workspace (optional)"),
      },
      handler: guard((a) => {
        const objective = String(a.objective ?? "");
        const w = orch.start(objective, optStr(a.project));
        telemetry?.recordPrompt({
          turnId: deps.currentTurnId(),
          workerId: w.id,
          kind: "start",
          prompt: objective,
        });
        return ok("subagent started — it will report back once when it finishes");
      }),
    },
  ];

  return [...memoryTools, ...orchestrationTools];
}

function numOr(v: unknown, fallback: number): number {
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}

function optStr(v: unknown): string | undefined {
  return typeof v === "string" && v.length > 0 ? v : undefined;
}
