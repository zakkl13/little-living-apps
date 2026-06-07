// Worker output bound (DESIGN §6: "summaries-and-pointers contract"). A worker that over-returns
// can't be allowed to blow up the manager's context, so output above the limit is deterministically
// clipped. No model call, no fallback path — a hard, predictable ceiling.

export type Summarize = (text: string) => Promise<string>;

/** Identity below the limit, hard-clip above it. */
export function clipSummarizer(limit = 2000): Summarize {
  return async (text) => (text.length <= limit ? text : text.slice(0, limit) + "\n…(truncated)");
}
