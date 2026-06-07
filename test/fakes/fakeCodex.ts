// In-process fake of the CodexRunner interface. Replaces the old fake-codex *binary*: there is
// no subprocess and no JSONL anymore — we implement the same seam the real SDK wrapper exposes.
//
// Prompt sentinels drive edge cases:
//   AUTH_FAILURE  -> a failed turn with an auth-flavored message, no thread id persisted
//   LONG_OUTPUT   -> a 9000-char reply (exercises Telegram chunking)
// On resume, the final message echoes the resumed thread id so tests can prove continuity.

import { friendlyError, type CodexRunner, type CodexTurn } from "../../src/workers/runner.js";

export interface FakeCodexCall {
  prompt: string;
  resumeThreadId?: string;
}

export interface FakeCodex extends CodexRunner {
  calls: FakeCodexCall[];
  authOk: boolean;
}

export function makeFakeCodex(opts: { authOk?: boolean } = {}): FakeCodex {
  const calls: FakeCodexCall[] = [];
  let counter = 0;

  const fake: FakeCodex = {
    calls,
    authOk: opts.authOk ?? true,

    async run({ prompt, resumeThreadId, onProgress }): Promise<CodexTurn> {
      calls.push({ prompt, resumeThreadId });

      if (prompt.includes("AUTH_FAILURE")) {
        return {
          ok: false,
          finalResponse: friendlyError("401 unauthorized — please login again"),
          // no thread id on failure -> handler must not persist one
        };
      }

      // Exercise the streaming/edit path with a couple of progress notes.
      onProgress?.("$ echo hello");
      onProgress?.("✏️ 1 file changed");

      const threadId = resumeThreadId ?? `thread-${++counter}`;
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
