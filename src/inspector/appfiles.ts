// Read-only reader for the target app's "memory bank" — the durable docs a worker keeps for the app
// it builds (AGENTS.md, README, a memory-bank/ or docs/ tree). Scoped strictly to workspaceDir and
// path-traversal guarded the same way MemFs guards /memories: the Inspector can read these files but
// can never reach outside the workspace, and never writes.

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, normalize, relative } from "node:path";

/** Top-level files (case-insensitive) treated as memory-bank material. */
const ROOT_FILES = ["AGENTS.md", "CLAUDE.md", "README.md", "README", ".agent.md", "DESIGN.md"];
/** Directories whose markdown/text contents are memory-bank material. */
const BANK_DIRS = ["memory-bank", ".memory", "docs", ".agent"];
const MAX_BYTES = 256 * 1024;
const MAX_FILES = 500;

export interface AppFiles {
  /** Repo-relative paths of memory-bank files that currently exist in the workspace. */
  list(): string[];
  /** Raw body of one file (undefined if missing, outside the workspace, or too large). */
  read(rel: string): string | undefined;
}

export function openAppFiles(workspaceDir: string): AppFiles {
  const root = normalize(workspaceDir);

  /** Map an untrusted relative path to an absolute path guaranteed to stay under root. */
  function safeAbs(rel: string): string | undefined {
    const abs = normalize(join(root, rel));
    const inside = abs === root || abs.startsWith(root + "/");
    return inside ? abs : undefined;
  }

  return {
    list() {
      const out: string[] = [];
      for (const name of ROOT_FILES) {
        const abs = join(root, name);
        if (existsSync(abs) && statSync(abs).isFile()) out.push(name);
      }
      for (const sub of BANK_DIRS) {
        const dir = join(root, sub);
        if (existsSync(dir) && statSync(dir).isDirectory()) {
          for (const r of walk(dir, root)) {
            out.push(r);
            if (out.length >= MAX_FILES) return out;
          }
        }
      }
      return out;
    },

    read(rel) {
      const abs = safeAbs(rel);
      if (!abs || !existsSync(abs) || !statSync(abs).isFile()) return undefined;
      if (statSync(abs).size > MAX_BYTES) return `(file too large to display: ${rel})`;
      return readFileSync(abs, "utf8");
    },
  };
}

/** Recursively list text-ish files under `dir`, as paths relative to `root`, skipping dotdirs. */
function walk(dir: string, root: string): string[] {
  const out: string[] = [];
  const stack = [dir];
  while (stack.length) {
    const current = stack.pop()!;
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      if (entry.name.startsWith(".")) continue;
      const full = join(current, entry.name);
      if (entry.isDirectory()) stack.push(full);
      else if (/\.(md|txt|mdx|markdown)$/i.test(entry.name)) {
        out.push(relative(root, full).split(/[\\/]/).join("/"));
      }
    }
  }
  return out.sort();
}
