// The tool registry (DESIGN §9): the manager's ENTIRE capability. It exposes the specs we send to
// the model and a single dispatch table. There is deliberately no bash/read/write/web/net tool —
// the "no hands" capability boundary is airtight by construction (DESIGN §4).

import type { ToolSpec } from "../anthropic.js";
import type { ToolHandler, ToolHandlerCtx, ToolModule, ToolResult } from "./types.js";

export interface ToolRegistry {
  specs(): ToolSpec[];
  dispatch(name: string, input: Record<string, unknown>, ctx: ToolHandlerCtx): Promise<ToolResult>;
}

export function buildRegistry(modules: ToolModule[]): ToolRegistry {
  const specs: ToolSpec[] = [];
  const handlers = new Map<string, ToolHandler>();

  for (const mod of modules) {
    for (const s of mod.specs) specs.push(s);
    for (const [name, handler] of Object.entries(mod.handlers)) {
      if (handlers.has(name)) throw new Error(`duplicate tool handler: ${name}`);
      handlers.set(name, handler);
    }
  }

  return {
    specs: () => specs,
    async dispatch(name, input, ctx) {
      const handler = handlers.get(name);
      if (!handler) return { content: `unknown tool: ${name}`, isError: true };
      try {
        return await handler(input, ctx);
      } catch (err) {
        // Surface the failure as a tool_result so the model can recover, rather than crashing the
        // turn (DESIGN §12: a tool failure is the model's problem to handle, not the loop's).
        return { content: `error: ${(err as Error).message}`, isError: true };
      }
    },
  };
}
