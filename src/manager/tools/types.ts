// Shared shapes for the manager's tool layer (DESIGN §9). A ToolModule bundles the specs we
// advertise to the model with the handlers that execute them; buildRegistry stitches modules into
// the single dispatch table that is the manager's entire capability surface.

import type { ToolSpec } from "../anthropic.js";

export interface ToolResult {
  content: string;
  isError?: boolean;
}

export interface ToolHandlerCtx {
  /** The owner chat this turn is serving (worker attribution / logging). */
  chatId: number;
}

export type ToolHandler = (
  input: Record<string, unknown>,
  ctx: ToolHandlerCtx,
) => Promise<ToolResult> | ToolResult;

export interface ToolModule {
  specs: ToolSpec[];
  handlers: Record<string, ToolHandler>;
}
