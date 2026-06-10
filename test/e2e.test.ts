// Headline end-to-end (MIGRATION-CODEX.md §14). The FULL runtime loop runs against fakes only: a
// fake manager backend (each turn drives the REAL Lila MCP tool handlers against live memory + the
// orchestrator), an in-process Codex worker runner, and a fake Telegram — over REAL memory (git +
// sqlite), the real serialized queue/loop, and the real long-poll loop. Nothing is deployed.
//
// Scenario: owner message → manager turn → subagent_start ×2 (parallel, prompt-scoped) → workers
// complete → worker_events → manager records a decision in memory and narrates → Telegram. Asserts
// memory writes land in MemFS, the worker prompts carried their scopes, the manager thread id is
// snapshotted, and a simulated cold wake loses nothing.

import { strict as assert } from "node:assert";
import { afterEach, describe, it } from "node:test";

import { startBot, messageUpdate, type TestBot } from "./helpers.js";
import { say, startWorkers, type ManagerStep } from "./fakes/fakeManager.js";
import { openSnapshotStore } from "../src/runtime/snapshot.js";

const bots: TestBot[] = [];
afterEach(async () => {
  while (bots.length) await bots.pop()!.close();
});
async function boot(...args: Parameters<typeof startBot>): Promise<TestBot> {
  const bot = await startBot(...args);
  bots.push(bot);
  return bot;
}

describe("e2e: owner → manager → parallel workers → narrate", () => {
  it("dispatches two prompt-scoped workers and narrates their completions", async () => {
    const recordDecision: ManagerStep = async (ctx) => {
      await ctx.call("memory_create", {
        path: "/memories/archival/decisions/stack.md",
        file_text: "---\ndescription: web framework\n---\nChose Fastify for the API.\n",
      });
      await ctx.say("✅ api worker done; recorded the decision");
    };

    const bot = await boot({
      script: [
        // Turn 1: spawn two non-overlapping workers, then ack.
        startWorkers(
          [
            { objective: "work only within src/api/**", project: "proj" },
            { objective: "work only within test/**", project: "proj" },
          ],
          "on it — two workers: src/api and test/",
        ),
        // Turn 2: first worker_event → record a decision in memory, then narrate.
        recordDecision,
        // Turn 3: second worker_event → narrate.
        say("✅ test worker done"),
      ],
    });

    bot.sendUpdate(messageUpdate("build the API and its tests in parallel"));
    await bot.telegram.waitFor(() => bot.telegram.sent.length >= 3);
    await bot.app.orchestrator.whenQuiet();
    await new Promise((r) => setTimeout(r, 20)); // let the final snapshot settle

    // Two workers were actually dispatched, each scoped to a non-overlapping subtree.
    assert.equal(bot.codex.calls.length, 2, "exactly two workers ran");
    const prompts = bot.codex.calls.map((c) => c.prompt).join("\n");
    assert.match(prompts, /src\/api/);
    assert.match(prompts, /test\//);

    // The manager's memory-tool write landed in real MemFS (git + sqlite).
    assert.equal(bot.app.mem.search("Fastify").length, 1, "decision recorded in memory");

    // The owner saw the ack and both completions.
    const texts = bot.telegram.sent.map((m) => m.text);
    assert.ok(texts.some((t) => /two workers/.test(t)));
    assert.ok(texts.some((t) => /api worker done/.test(t)));
    assert.ok(texts.some((t) => /test worker done/.test(t)));

    // The manager thread id was snapshotted (Codex resumes it on cold wake — §7).
    const snap = openSnapshotStore(bot.config.managerStateDir).load()!;
    assert.ok(snap.managerThreadId, "manager thread id persisted");
  });
});

describe("e2e: cold-wake recovery", () => {
  it("a fresh process restores memory and keeps serving", async () => {
    // --- instance #1: write a durable fact to core memory, then die ---
    const bot1 = await boot({
      script: [
        async (ctx) => {
          await ctx.call("memory_create", {
            path: "/memories/system/owner.md",
            file_text: "---\ndescription: owner profile\n---\nOwner is zakk; prefers terse replies.\n",
          });
          await ctx.say("saved your profile");
        },
      ],
    });
    bot1.sendUpdate(messageUpdate("remember I prefer terse replies"));
    await bot1.telegram.waitFor(() => bot1.telegram.sent.length >= 1);
    await new Promise((r) => setTimeout(r, 20));
    assert.ok(bot1.app.mem.readRelative("system/owner.md"), "memory written");
    await bot1.close();
    bots.length = 0; // bot1 already closed

    // --- instance #2: fresh process, SAME memory/state dirs → auto-restores on boot ---
    const bot2 = await boot({
      configOverrides: {
        MEMORY_DIR: bot1.config.memoryDir,
        MANAGER_STATE_DIR: bot1.config.managerStateDir,
        WORKSPACE_DIR: bot1.config.workspaceDir,
      },
      script: [say("still here — terse it is")],
    });

    bot2.sendUpdate(messageUpdate("you there?"));
    await bot2.telegram.waitFor(() => bot2.telegram.sent.length >= 1);

    assert.ok(bot2.telegram.sent.some((m) => /still here/.test(m.text)));
    // Core memory (git) survived: the owner profile is on disk for the next turn's context header.
    assert.match(bot2.app.mem.loadSystem(), /prefers terse replies/);
  });
});

describe("e2e: transport guards", () => {
  it("rejects a non-allowlisted user and never runs the manager", async () => {
    const bot = await boot();
    bot.sendUpdate(messageUpdate("hello", { fromId: 99999999 }));
    await bot.telegram.waitFor(() => bot.telegram.sent.length >= 1);
    await new Promise((r) => setTimeout(r, 30));
    assert.match(bot.telegram.sent[0]!.text, /not authorized/i);
    assert.equal(bot.manager.turns, 0, "the manager never ran");
    assert.equal(bot.codex.calls.length, 0);
  });

  it("answers /status without invoking the manager", async () => {
    const bot = await boot();
    bot.sendUpdate(messageUpdate("/status"));
    await bot.telegram.waitFor(() => bot.telegram.sent.length >= 1);
    assert.match(bot.telegram.sent[0]!.text, /Workers:/);
    assert.equal(bot.manager.turns, 0);
  });
});
