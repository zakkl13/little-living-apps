// Phase 5: durability. Snapshots are written after every turn; a rebuilt app restores transcript
// (incl. compaction blocks, verbatim), the pending queue, and worker records from the last
// snapshot — a simulated cold wake loses nothing (DESIGN §11, §4).

import { strict as assert } from "node:assert";
import { describe, it, afterEach } from "node:test";

import { createManagerApp, type ManagerApp } from "../src/app.js";
import { openSnapshotStore, type ManagerSnapshot } from "../src/runtime/snapshot.js";
import { noopHold } from "../src/runtime/hold.js";
import { makeFakeCodex } from "./fakes/fakeCodex.js";
import { makeFakeAnthropic, resp, text, compaction, type FakeAnthropic } from "./fakes/fakeAnthropic.js";
import { buildConfig, ALLOWED_USER_ID } from "./helpers.js";
import type { Config } from "../src/config.js";

const apps: ManagerApp[] = [];
afterEach(async () => {
  while (apps.length) await apps.pop()!.close();
});

function buildApp(config: Config, fake: FakeAnthropic): { app: ManagerApp; sent: Array<{ text: string }> } {
  const sent: Array<{ text: string }> = [];
  const app = createManagerApp({
    config,
    model: fake,
    runner: makeFakeCodex(),
    hold: noopHold,
    deliver: async (_chatId, t) => {
      sent.push({ text: t });
    },
  });
  apps.push(app);
  return { app, sent };
}

describe("snapshot store", () => {
  it("round-trips a snapshot including compaction blocks", () => {
    const config = buildConfig();
    const store = openSnapshotStore(config.managerStateDir);
    const snap: ManagerSnapshot = {
      version: 1,
      transcript: [
        { role: "user", content: [{ type: "text", text: "hi" }] },
        { role: "assistant", content: [{ type: "compaction", id: "cmp_1" }, { type: "text", text: "ok" }] },
      ],
      queue: [{ kind: "owner_message", id: "evt_1", chatId: 7, text: "pending" }],
      workers: [{ id: "w1", purpose: "p", project: "/w", status: "idle", threadId: "thread-1" }],
    };
    store.save(snap);
    assert.deepEqual(store.load(), snap);
  });

  it("returns undefined when no snapshot exists", () => {
    const config = buildConfig();
    assert.equal(openSnapshotStore(config.managerStateDir).load(), undefined);
  });
});

describe("cold-wake recovery (DESIGN §11)", () => {
  it("persists after each turn and restores transcript with compaction blocks verbatim", async () => {
    const config = buildConfig();

    // --- app instance #1: one turn that produces a compaction block, then dies ---
    const fake1 = makeFakeAnthropic([resp([compaction("cmp_LIVE"), text("ack")])]);
    const { app: app1, sent: sent1 } = buildApp(config, fake1);
    app1.start();
    app1.enqueueOwner(ALLOWED_USER_ID, "first message");
    await app1.loop.whenIdle();
    assert.deepEqual(sent1.map((m) => m.text), ["ack"]);
    await app1.close();

    // The snapshot on disk carries the compaction block.
    const onDisk = openSnapshotStore(config.managerStateDir).load()!;
    const compactionOnDisk = onDisk.transcript.flatMap((m) => m.content).find((b) => b.type === "compaction");
    assert.ok(compactionOnDisk, "compaction block persisted to snapshot");

    // --- app instance #2: fresh process, same dirs → restore and continue ---
    const fake2 = makeFakeAnthropic([resp([text("continued")])]);
    const { app: app2, sent: sent2 } = buildApp(config, fake2);
    app2.restore();
    app2.start();
    app2.enqueueOwner(ALLOWED_USER_ID, "second message");
    await app2.loop.whenIdle();

    assert.deepEqual(sent2.map((m) => m.text), ["continued"]);
    // The compaction block from instance #1 round-tripped into instance #2's first request.
    const firstReq = fake2.requests[0]!;
    const survived = firstReq.messages.flatMap((m) => m.content).find((b) => b.type === "compaction");
    assert.ok(survived, "compaction block survived the restart");
    assert.equal((survived as unknown as { id: string }).id, "cmp_LIVE");
  });

  it("resumes a pending queued event after a restart", async () => {
    const config = buildConfig();

    // Enqueue but never drain (loop not started), then persist and die.
    const { app: app1 } = buildApp(config, makeFakeAnthropic());
    app1.enqueueOwner(ALLOWED_USER_ID, "queued before crash");
    app1.persist();
    await app1.close();

    // New instance restores the pending event and drains it on start.
    const fake2 = makeFakeAnthropic([resp([text("drained on restart")])]);
    const { app: app2, sent } = buildApp(config, fake2);
    app2.restore();
    app2.start();
    await app2.loop.whenIdle();
    assert.deepEqual(sent.map((m) => m.text), ["drained on restart"]);
  });

  it("rehydrates worker records (running → idle, thread id preserved)", async () => {
    const config = buildConfig();

    const { app: app1 } = buildApp(config, makeFakeAnthropic());
    const info = app1.orchestrator.start("scope A", "proj");
    await app1.orchestrator.whenQuiet();
    app1.persist();
    const threadId = app1.orchestrator.registry.get(info.id)!.threadId;
    await app1.close();

    const { app: app2 } = buildApp(config, makeFakeAnthropic());
    app2.restore();
    const rec = app2.orchestrator.registry.get(info.id);
    assert.ok(rec, "worker rehydrated");
    assert.equal(rec!.threadId, threadId, "codex thread id preserved across restart");
  });
});
