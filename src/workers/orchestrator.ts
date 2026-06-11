// The async worker tier (DESIGN §6), purely ephemeral. start() launches a single-shot Codex run in
// the background and returns immediately; when the run settles, ONE worker_event lands on the queue
// (which opens a later manager turn) and the worker is gone. No registry, no resume, no steer, no
// roster — a worker's output is its summary, and everything else it produced lives in the workspace
// and git history, where the next worker can find it.

import { join } from "node:path";

import type { CodexRunner } from "./runner.js";
import type { Orchestrator } from "./types.js";
import type { Summarize } from "./summarize.js";
import { managerSummarizer, withProtocol } from "./protocol.js";
import { logger } from "../logger.js";

export interface OrchestratorDeps {
  runner: CodexRunner;
  /** Default project dir (workspace) when a worker is started without an explicit project. */
  workspaceDir: string;
  /** Push the worker's one completion event onto the manager queue. */
  emitEvent: (event: {
    workerId: string;
    objective: string;
    status: "completed" | "failed";
    summary: string;
  }) => void;
  /** Condense over-long worker output (default: keep the worker's own summary block). */
  summarize?: Summarize;
}

export function createOrchestrator(deps: OrchestratorDeps): Orchestrator {
  // Default condenser keeps only the worker's own summary block (protocol.ts), not a byte-clip of its
  // whole transcript — so the manager gets the worker's intended summary, conclusion included.
  const summarize = deps.summarize ?? managerSummarizer();
  const inflight = new Set<Promise<void>>();
  let counter = 0;

  return {
    start(objective, project) {
      counter += 1;
      const id = `w${counter}`;
      const projectDir = project ? join(deps.workspaceDir, project) : deps.workspaceDir;
      logger.debug("Worker start", { id, project: projectDir });

      // Retire the run BEFORE emitting its event: the event wakes the manager loop synchronously,
      // and the manager's reply gate reads running() — if the settled run still counted as in
      // flight, the gate would swallow the final report (the worker would look busy from beyond
      // the grave).
      const settle = (status: "completed" | "failed", summary: string): void => {
        inflight.delete(promise);
        deps.emitEvent({ workerId: id, objective, status, summary });
      };
      const promise: Promise<void> = deps.runner
        .run({
          // Every worker run carries the standing protocol (summary-block contract + git guidance).
          prompt: withProtocol(objective),
        })
        .then(
          async (turn) => {
            const summary = turn.ok ? await summarize(turn.finalResponse) : turn.finalResponse;
            settle(turn.ok ? "completed" : "failed", summary);
          },
          (err: Error) => settle("failed", err.message),
        );
      inflight.add(promise);
      return { id };
    },

    running: () => inflight.size,

    async whenQuiet() {
      while (inflight.size > 0) await Promise.all([...inflight]);
    },
  };
}
