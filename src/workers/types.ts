// Worker-tier domain types (DESIGN §6). Subagents are PURELY EPHEMERAL: a worker is born for one
// objective, runs once, reports back as a single event, and is gone. There is no follow-up channel,
// no roster, no resume — continuity lives in the durable substrate (the workspace, git history, and
// the manager's memory), never in worker state. The Orchestrator interface is correspondingly tiny.

export interface Orchestrator {
  /** Spawn a single-shot worker for `objective` in `project` (returns immediately; the worker
   *  reports back once as a worker_event and is then gone). */
  start(objective: string, project?: string): { id: string };
  /** Number of runs currently in flight (used to gate premature "all done" replies). */
  running(): number;
  /** Resolve once every in-flight run has settled (test/eval helper). */
  whenQuiet(): Promise<void>;
}

/** The slice of Telemetry the orchestration tools write to (decoupled so tests can pass a stub or
 *  nothing at all). Records the exact Codex prompt the manager dispatched, stamped with the turn. */
export interface PromptRecorder {
  recordPrompt(rec: { turnId: number; workerId: string; kind: "start"; prompt: string }): void;
}
