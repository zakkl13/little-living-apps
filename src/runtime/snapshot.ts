// Crash-recovery snapshots (DESIGN §11). Written after every turn (cheap, small) so a hibernate
// mid-conversation loses nothing. Memory (MEMORY_DIR) is the *semantic* truth; this snapshot is the
// *mechanical* state: the working transcript (INCLUDING server compaction blocks, which must be
// preserved verbatim — DESIGN §4/§12), the pending event queue, and the worker registry.
//
// Atomic write (tmp + fsync + rename) so a crash mid-write can never corrupt the file — the same
// discipline the v0.1 session store used.

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

import type { ModelMessage } from "../manager/anthropic.js";
import type { ManagerEvent } from "./eventQueue.js";
import type { WorkerSnapshot } from "../workers/registry.js";
import type { CostSnapshot } from "./telemetry.js";
import { logger } from "../logger.js";

export interface ManagerSnapshot {
  version: 2;
  transcript: ModelMessage[];
  queue: ManagerEvent[];
  workers: WorkerSnapshot[];
  /** Cumulative Inspector cost meter; absent in v1 snapshots and when the Inspector is off. */
  cost?: CostSnapshot;
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
          transcript?: ModelMessage[];
          queue?: ManagerEvent[];
          workers?: WorkerSnapshot[];
          cost?: CostSnapshot;
        };
        // We are the only writer; every version carries all three arrays. A missing field means the
        // file is corrupt/truncated, so ignore it rather than papering over it. v1 snapshots (no
        // cost field) load fine — cost just starts from zero, which is correct.
        if (
          (parsed.version === 1 || parsed.version === 2) &&
          Array.isArray(parsed.transcript) &&
          Array.isArray(parsed.queue) &&
          Array.isArray(parsed.workers)
        ) {
          return {
            version: 2,
            transcript: parsed.transcript as ModelMessage[],
            queue: parsed.queue as ManagerEvent[],
            workers: parsed.workers as WorkerSnapshot[],
            ...(parsed.cost ? { cost: parsed.cost } : {}),
          };
        }
        logger.warn("Snapshot had unexpected shape; ignoring", { path });
      } catch (err) {
        logger.warn("Failed to parse snapshot; starting fresh", { path, error: (err as Error).message });
      }
      return undefined;
    },
  };
}
