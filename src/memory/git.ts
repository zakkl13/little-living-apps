// Git changelog for the memory repo (DESIGN §5: "Every backend write is a git commit → a literal
// changelog of what the manager has learned"). The source of truth is markdown on disk; git gives
// us history/rollback for free. We shell out via execFileSync (no git library dependency).

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";

function git(dir: string, args: string[]): string {
  return execFileSync("git", args, {
    cwd: dir,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
}

/** Ensure `dir` exists and is a git repo with a committer identity (idempotent). */
export function ensureRepo(dir: string): void {
  mkdirSync(dir, { recursive: true });
  if (existsSync(join(dir, ".git"))) return;
  git(dir, ["init", "-q"]);
  // Local identity so commits never depend on the host's global git config.
  git(dir, ["config", "user.name", "little-living-apps manager"]);
  git(dir, ["config", "user.email", "manager@lila.local"]);
}

/**
 * Stage everything and commit if (and only if) there is a change. Returns true when a commit was
 * made. `git diff --cached --quiet` exits non-zero when there ARE staged changes, so the throw is
 * the signal to commit.
 */
export function commitAll(dir: string, message: string): boolean {
  git(dir, ["add", "-A"]);
  try {
    git(dir, ["diff", "--cached", "--quiet"]);
    return false; // exit 0 → nothing staged → nothing to commit
  } catch {
    git(dir, ["commit", "-q", "-m", message]);
    return true;
  }
}

/** Number of commits on HEAD (0 if the repo has no commits yet). Used by tests/diagnostics. */
export function commitCount(dir: string): number {
  try {
    return Number(git(dir, ["rev-list", "--count", "HEAD"]).trim()) || 0;
  } catch {
    return 0;
  }
}
