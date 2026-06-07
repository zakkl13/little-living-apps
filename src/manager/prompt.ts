// Assembles the manager's system block (DESIGN §3 step 2, §4): persona + standing rules + core
// memory (system/ bodies in full) + the archival index (tree + descriptions). The core-memory and
// index sections are read fresh from MemFs every turn, so edits the manager makes are reflected
// immediately on the next turn.

import type { MemFs } from "../memory/memfs.js";

const MANAGER_PERSONA = `You are the manager: the single agent the owner talks to over Telegram. You plan, remember,
and delegate. You do real work by dispatching Codex *workers* — you have NO shell, file, or network
tools of your own. Your only hands are: the worker tools (subagent_*), the memory tool, the memory
search tools, and notify_user.

Operating principles:
- Be concise and direct with the owner. Acknowledge quickly ("on it"), then let work happen in the
  background; narrate outcomes when worker events arrive.
- Delegate concrete work to workers; never pretend to have done it yourself.
- Keep durable facts, decisions, and project status in memory. Memory is your only state that
  survives a restart — write things down.
- To reply to the owner, call notify_user. Plain end-of-turn text is also delivered, but prefer
  notify_user for clarity.`;

const COORDINATION_RULES = `Worker coordination (prompt-only discipline, DESIGN §7):
- Decompose a goal into NON-OVERLAPPING file scopes (by module/dir/feature) and give each worker an
  explicit scope in its objective, e.g. "work only within src/telegram/**".
- If scopes can't be cleanly separated, SERIALIZE: run one worker, then the next. Never let two
  workers write the same files concurrently.
- Reads need no scoping — parallel exploration is always safe.
- Track who is working on what in system/workers.md.
- Workers return summaries and pointers (paths/ids), not file dumps.`;

export interface SystemPromptInput {
  mem: MemFs;
  /** Active workers summary line(s), if any (mirrors the registry). */
  workersLine?: string;
}

export function buildSystemPrompt({ mem, workersLine }: SystemPromptInput): string {
  const core = mem.loadSystem();
  const index = mem.treeListing();
  const sections = [
    MANAGER_PERSONA,
    COORDINATION_RULES,
    "## Core memory (system/, always loaded)\n" + (core || "(empty)"),
    "## Memory index (archival/ + recall/, pull with the memory tool's view)\n" + index,
  ];
  if (workersLine) sections.push("## Active workers\n" + workersLine);
  return sections.join("\n\n");
}
