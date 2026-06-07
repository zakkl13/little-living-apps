// Memory tools (DESIGN §5, §9): the native `memory` tool (memory_20250818) backed by MemFs, plus
// the two search tools the native tool lacks — memory_search (FTS over everything) and
// recall_search (FTS over recall/). The native tool's input IS a command object; we hand it
// straight to the MemFs dispatcher.

import type { MemFs, MemoryCommand } from "../../memory/memfs.js";
import type { SearchHit } from "../../memory/fts.js";
import type { ToolModule } from "./types.js";

export function memoryToolModule(mem: MemFs): ToolModule {
  return {
    specs: [
      { kind: "memory" },
      {
        kind: "custom",
        name: "memory_search",
        description:
          "Full-text search across ALL memory files (system, archival, recall). Returns paths + " +
          "snippets; use the memory tool's view to read a hit in full.",
        input_schema: {
          type: "object",
          properties: {
            query: { type: "string", description: "search terms" },
            limit: { type: "number", description: "max results (default 10)" },
          },
          required: ["query"],
        },
      },
      {
        kind: "custom",
        name: "recall_search",
        description: "Full-text search restricted to recall/ (summarized past conversations).",
        input_schema: {
          type: "object",
          properties: {
            query: { type: "string" },
            limit: { type: "number" },
          },
          required: ["query"],
        },
      },
    ],
    handlers: {
      memory: (input) => ({ content: mem.execute(input as unknown as MemoryCommand) }),
      memory_search: (input) => ({
        content: formatHits(mem.search(String(input.query ?? ""), numOr(input.limit, 10))),
      }),
      recall_search: (input) => ({
        content: formatHits(mem.recallSearch(String(input.query ?? ""), numOr(input.limit, 10))),
      }),
    },
  };
}

function formatHits(hits: SearchHit[]): string {
  if (hits.length === 0) return "(no matches)";
  return hits.map((h) => `${h.path}\n    ${h.snippet}`).join("\n");
}

function numOr(v: unknown, fallback: number): number {
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}
