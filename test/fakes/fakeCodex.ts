// In-process fake of CodexRunner. Implements the same seam the real SDK wrapper exposes — no
// subprocess, no JSONL. Every run is a fresh single-shot thread (workers are purely ephemeral),
// echo replies + progress notes preserved from v0.1.
//
// Prompt sentinels:
//   AUTH_FAILURE    -> failed turn, auth-flavored, no thread id
//   WORKER_FAIL     -> failed turn WITH a thread id (a worker that ran but errored)
//   LONG_OUTPUT     -> a 9000-char reply (chunking / summarize fallback)

import { friendlyError, type CodexRunner, type CodexTurn } from "../../src/workers/runner.js";

export interface FakeCodexCall {
  prompt: string;
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

    async run({ prompt, onProgress }): Promise<CodexTurn> {
      calls.push({ prompt });

      const threadId = `thread-${++counter}`;

      if (prompt.includes("AUTH_FAILURE")) {
        return { ok: false, finalResponse: friendlyError("401 unauthorized — please login again") };
      }

      onProgress?.("$ echo hello");
      onProgress?.("✏️ 1 file changed");

      if (prompt.includes("WORKER_FAIL")) {
        return { ok: false, threadId, finalResponse: "worker failed: build error in module X" };
      }

      await tick(); // genuinely background

      let finalResponse = `echo: ${prompt}`;
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
