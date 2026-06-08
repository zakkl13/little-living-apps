// Worker registry (DESIGN §6): the live set of Codex workers — id, purpose, status, the codex
// thread id (for resume/steer), an AbortController for the in-flight run, and the latest condensed
// output. Mirrored into system/workers.md by the orchestrator so it survives cold wake.

import type { WorkerInfo, WorkerStatus } from "../manager/tools/orchestration.js";

export interface WorkerRecord {
  id: string;
  purpose: string;
  project: string;
  status: WorkerStatus;
  /** Codex thread id once known (threads persist server-side → durability for free). */
  threadId?: string;
  /** Latest condensed result/output line(s). */
  latest?: string;
  /** The AbortController for the current run; compared on settle to detect supersession. */
  currentAbort?: AbortController;
}

/** Durable projection of a worker for cold-wake recovery (DESIGN §11). */
export interface WorkerSnapshot {
  id: string;
  purpose: string;
  project: string;
  status: WorkerStatus;
  threadId?: string;
  latest?: string;
}

export interface WorkerRegistry {
  add(rec: { id: string; purpose: string; project: string }): WorkerRecord;
  get(id: string): WorkerRecord | undefined;
  info(id: string): WorkerInfo | undefined;
  infos(): WorkerInfo[];
  /** Durable projection for snapshotting. */
  snapshot(): WorkerSnapshot[];
  /** Rehydrate from a snapshot on boot; in-flight runs are gone so statuses settle to idle. */
  rehydrate(records: WorkerSnapshot[]): void;
}

const toInfo = (r: WorkerRecord): WorkerInfo => ({
  id: r.id,
  purpose: r.purpose,
  status: r.status,
  project: r.project,
});

export function createWorkerRegistry(): WorkerRegistry {
  const workers = new Map<string, WorkerRecord>();
  return {
    add({ id, purpose, project }) {
      const rec: WorkerRecord = { id, purpose, project, status: "running" };
      workers.set(id, rec);
      return rec;
    },
    get: (id) => workers.get(id),
    info: (id) => {
      const r = workers.get(id);
      return r ? toInfo(r) : undefined;
    },
    infos: () => [...workers.values()].map(toInfo),
    snapshot: () =>
      [...workers.values()].map((r) => ({
        id: r.id,
        purpose: r.purpose,
        project: r.project,
        status: r.status,
        ...(r.threadId ? { threadId: r.threadId } : {}),
        ...(r.latest ? { latest: r.latest } : {}),
      })),
    rehydrate(records) {
      for (const rec of records) {
        workers.set(rec.id, {
          id: rec.id,
          purpose: rec.purpose,
          project: rec.project,
          // A worker that was "running" before the crash has no live run now → treat as idle and
          // reconcilable via subagent_poll (the codex thread persists server-side).
          status: rec.status === "running" ? "idle" : rec.status,
          ...(rec.threadId ? { threadId: rec.threadId } : {}),
          ...(rec.latest ? { latest: rec.latest } : {}),
        });
      }
    },
  };
}
