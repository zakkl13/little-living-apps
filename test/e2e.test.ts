// End-to-end test of the full Telegram -> bot -> Codex -> Telegram loop, with NO real Sprite,
// NO real Telegram, and NO real Codex/ChatGPT. The bot runs as a local process; Telegram is a
// fake HTTP server; Codex is an in-process CodexRunner fake. Covers most of SPEC §12.

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { openSessionStore } from "../src/sessions.js";
import { startFakeTelegram, type FakeTelegram } from "./fakes/fakeTelegram.js";
import {
  ALLOWED_USER_ID,
  buildConfig,
  makeFakeCodex,
  messageUpdate,
  startBot,
  type FakeCodex,
  type TestBot,
} from "./helpers.js";

async function setup(overrides: Record<string, string> = {}): Promise<{
  fake: FakeTelegram;
  codex: FakeCodex;
  bot: TestBot;
  cleanup: () => Promise<void>;
}> {
  const fake = await startFakeTelegram();
  const config = buildConfig({ TELEGRAM_API_BASE_URL: fake.baseUrl, ...overrides });
  const codex = makeFakeCodex();
  const bot = await startBot(config, codex);
  return {
    fake,
    codex,
    bot,
    cleanup: async () => {
      await bot.close();
      await fake.close();
    },
  };
}

describe("e2e: Telegram <-> Codex loop", () => {
  it("rejects a non-allowlisted user and never runs Codex", async () => {
    const { fake, codex, bot, cleanup } = await setup();
    try {
      const res = await bot.postUpdate(messageUpdate("hello", { fromId: 99999999 }));
      assert.equal(res.status, 200); // we still 200 Telegram
      await fake.waitFor(() => fake.sent.length >= 1);
      await new Promise((r) => setTimeout(r, 50)); // settle to catch erroneous extras
      assert.equal(fake.sent.length, 1);
      assert.match(fake.sent[0]!.text, /not authorized/i);
      assert.equal(codex.calls.length, 0, "Codex must not run for an unauthorized user");
      assert.equal(bot.store.get(99999999), undefined);
    } finally {
      await cleanup();
    }
  });

  it("first turn creates a thread and replies with Codex output", async () => {
    const { fake, bot, cleanup } = await setup();
    try {
      await bot.postUpdate(messageUpdate("what is 2+2"));
      await fake.waitFor(() => fake.sent.length >= 2);

      assert.equal(fake.sent[0]!.text, "⏳ Working…");
      assert.match(fake.sent[1]!.text, /^echo: what is 2\+2/);

      const threadId = bot.store.get(ALLOWED_USER_ID);
      assert.ok(threadId, "thread id should be persisted after first turn");
    } finally {
      await cleanup();
    }
  });

  it("streams live progress by editing the status message", async () => {
    const { fake, bot, cleanup } = await setup();
    try {
      await bot.postUpdate(messageUpdate("do some work"));
      await fake.waitFor(() => fake.sent.length >= 2);
      await new Promise((r) => setTimeout(r, 30));

      // The status message (first sent) was edited with progress, then collapsed to "Done".
      assert.ok(fake.edits.length >= 1, "expected at least one status edit");
      assert.ok(
        fake.edits.some((e) => /echo hello/.test(e.text)),
        "a streamed progress line should appear in an edit",
      );
      assert.ok(
        fake.edits.some((e) => /✅ Done/.test(e.text)),
        "status should collapse to Done",
      );
      // All edits target the first message id (the status message).
      assert.ok(fake.edits.every((e) => e.messageId === fake.sent[0]!.messageId));
    } finally {
      await cleanup();
    }
  });

  it("holds the Sprite awake for the whole turn and releases it after", async () => {
    const { fake, bot, cleanup } = await setup();
    try {
      await bot.postUpdate(messageUpdate("keep me awake"));
      await fake.waitFor(() => fake.sent.length >= 2);
      await new Promise((r) => setTimeout(r, 30));
      assert.equal(bot.hold.acquired, 1, "a keep-alive hold should be acquired per turn");
      assert.equal(bot.hold.released, 1, "the hold should be released when the turn ends");
      assert.equal(bot.hold.held, 0, "no hold should leak after the turn");
    } finally {
      await cleanup();
    }
  });

  it("releases the keep-alive hold even when the turn fails", async () => {
    const { fake, bot, cleanup } = await setup();
    try {
      await bot.postUpdate(messageUpdate("trigger AUTH_FAILURE"));
      await fake.waitFor(() => fake.sent.length >= 2);
      await new Promise((r) => setTimeout(r, 30));
      assert.equal(bot.hold.held, 0, "a failed turn must not leak the hold");
      assert.equal(bot.hold.released, 1);
    } finally {
      await cleanup();
    }
  });

  it("resumes the SAME thread on a follow-up (stable thread id)", async () => {
    const { fake, codex, bot, cleanup } = await setup();
    try {
      await bot.postUpdate(messageUpdate("first"));
      await fake.waitFor(() => fake.sent.length >= 2);
      const threadId = bot.store.get(ALLOWED_USER_ID)!;
      assert.ok(threadId);

      await bot.postUpdate(messageUpdate("second"));
      await fake.waitFor(() => fake.sent.length >= 4);

      // The runner was invoked with the stored thread id, and echoes it back -> continuity.
      assert.equal(codex.calls.at(-1)!.resumeThreadId, threadId);
      assert.match(fake.sent[3]!.text, new RegExp(`resumed ${threadId}`));
      assert.equal(bot.store.get(ALLOWED_USER_ID), threadId, "thread id stays stable");
    } finally {
      await cleanup();
    }
  });

  it("/new drops the thread so the next message starts fresh", async () => {
    const { fake, bot, cleanup } = await setup();
    try {
      await bot.postUpdate(messageUpdate("first"));
      await fake.waitFor(() => fake.sent.length >= 2);
      const firstId = bot.store.get(ALLOWED_USER_ID)!;

      await bot.postUpdate(messageUpdate("/new"));
      await fake.waitFor(() => fake.sent.some((m) => /fresh thread/i.test(m.text)));
      assert.equal(bot.store.get(ALLOWED_USER_ID), undefined);

      await bot.postUpdate(messageUpdate("again"));
      await fake.waitFor(() => fake.sent.filter((m) => /^echo:/.test(m.text)).length >= 2);
      const newId = bot.store.get(ALLOWED_USER_ID)!;
      assert.notEqual(newId, firstId, "a brand new thread id should be issued");
    } finally {
      await cleanup();
    }
  });

  it("/status reports auth, thread and sandbox", async () => {
    const { fake, bot, cleanup } = await setup();
    try {
      await bot.postUpdate(messageUpdate("/status"));
      await fake.waitFor(() => fake.sent.length >= 1);
      const text = fake.sent[0]!.text;
      assert.match(text, /Auth: ✅/);
      assert.match(text, /subscription/i);
      assert.match(text, /Sandbox: danger-full-access/);
    } finally {
      await cleanup();
    }
  });

  it("chunks replies longer than 4096 chars instead of truncating", async () => {
    const { fake, bot, cleanup } = await setup();
    try {
      await bot.postUpdate(messageUpdate("please LONG_OUTPUT now"));
      // ack + 3 chunks (9000 chars / 4096)
      await fake.waitFor(() => fake.sent.length >= 4);
      await new Promise((r) => setTimeout(r, 50));

      const chunks = fake.sent.slice(1);
      assert.equal(chunks.length, 3);
      assert.ok(chunks.every((m) => m.text.length <= 4096));
      assert.equal(
        chunks.reduce((n, m) => n + m.text.length, 0),
        9000,
        "no characters lost across chunks",
      );
    } finally {
      await cleanup();
    }
  });

  it("surfaces a Codex auth failure as a re-auth hint (not a silent failure)", async () => {
    const { fake, bot, cleanup } = await setup();
    try {
      await bot.postUpdate(messageUpdate("trigger AUTH_FAILURE"));
      await fake.waitFor(() => fake.sent.length >= 2);
      assert.match(fake.sent[1]!.text, /auth|login/i);
      // failed run must not persist a thread id
      assert.equal(bot.store.get(ALLOWED_USER_ID), undefined);
    } finally {
      await cleanup();
    }
  });

  it("rejects webhook POSTs with a bad secret token (401, no processing)", async () => {
    const { fake, bot, cleanup } = await setup();
    try {
      const res = await bot.postUpdate(messageUpdate("hello"), { secret: "wrong" });
      assert.equal(res.status, 401);
      await new Promise((r) => setTimeout(r, 50));
      assert.equal(fake.sent.length, 0);
    } finally {
      await cleanup();
    }
  });

  it("thread survives a process restart (state was on disk)", async () => {
    const { fake, bot, cleanup } = await setup();
    try {
      await bot.postUpdate(messageUpdate("remember me"));
      await fake.waitFor(() => fake.sent.length >= 2);
      const threadId = bot.store.get(ALLOWED_USER_ID)!;

      // Simulate hibernate->wake: kill the bot, re-read the store from disk.
      await bot.close();
      const reopened = openSessionStore(bot.config.sessionStorePath);
      assert.equal(reopened.get(ALLOWED_USER_ID), threadId, "thread survived restart");

      // A fresh bot process (fresh runner) on the same store resumes the same thread.
      const bot2 = await startBot(bot.config, makeFakeCodex());
      try {
        fake.reset();
        await bot2.postUpdate(messageUpdate("still there?"));
        await fake.waitFor(() => fake.sent.length >= 2);
        assert.match(fake.sent[1]!.text, new RegExp(`resumed ${threadId}`));
      } finally {
        await bot2.close();
      }
    } finally {
      await fake.close();
    }
  });
});
