// The worker⇄manager contract (DESIGN §6, "summaries-and-pointers"). Two halves of one idea:
//
//   1. WORKER_PROTOCOL — prepended to every prompt we hand a Codex worker. It tells the worker the
//      one thing it can't otherwise know: the manager never sees its transcript, tool output, or
//      files — ONLY a summary block it must write. So we spell out that block's format + size budget
//      up front, instead of the manager discovering the limit reactively and asking for resends.
//      It also mandates browser self-validation (every worker proves its own work with screenshots
//      + real interaction before reporting) and clean checkpoint discipline.
//
//   2. extractManagerSummary / managerSummarizer — the reader half. We pull just that block back out
//      of the worker's full output, so the manager's context carries the worker's own intended
//      summary, not a blind byte-clip of its whole transcript (which dropped the conclusion).

import type { Summarize } from "./summarize.js";

export const MANAGER_SUMMARY_MARKER = "### SUMMARY FOR MANAGER";

/** Safety ceiling if a worker over-writes its summary block. */
const SUMMARY_CEILING = 1500;

export const WORKER_PROTOCOL = [
  "[Manager protocol — applies every turn; read this before you start]",
  "- Your manager cannot see your transcript, your tool output, or your files. The ONLY thing it",
  "  receives back from you is the summary block described below — so everything it needs must be there.",
  "- Validate your own work before reporting — your word alone is not proof. You have headless",
  "  Chromium via Playwright. For anything a user would see or click, exercise the change the way a",
  "  user would against the running app — load the real pages, click, fill, submit, not just a 200",
  "  check — and screenshot the result. Save screenshots under /tmp/lila-shots/ (create it if needed)",
  "  with descriptive names, then open each one and confirm it shows what the objective asked for.",
  "  For work with nothing to see, validate with tests or real requests instead.",
  `- End your reply with a section that begins with the exact line "${MANAGER_SUMMARY_MARKER}". Its`,
  "  FIRST line must be the outcome on its own: PASS or FAIL for a validation task, otherwise done or",
  "  blocked plus one clause. Then a tight report in 150 words or less: what you did, which files",
  "  changed, any commit, and concrete verification (HTTP status codes, test results, what your",
  "  screenshots show). End the block with a `Screenshots:` line listing the absolute paths of the",
  "  screenshots that prove the result — the manager can send them to the owner — or `Screenshots: none`",
  "  if the work had nothing visual. Write normally above the block — only this block is relayed, and",
  "  if it runs long it is clipped from the end, so the verdict goes first and nothing outside the",
  "  block reaches the manager.",
  "- Check `git status --short` before editing. Do not modify unrelated dirty files. If a file you",
  "  need to edit is already dirty, inspect the diff first and work with it deliberately; report that",
  "  context in your summary.",
  "- Commit your own finished edits in small logical units. If you cannot commit, say exactly why and",
  "  include the remaining `git status --short` in your summary.",
  "",
  "---- your task ----",
  "",
].join("\n");

/** Prepend the standing protocol to a worker prompt (objective / follow-up / steer guidance). */
export function withProtocol(prompt: string): string {
  return WORKER_PROTOCOL + prompt;
}

/** Pull just the manager-summary block out of a worker's full output. Falls back to the TAIL (where
 *  the conclusion lives) when no block is present — never the head, which is setup/preamble. */
export function extractManagerSummary(output: string): string {
  const idx = output.lastIndexOf(MANAGER_SUMMARY_MARKER);
  if (idx !== -1) {
    const block = output.slice(idx + MANAGER_SUMMARY_MARKER.length).trim();
    if (block.length <= SUMMARY_CEILING) return block;
    // Over-ceiling: keep BOTH ends. The protocol mandates a verdict-first block (head), but a
    // non-compliant worker's conclusion lives at the tail — losing either once cost a whole
    // resume round-trip just to re-ask for the verdict.
    const head = block.slice(0, SUMMARY_CEILING - 400);
    const tail = block.slice(block.length - 380);
    return `${head}\n…(clipped)…\n${tail}`;
  }
  const trimmed = output.trim();
  if (trimmed.length <= SUMMARY_CEILING) return trimmed;
  // Keep the end, not the start: the verification/result a manager needs lives at the conclusion.
  return "…(no summary block; showing the tail)\n" + trimmed.slice(trimmed.length - SUMMARY_CEILING);
}

/** The default condenser: extract the worker's summary block (DESIGN §6). */
export function managerSummarizer(): Summarize {
  return async (text) => extractManagerSummary(text);
}
