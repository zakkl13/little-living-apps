// Durability (MIGRATION-CODEX.md §7). Snapshots are written after every turn; a rebuilt app restores
// the manager thread id (to resume the Codex rollout), the pending queue, and worker records from the
// last snapshot — a simulated cold wake loses nothing. v3 drops the ModelMessage transcript: Codex
// owns the thread's rollout on disk.

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
  it("round-trips a v3 snapshot (thread id + queue + workers)", () => {
    const config = buildConfig();
    const store = openSnapshotStore(config.managerStateDir);
    const snap: ManagerSnapshot = {
      version: 3,
      managerThreadId: "thread-xyz",
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

  it("ignores a pre-v3 (Opus) snapshot so a fresh thread starts", () => {
    const config = buildConfig();
    const store = openSnapshotStore(config.managerStateDir);
    // Hand-write a v2 file the way the old runtime would have.
    openSnapshotStore(config.managerStateDir).save({
      // deliberately the old shape
      version: 2 as unknown as 3,
      queue: [],
      workers: [],
    } as ManagerSnapshot);
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

  it("rehydrates worker records (running → idle, thread id preserved)", async () => {
    const config = buildConfig();

    const { app: app1 } = await buildApp(config, makeFakeManager());
    const info = app1.orchestrator.start("scope A", "proj");
    await app1.orchestrator.whenQuiet();
    app1.persist();
    const threadId = app1.orchestrator.registry.get(info.id)!.threadId;
    await app1.close();

    const { app: app2 } = await buildApp(config, makeFakeManager());
    app2.restore();
    const rec = app2.orchestrator.registry.get(info.id);
    assert.ok(rec, "worker rehydrated");
    assert.equal(rec!.threadId, threadId, "codex thread id preserved across restart");
  });
});
