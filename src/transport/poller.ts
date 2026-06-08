// Telegram long-poll transport. Replaces the inbound webhook server: the box makes OUTBOUND
// getUpdates calls and never listens on a port, so the bot runs behind NAT, on a home box, or on a
// bare VM with no public URL or TLS. One consumer loop pulls updates, hands each to `onUpdate`
// (which authorizes + enqueues), and advances the confirmation offset past it.

import type { GetUpdatesOptions, TelegramUpdate } from "./telegram.js";
import { logger } from "../logger.js";

export interface Poller {
  /** Stop after the in-flight long-poll returns (the abort makes that immediate). */
  stop(): Promise<void>;
}

export interface PollerDeps {
  getUpdates: (opts?: GetUpdatesOptions) => Promise<TelegramUpdate[]>;
  /** Called for each update in order. Must be fast and non-throwing (it enqueues and returns). */
  onUpdate: (update: TelegramUpdate) => void;
  /** Long-poll timeout in seconds (Telegram holds the request open this long when idle). */
  timeoutSeconds?: number;
  /** Backoff after a failed poll, in ms (network errors); default 1000. */
  backoffMs?: number;
}

export function startPoller(deps: PollerDeps): Poller {
  const timeout = deps.timeoutSeconds ?? 50;
  const backoffMs = deps.backoffMs ?? 1000;
  const controller = new AbortController();
  let stopped = false;
  let offset: number | undefined;

  async function loop(): Promise<void> {
    while (!stopped) {
      try {
        const updates = await deps.getUpdates({
          ...(offset !== undefined ? { offset } : {}),
          timeout,
          signal: controller.signal,
        });
        for (const update of updates) {
          if (update.update_id !== undefined) offset = update.update_id + 1;
          try {
            deps.onUpdate(update);
          } catch (err) {
            logger.error("onUpdate failed", { error: (err as Error).message });
          }
        }
      } catch (err) {
        if (stopped) break; // the abort during shutdown surfaces here — expected, not an error.
        logger.error("getUpdates failed; backing off", { error: (err as Error).message });
        await sleep(backoffMs);
      }
    }
  }

  const done = loop();
  logger.info("Telegram long-poll started", { timeout });

  return {
    async stop(): Promise<void> {
      stopped = true;
      controller.abort();
      await done.catch(() => undefined);
    },
  };
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
