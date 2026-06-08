// Phase 2: the Telegram long-poll loop. Drives the real poller over a scripted getUpdates and
// asserts the two invariants that matter: updates are ingested in order, and the confirmation
// offset advances to last_update_id + 1 after each batch. Also that a failed poll backs off and
// recovers rather than killing the loop.

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";

import { startPoller } from "../src/transport/poller.js";
import type { GetUpdatesOptions, TelegramUpdate } from "../src/transport/telegram.js";

const u = (id: number): TelegramUpdate => ({ update_id: id });

async function waitFor(predicate: () => boolean, timeoutMs = 2000): Promise<void> {
  const start = Date.now();
  while (!predicate()) {
    if (Date.now() - start > timeoutMs) throw new Error("waitFor timed out");
    await new Promise((r) => setTimeout(r, 5));
  }
}

describe("Telegram long-poll", () => {
  it("ingests updates in order and advances the offset past each batch", async () => {
    const batches: TelegramUpdate[][] = [[u(1), u(2)], [u(3)]];
    const offsets: Array<number | undefined> = [];
    let i = 0;
    const getUpdates = async (opts?: GetUpdatesOptions): Promise<TelegramUpdate[]> => {
      offsets.push(opts?.offset);
      if (i < batches.length) return batches[i++]!;
      await new Promise((r) => setTimeout(r, 5)); // idle: don't busy-spin
      return [];
    };

    const seen: number[] = [];
    const poller = startPoller({
      getUpdates,
      onUpdate: (update) => seen.push(update.update_id!),
      timeoutSeconds: 0,
      backoffMs: 5,
    });

    await waitFor(() => seen.length >= 3);
    await poller.stop();

    assert.deepEqual(seen, [1, 2, 3], "ingested in order");
    assert.equal(offsets[0], undefined, "first poll has no offset");
    assert.equal(offsets[1], 3, "offset = last_update_id + 1 after [1,2]");
    assert.equal(offsets[2], 4, "offset advances again after [3]");
  });

  it("backs off and recovers after a failed poll", async () => {
    let calls = 0;
    const getUpdates = async (): Promise<TelegramUpdate[]> => {
      calls += 1;
      if (calls === 1) throw new Error("network blip");
      if (calls === 2) return [u(7)];
      await new Promise((r) => setTimeout(r, 5));
      return [];
    };

    const seen: number[] = [];
    const poller = startPoller({
      getUpdates,
      onUpdate: (update) => seen.push(update.update_id!),
      timeoutSeconds: 0,
      backoffMs: 5,
    });

    await waitFor(() => seen.includes(7));
    await poller.stop();
    assert.deepEqual(seen, [7], "the update after the failure was still ingested");
  });
});
