// Worker output condenser (DESIGN §6: "summaries-and-pointers contract; a Haiku summarize pass is
// the fallback if a worker over-returns"). Short output passes through untouched; long output is
// condensed by the utility model so a verbose worker can't blow up the manager's context.

import { textOf, type ManagerModel } from "../manager/anthropic.js";

export type Summarize = (text: string) => Promise<string>;

/** Default: identity below the limit, hard-clip above it (no model call). */
export function clipSummarizer(limit = 2000): Summarize {
  return async (text) => (text.length <= limit ? text : text.slice(0, limit) + "\n…(truncated)");
}

/** Real fallback: condense over-limit output with the utility (Haiku) model. */
export function modelSummarizer(model: ManagerModel, opts: { modelName: string; limit?: number }): Summarize {
  const limit = opts.limit ?? 2000;
  return async (text) => {
    if (text.length <= limit) return text;
    const res = await model.createMessage({
      model: opts.modelName,
      system:
        "Condense the following worker output into a tight summary for a manager: keep concrete " +
        "results, file paths, ids, and decisions; drop logs and filler. 8 lines max.",
      messages: [{ role: "user", content: [{ type: "text", text }] }],
      tools: [],
      maxTokens: 512,
    });
    return textOf(res.content) || text.slice(0, limit);
  };
}
