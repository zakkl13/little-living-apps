// Headline end-to-end (DESIGN §13). The FULL runtime loop runs against fakes only: scripted
// Anthropic, in-process Codex, fake Telegram — over REAL memory (git + sqlite), the
// real serialized queue/loop, and the real webhook. Nothing is deployed.
//
// Scenario: owner message → manager turn → subagent_start ×2 (parallel, prompt-scoped) → workers
// complete → worker_events → manager narrates as plain text → Telegram. Asserts memory-tool writes land in
// MemFS, compaction blocks round-trip, and a simulated cold wake loses nothing.

import { strict as assert } from "node:assert";
import { afterEach, describe, it } from "node:test";

import { startBot, messageUpdate, type TestBot } from "./helpers.js";
import { resp, text, toolUse, compaction } from "./fakes/fakeAnthropic.js";

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
    const bot = await boot({
      script: [
        // Turn 1: spawn two non-overlapping workers (a compaction block rides along).
        resp([
          compaction("cmp_E2E"),
          toolUse("subagent_start", { objective: "work only within src/api/**", project: "proj" }),
          toolUse("subagent_start", { objective: "work only within test/**", project: "proj" }),
        ]),
        // …then ack as plain text, which ends the turn.
        resp([text("on it — two workers: src/api and test/")]),
        // Turn 2: first worker_event → record a decision in memory, then narrate it.
        resp([
          toolUse("memory", {
            command: "create",
            path: "/memories/archival/decisions/stack.md",
            file_text: "---\ndescription: web framework\n---\nChose Fastify for the API.\n",
          }),
        ]),
        resp([text("✅ api worker done; recorded the decision")]),
        // Turn 3: second worker_event → narrate.
        resp([text("✅ test worker done")]),
        resp([]), // safety buffer
      ],
    });

    bot.sendUpdate(messageUpdate("build the API and its tests in parallel"));
    // Wait for all three narrations (ack + two completions).
    await bot.telegram.waitFor(() => bot.telegram.sent.length >= 3);
    await bot.app.orchestrator.whenQuiet();
    await new Promise((r) => setTimeout(r, 20)); // let the final end_turn + snapshot settle

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

    // The compaction block round-tripped through every later request (DESIGN §4).
    const lastReq = bot.anthropic.requests.at(-1)!;
    const survived = lastReq.messages.flatMap((m) => m.content).find((b) => b.type === "compaction");
    assert.ok(survived, "compaction block survived across turns");
    assert.equal((survived as unknown as { id: string }).id, "cmp_E2E");
  });
});

describe("e2e: cold-wake recovery", () => {
  it("a fresh process restores memory, transcript, and compaction state", async () => {
    // --- instance #1: write a durable fact to core memory, then die ---
    const bot1 = await boot({
      script: [
        resp([
          toolUse("memory", {
            command: "create",
            path: "/memories/system/owner.md",
            file_text: "---\ndescription: owner profile\n---\nOwner is zakk; prefers terse replies.\n",
          }),
          compaction("cmp_CW"),
          text("saved your profile"),
        ]),
        resp([]),
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
      script: [resp([text("still here — terse it is")])],
    });

    bot2.sendUpdate(messageUpdate("you there?"));
    await bot2.telegram.waitFor(() => bot2.telegram.sent.length >= 1);

    assert.ok(bot2.telegram.sent.some((m) => /still here/.test(m.text)));
    const firstReq = bot2.anthropic.requests[0]!;
    // Core memory (git) survived: the owner profile is injected into the system prompt.
    assert.match(firstReq.system, /prefers terse replies/);
    // Transcript snapshot survived: the compaction block from instance #1 round-tripped.
    const survived = firstReq.messages.flatMap((m) => m.content).find((b) => b.type === "compaction");
    assert.ok(survived, "compaction block survived the restart");
    assert.equal((survived as unknown as { id: string }).id, "cmp_CW");
  });
});

describe("e2e: transport guards", () => {
  it("rejects a non-allowlisted user and never runs the model", async () => {
    const bot = await boot();
    bot.sendUpdate(messageUpdate("hello", { fromId: 99999999 }));
    await bot.telegram.waitFor(() => bot.telegram.sent.length >= 1);
    await new Promise((r) => setTimeout(r, 30));
    assert.match(bot.telegram.sent[0]!.text, /not authorized/i);
    assert.equal(bot.anthropic.requests.length, 0, "the manager model never ran");
    assert.equal(bot.codex.calls.length, 0);
  });

  it("answers /status without invoking the model", async () => {
    const bot = await boot();
    bot.sendUpdate(messageUpdate("/status"));
    await bot.telegram.waitFor(() => bot.telegram.sent.length >= 1);
    assert.match(bot.telegram.sent[0]!.text, /Workers:/);
    assert.equal(bot.anthropic.requests.length, 0);
  });
});
