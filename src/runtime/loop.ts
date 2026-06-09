// The serialized manager loop (DESIGN §3). One consumer drains the queue, running exactly one
// manager turn at a time — the core invariant that keeps memory + transcript coherent without
// locks. Workers run OUTSIDE the turn and re-enter as events.

import type { EventQueue, ManagerEvent } from "./eventQueue.js";
import { logger } from "../logger.js";

export interface ManagerLoop {
  start(): void;
  /** Resolves when the queue is drained and no turn is running. */
  whenIdle(): Promise<void>;
  stop(): Promise<void>;
}

export interface LoopDeps {
  queue: EventQueue;
  /** Default owner chat for worker_event / tick turns (owner_message carries its own chatId). */
  ownerChatId: number;
  runTurn: (event: ManagerEvent, chatId: number, turnId: number) => Promise<void>;
  /** Persistence hook, run after every turn (snapshot transcript + queue) — DESIGN §11. */
  onTurnComplete?: () => void | Promise<void>;
}

export function createLoop(deps: LoopDeps): ManagerLoop {
  let draining = false;
  let stopped = false;
  let turnCounter = 0;
  let idleWaiters: Array<() => void> = [];

  const chatIdFor = (e: ManagerEvent): number =>
    e.kind === "owner_message" ? e.chatId : deps.ownerChatId;

  function resolveIdle(): void {
    const waiters = idleWaiters;
    idleWaiters = [];
    for (const w of waiters) w();
  }

  async function drain(): Promise<void> {
    if (draining || stopped) return;
    draining = true;
    try {
      while (!stopped && !deps.queue.isEmpty()) {
        const event = deps.queue.dequeue()!;
        turnCounter += 1;
        try {
          await deps.runTurn(event, chatIdFor(event), turnCounter);
        } catch (err) {
          logger.error("Manager turn crashed", { event: event.kind, error: (err as Error).message });
        }
        await deps.onTurnComplete?.();
      }
    } finally {
      draining = false;
    }
    // Something may have been enqueued during the final release window.
    if (!stopped && !deps.queue.isEmpty()) {
      void drain();
      return;
    }
    resolveIdle();
  }

  return {
    start() {
      stopped = false;
      deps.queue.onEnqueue(() => void drain());
      if (!deps.queue.isEmpty()) void drain();
    },
    whenIdle() {
      if (!draining && deps.queue.isEmpty()) return Promise.resolve();
      return new Promise<void>((resolve) => idleWaiters.push(resolve));
    },
    async stop() {
      stopped = true;
      // Wait out an in-flight drain so we don't tear down mid-turn.
      while (draining) await new Promise((r) => setTimeout(r, 5));
    },
  };
}
