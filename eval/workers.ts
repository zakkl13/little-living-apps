// REAL Codex workers, instrumented. The whole point of an eval here is the manager⇄worker
// interplay, so workers are never scripted or tuned: this is the production `createCodexRunner`
// (same SDK, same sandbox config, same subscription auth) wrapped only to RECORD what flows through
// it — dispatches, live progress notes, and final reports — into the trial timeline for grading.

import { createCodexRunner, type CodexRunner, type CodexTurn } from "../src/workers/runner.js";
import { WORKER_PROTOCOL } from "../src/workers/protocol.js";
import type { Config } from "../src/config.js";
import type { TimelineEvent, WorkerSession } from "./types.js";

/** The orchestrator prepends the standing worker protocol to every prompt. Graders must see the
 *  manager's actual objective, not the protocol boilerplate (which e.g. mentions "verification"
 *  and would make every prompt look like a validation objective). Recording-side only — the real
 *  worker still receives the full prompt untouched. */
export function stripProtocol(prompt: string): string {
  if (prompt.startsWith(WORKER_PROTOCOL)) return prompt.slice(WORKER_PROTOCOL.length).trim();
  const marker = "---- your task ----";
  const idx = prompt.indexOf(marker);
  return idx === -1 ? prompt : prompt.slice(idx + marker.length).trim();
}

export interface InstrumentedWorkers extends CodexRunner {
  /** One fully attributed session per dispatch, in order (the review record's worker lanes).
   *  Mutated in place while workers run; read after the trial drains. */
  sessions: WorkerSession[];
}

export function instrumentWorkers(
  config: Config,
  record: (entry: TimelineEvent) => void,
): InstrumentedWorkers {
  const inner = createCodexRunner(config);
  const sessions: WorkerSession[] = [];

  return {
    sessions,

    async run(args): Promise<CodexTurn> {
      const callId = sessions.length + 1;
      const prompt = stripProtocol(args.prompt);
      const session: WorkerSession = {
        callId,
        prompt,
        promptFull: args.prompt,
        notes: [],
        startedAt: Date.now(),
      };
      sessions.push(session);
      record({ type: "worker_call", callId, prompt });

      const turn = await inner.run({
        ...args,
        onProgress: (note) => {
          session.notes.push({ at: Date.now(), note });
          record({ type: "worker_note", callId, note });
          args.onProgress?.(note);
        },
      });

      session.endedAt = Date.now();
      session.ok = turn.ok;
      session.response = turn.finalResponse;
      if (turn.threadId) session.threadId = turn.threadId;
      record({ type: "worker_done", callId, ok: turn.ok, response: turn.finalResponse });
      return turn;
    },

    loginStatus: () => inner.loginStatus(),
  };
}
