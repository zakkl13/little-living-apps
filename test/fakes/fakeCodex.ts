// In-process fake of CodexRunner. Implements the same seam the real SDK wrapper exposes — no
// subprocess, no JSONL. Preserves the v0.1 contract (echo replies, progress notes, resume echo)
// AND the v0.2 async-worker surface (onThreadId early, AbortSignal, failure/long-output sentinels).
//
// Prompt sentinels:
//   AUTH_FAILURE    -> failed turn, auth-flavored, no thread id
//   WORKER_FAIL     -> failed turn WITH a thread id (a worker that ran but errored)
//   WAIT_FOR_ABORT  -> never completes on its own; only an AbortSignal ends it (steer/cancel)
//   LONG_OUTPUT     -> a 9000-char reply (chunking / summarize fallback)
// On resume, the final message echoes the resumed thread id so tests can prove continuity.

import { friendlyError, type CodexRunner, type CodexTurn } from "../../src/workers/runner.js";

export interface FakeCodexCall {
  prompt: string;
  resumeThreadId?: string;
  hadSignal: boolean;
}

export interface FakeCodex extends CodexRunner {
  calls: FakeCodexCall[];
  authOk: boolean;
}

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 5));

export function makeFakeCodex(opts: { authOk?: boolean } = {}): FakeCodex {
  const calls: FakeCodexCall[] = [];
  let counter = 0;

  const fake: FakeCodex = {
    calls,
    authOk: opts.authOk ?? true,

    async run({ prompt, resumeThreadId, onProgress, signal, onThreadId }): Promise<CodexTurn> {
      calls.push({ prompt, resumeThreadId, hadSignal: Boolean(signal) });

      const threadId = resumeThreadId ?? `thread-${++counter}`;
      onThreadId?.(threadId);

      if (prompt.includes("AUTH_FAILURE")) {
        return { ok: false, finalResponse: friendlyError("401 unauthorized — please login again") };
      }

      onProgress?.("$ echo hello");
      onProgress?.("✏️ 1 file changed");

      if (prompt.includes("WORKER_FAIL")) {
        return { ok: false, threadId, finalResponse: "worker failed: build error in module X" };
      }

      // A run that only ends when aborted (drives steer/cancel tests).
      if (prompt.includes("WAIT_FOR_ABORT")) {
        await new Promise<void>((_resolve, reject) => {
          const fail = (): void => reject(new DOMException("aborted", "AbortError"));
          if (signal?.aborted) return fail();
          signal?.addEventListener("abort", fail, { once: true });
        });
      } else {
        await tick(); // genuinely background
      }

      let finalResponse = `echo: ${prompt}`;
      if (resumeThreadId) finalResponse += ` (resumed ${resumeThreadId})`;
      if (prompt.includes("LONG_OUTPUT")) finalResponse = "X".repeat(9000);

      return { ok: true, threadId, finalResponse };
    },

    async loginStatus() {
      return fake.authOk
        ? { ok: true, detail: "Logged in using ChatGPT (plan: Pro)" }
        : { ok: false, detail: "not logged in" };
    },
  };

  return fake;
}
