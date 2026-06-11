// Worker-tier domain types (DESIGN §6). The Orchestrator interface and its value objects, plus the
// telemetry seam the orchestration tools write to. These live here (not in the manager's tool layer)
// because they are core to the host-driven worker model and are consumed by the registry, the
// orchestrator, and the Lila MCP tools alike.

export type WorkerStatus = "running" | "idle" | "failed" | "canceled";

export interface WorkerInfo {
  id: string;
  purpose: string;
  status: WorkerStatus;
  project: string;
}

export interface Orchestrator {
  /** Spawn a worker for `objective` in `project` (returns immediately). */
  start(objective: string, project?: string): WorkerInfo;
  /** Send a follow-up to an idle worker (async). */
  send(id: string, message: string): WorkerInfo;
  /** Redirect a busy worker: abort the in-flight run, then resume with `guidance`. */
  steer(id: string, guidance: string): WorkerInfo;
  /** Abort a worker's run without resuming. */
  cancel(id: string): WorkerInfo;
  /** All known workers (mirrors system/workers.md). */
  list(): WorkerInfo[];
}

/** The slice of Telemetry the orchestration tools write to (decoupled so tests can pass a stub or
 *  nothing at all). Records the exact Codex prompt the manager dispatched, stamped with the turn. */
export interface PromptRecorder {
  recordPrompt(rec: {
    turnId: number;
    workerId: string;
    kind: "start" | "send" | "steer" | "cancel";
    prompt: string;
  }): void;
}
