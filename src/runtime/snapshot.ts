// Crash-recovery snapshots (MIGRATION-CODEX.md §7). Written after every turn (cheap, small) so a
// hibernate mid-conversation loses nothing. Memory (MEMORY_DIR) is the *semantic* truth; this
// snapshot is the *mechanical* state. v3 is a big simplification over v2: Codex owns the manager
// thread's rollout on disk (CODEX_HOME/sessions) and runs its own compaction, so we no longer
// snapshot the ModelMessage[] transcript or compaction blocks — only the thread id, to resume it.
//
// Atomic write (tmp + fsync + rename) so a crash mid-write can never corrupt the file.
//
// No-compat cutover: only v3 is read. The first start after the Opus→Codex deploy finds the old v2
// file, ignores it, and begins a fresh manager thread seeded by persisted memory (§7 cutover note).

import {
  closeSync,
  existsSync,
  fsyncSync,
  mkdirSync,
  openSync,
  readFileSync,
  renameSync,
  writeSync,
} from "node:fs";
import { join } from "node:path";

import type { ManagerEvent } from "./eventQueue.js";
import type { WorkerSnapshot } from "../workers/registry.js";
import type { UsageSnapshot } from "./telemetry.js";
import { logger } from "../logger.js";

export interface ManagerSnapshot {
  version: 3;
  /** The Codex manager thread to resume on cold wake; absent before the first turn / after /new. */
  managerThreadId?: string;
  queue: ManagerEvent[];
  workers: WorkerSnapshot[];
  /** Cumulative token-usage meter (lifetime totals survive a restart). */
  usage?: UsageSnapshot;
}

export interface SnapshotStore {
  save(snap: ManagerSnapshot): void;
  load(): ManagerSnapshot | undefined;
}

export function openSnapshotStore(dir: string): SnapshotStore {
  const path = join(dir, "manager.json");
  return {
    save(snap) {
      mkdirSync(dir, { recursive: true });
      const tmp = `${path}.tmp`;
      const fd = openSync(tmp, "w");
      try {
        writeSync(fd, JSON.stringify(snap));
        fsyncSync(fd);
      } finally {
        closeSync(fd);
      }
      renameSync(tmp, path);
    },
    load() {
      if (!existsSync(path)) return undefined;
      try {
        const parsed = JSON.parse(readFileSync(path, "utf8")) as {
          version?: number;
          managerThreadId?: string;
          queue?: ManagerEvent[];
          workers?: WorkerSnapshot[];
          usage?: UsageSnapshot;
        };
        // We are the only writer; a v3 file carries both arrays. A missing field means it is
        // corrupt/truncated, so ignore it. A pre-v3 (Opus) file is discarded — a fresh thread starts.
        if (parsed.version === 3 && Array.isArray(parsed.queue) && Array.isArray(parsed.workers)) {
          return {
            version: 3,
            ...(parsed.managerThreadId ? { managerThreadId: parsed.managerThreadId } : {}),
            queue: parsed.queue as ManagerEvent[],
            workers: parsed.workers as WorkerSnapshot[],
            ...(parsed.usage ? { usage: parsed.usage } : {}),
          };
        }
        logger.warn("Snapshot was missing/old version; ignoring (fresh manager thread)", {
          path,
          version: parsed.version,
        });
      } catch (err) {
        logger.warn("Failed to parse snapshot; starting fresh", { path, error: (err as Error).message });
      }
      return undefined;
    },
  };
}
