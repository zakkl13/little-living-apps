// Durability (MIGRATION-CODEX.md §7). Snapshots are written after every turn; a rebuilt app restores
// the manager thread id (to resume the Codex rollout) and the pending queue from the last snapshot —
// a simulated cold wake loses nothing. v4 carries no worker records (workers are ephemeral; only
// their queued completion events persist) and no ModelMessage transcript: Codex owns the rollout.

import { strict as assert } from "node:assert";
import { describe, it, afterEach } from "node:test";

import { createManagerApp, type ManagerApp } from "../src/app.js";
import { openSnapshotStore, type ManagerSnapshot } from "../src/runtime/snapshot.js";
import { makeFakeCodex } from "./fakes/fakeCodex.js";
import { makeFakeManager, say, type FakeManager } from "./fakes/fakeManager.js";
import { buildConfig, ALLOWED_USER_ID } from "./helpers.js";
import type { Config } from "../src/config.js";

const apps: ManagerApp[] = [];
afterEach(async () => {
  while (apps.length) await apps.pop()!.close();
});

async function buildApp(
  config: Config,
  manager: FakeManager,
): Promise<{ app: ManagerApp; sent: Array<{ text: string }> }> {
  const sent: Array<{ text: string }> = [];
  const app = await createManagerApp({
    config,
    runner: makeFakeCodex(),
    deliver: async (_chatId, t) => {
      sent.push({ text: t });
    },
    backendFactory: manager.factory,
  });
  apps.push(app);
  return { app, sent };
}

describe("snapshot store", () => {
  it("round-trips a v4 snapshot (thread id + queue; no worker roster — workers are ephemeral)", () => {
    const config = buildConfig();
    const store = openSnapshotStore(config.managerStateDir);
    const snap: ManagerSnapshot = {
      version: 4,
      managerThreadId: "thread-xyz",
      queue: [{ kind: "owner_message", id: "evt_1", chatId: 7, text: "pending" }],
    };
    store.save(snap);
    assert.deepEqual(store.load(), snap);
  });

  it("returns undefined when no snapshot exists", () => {
    const config = buildConfig();
    assert.equal(openSnapshotStore(config.managerStateDir).load(), undefined);
  });

  it("ignores a pre-v4 snapshot so a fresh thread starts", () => {
    const config = buildConfig();
    const store = openSnapshotStore(config.managerStateDir);
    // Hand-write a v3 file the way the old runtime would have (worker roster included).
    openSnapshotStore(config.managerStateDir).save({
      // deliberately the old shape
      version: 3 as unknown as 4,
      queue: [],
      workers: [],
    } as unknown as ManagerSnapshot);
    assert.equal(store.load(), undefined, "old snapshot discarded");
  });
});

describe("cold-wake recovery (MIGRATION-CODEX.md §7)", () => {
  it("persists after each turn and restores the manager thread id", async () => {
    const config = buildConfig();

    // --- app instance #1: one turn, then die ---
    const { app: app1, sent: sent1 } = await buildApp(config, makeFakeManager([say("ack")]));
    app1.start();
    app1.enqueueOwner(ALLOWED_USER_ID, "first message");
    await app1.loop.whenIdle();
    assert.deepEqual(sent1.map((m) => m.text), ["ack"]);
    await app1.close();

    const onDisk = openSnapshotStore(config.managerStateDir).load()!;
    assert.ok(onDisk.managerThreadId, "manager thread id persisted to snapshot");
    const threadId = onDisk.managerThreadId;

    // --- app instance #2: fresh process, same dirs → restore the thread id and continue ---
    const { app: app2, sent: sent2 } = await buildApp(config, makeFakeManager([say("continued")]));
    app2.restore();
    app2.start();
    app2.enqueueOwner(ALLOWED_USER_ID, "second message");
    await app2.loop.whenIdle();

    assert.deepEqual(sent2.map((m) => m.text), ["continued"]);
    // The thread id from instance #1 survived the restart (Codex resumes the same rollout).
    const after = openSnapshotStore(config.managerStateDir).load()!;
    assert.equal(after.managerThreadId, threadId, "manager thread id preserved across restart");
  });

  it("resumes a pending queued event after a restart", async () => {
    const config = buildConfig();

    // Enqueue but never drain (loop not started), then persist and die.
    const { app: app1 } = await buildApp(config, makeFakeManager());
    app1.enqueueOwner(ALLOWED_USER_ID, "queued before crash");
    app1.persist();
    await app1.close();

    // New instance restores the pending event and drains it on start.
    const { app: app2, sent } = await buildApp(config, makeFakeManager([say("drained on restart")]));
    app2.restore();
    app2.start();
    await app2.loop.whenIdle();
    assert.deepEqual(sent.map((m) => m.text), ["drained on restart"]);
  });

  it("persists a settled worker's completion event so it survives a restart (workers themselves don't)", async () => {
    const config = buildConfig();

    // Worker settles, its event lands on the queue, but the loop never drains it — then we die.
    const { app: app1 } = await buildApp(config, makeFakeManager());
    app1.orchestrator.start("scope A", "proj");
    await app1.orchestrator.whenQuiet();
    app1.persist();
    await app1.close();

    // The fresh instance restores the queued worker_event and the manager turn narrates it —
    // ephemeral workers leave no roster to rehydrate, but their reports are never lost.
    const { app: app2, sent } = await buildApp(config, makeFakeManager([say("worker report handled")]));
    app2.restore();
    app2.start();
    await app2.loop.whenIdle();
    assert.deepEqual(sent.map((m) => m.text), ["worker report handled"]);
  });
});
