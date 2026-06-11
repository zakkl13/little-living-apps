// The async worker tier (DESIGN §6). Implements the Orchestrator interface the manager's
// subagent_* tools call. Every start/send/steer returns a handle IMMEDIATELY and runs the Codex
// worker in the background; when a run settles, a worker_event is pushed onto the queue, which
// triggers a later manager turn that narrates the outcome. This is what keeps long builds from
// freezing the conversation.
//
// Steering is abort+resume (the TS SDK exposes no mid-turn steer): we abort the in-flight run and
// immediately resume the same thread with new guidance. Supersession is detected by comparing the
// settling run's AbortController against the worker's current one — an aborted-for-steer run is a
// transition, not a completion, so it emits no event.

import { join } from "node:path";

import type { CodexRunner } from "./runner.js";
import { createWorkerRegistry, type WorkerRegistry } from "./registry.js";
import type { Orchestrator, WorkerInfo } from "./types.js";
import type { Summarize } from "./summarize.js";
import { managerSummarizer, withProtocol } from "./protocol.js";
import { logger } from "../logger.js";

export interface OrchestratorDeps {
  runner: CodexRunner;
  /** Default project dir (workspace) when a worker is started without an explicit project. */
  workspaceDir: string;
  /** Push a worker-completion event onto the manager queue. */
  emitEvent: (event: { workerId: string; status: "completed" | "failed"; summary: string }) => void;
  /** Condense over-long worker output (default: clip). */
  summarize?: Summarize;
  /** Called whenever the worker set changes, to mirror it into system/workers.md. */
  onWorkersChanged?: (workers: WorkerInfo[]) => void;
}

export interface WorkerOrchestrator extends Orchestrator {
  registry: WorkerRegistry;
  /** Resolve once every in-flight run has settled (test helper). */
  whenQuiet(): Promise<void>;
}

export function createOrchestrator(deps: OrchestratorDeps): WorkerOrchestrator {
  const registry = createWorkerRegistry();
  // Default condenser keeps only the worker's own summary block (protocol.ts), not a byte-clip of its
  // whole transcript — so the manager gets the worker's intended summary, conclusion included.
  const summarize = deps.summarize ?? managerSummarizer();
  const inflight = new Set<Promise<void>>();
  let counter = 0;

  function mirror(): void {
    deps.onWorkersChanged?.(registry.infos());
  }

  /** Launch (or relaunch) a run for a worker; non-blocking. */
  function launch(id: string, prompt: string, resume: boolean): void {
    const rec = registry.get(id);
    if (!rec) return;
    const abort = new AbortController();
    rec.currentAbort = abort;
    rec.status = "running";

    const promise = deps.runner
      .run({
        // Every worker turn carries the standing protocol (summary-block contract + git guidance).
        prompt: withProtocol(prompt),
        ...(resume && rec.threadId ? { resumeThreadId: rec.threadId } : {}),
        signal: abort.signal,
        onThreadId: (tid) => {
          const r = registry.get(id);
          if (r) r.threadId = tid;
        },
        // Record the worker's latest line while the run is in flight; the final summary overwrites
        // it on settle. Kept as a hook for a future status surface (e.g. a richer subagent_list).
        onProgress: (note) => {
          const r = registry.get(id);
          if (r) r.latest = note;
        },
      })
      .then(
        (turn) => settle(id, abort, turn.ok, turn.threadId, turn.finalResponse),
        (err: Error) => settle(id, abort, false, undefined, err.message),
      )
      .finally(() => {
        inflight.delete(promise);
      });
    inflight.add(promise);
  }

  async function settle(
    id: string,
    abort: AbortController,
    ok: boolean,
    threadId: string | undefined,
    output: string,
  ): Promise<void> {
    const rec = registry.get(id);
    if (!rec) return;
    // Superseded by a steer/cancel: this run was aborted to make way for another → a transition,
    // not a terminal event. Stay silent.
    if (rec.currentAbort !== abort) return;

    if (threadId) rec.threadId = threadId;
    const summary = ok ? await summarize(output) : output;
    rec.latest = summary;
    rec.status = ok ? "idle" : "failed";
    rec.currentAbort = undefined;
    mirror();
    deps.emitEvent({ workerId: id, status: ok ? "completed" : "failed", summary });
  }

  const orch: WorkerOrchestrator = {
    registry,

    start(objective, project) {
      counter += 1;
      const id = `w${counter}`;
      const projectDir = project ? join(deps.workspaceDir, project) : deps.workspaceDir;
      registry.add({ id, purpose: objective, project: projectDir });
      logger.debug("Worker start", { id, project: projectDir });
      launch(id, objective, false);
      mirror();
      return registry.info(id)!;
    },

    send(id, message) {
      const rec = registry.get(id);
      if (!rec) throw new Error(`no such worker: ${id}`);
      launch(id, message, true);
      mirror();
      return registry.info(id)!;
    },

    steer(id, guidance) {
      const rec = registry.get(id);
      if (!rec) throw new Error(`no such worker: ${id}`);
      const old = rec.currentAbort;
      // Launch the resume FIRST (sets a new currentAbort) so the old run's settle is recognized as
      // superseded, THEN abort the old run.
      launch(id, guidance, true);
      old?.abort();
      mirror();
      return registry.info(id)!;
    },

    cancel(id) {
      const rec = registry.get(id);
      if (!rec) throw new Error(`no such worker: ${id}`);
      const old = rec.currentAbort;
      rec.currentAbort = undefined; // any pending settle for `old` is now superseded → silent
      rec.status = "canceled";
      old?.abort();
      mirror();
      return registry.info(id)!;
    },

    list: () => registry.infos(),

    async whenQuiet() {
      while (inflight.size > 0) await Promise.all([...inflight]);
    },
  };

  return orch;
}
