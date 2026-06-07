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
  /** Whether this worker currently holds a Sprite keep-alive reference. */
  holding: boolean;
  /** The AbortController for the current run; compared on settle to detect supersession. */
  currentAbort?: AbortController;
}

export interface WorkerRegistry {
  add(rec: { id: string; purpose: string; project: string }): WorkerRecord;
  get(id: string): WorkerRecord | undefined;
  all(): WorkerRecord[];
  info(id: string): WorkerInfo | undefined;
  infos(): WorkerInfo[];
  /** Workers with a run in flight (status running). */
  activeCount(): number;
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
      const rec: WorkerRecord = { id, purpose, project, status: "running", holding: false };
      workers.set(id, rec);
      return rec;
    },
    get: (id) => workers.get(id),
    all: () => [...workers.values()],
    info: (id) => {
      const r = workers.get(id);
      return r ? toInfo(r) : undefined;
    },
    infos: () => [...workers.values()].map(toInfo),
    activeCount: () => [...workers.values()].filter((r) => r.status === "running").length,
  };
}
