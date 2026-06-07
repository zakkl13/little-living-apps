// Keeps the Sprite awake while a Codex turn is in flight (docs.sprites.dev/keeping-sprites-running).
//
// A Sprite pauses on idle even while a Service is alive, and "open TCP connections drop on the
// pause, even on warm" — which would kill the long-running streaming connection to OpenAI mid
// turn. The Tasks API is the documented fix: "while at least one task is live, the Sprite runs."
//
// We hold ONE refcounted task ("codex-turn") for as long as >=1 turn is running, refreshing it
// on a heartbeat (5m expiry, refreshed every 60s -> four missed beats of margin). The hold is
// acquired at the start of a turn and released in a finally, so a crash can't leak it for long.
//
// Off-Sprite (local dev, tests) the unix socket doesn't exist; the first failed call disables
// the hold and everything becomes a no-op, so the same code runs everywhere.

import { request } from "node:http";
import { logger } from "../logger.js";

export interface SpriteHold {
  /** Register interest in keeping the Sprite awake (refcounted; starts the heartbeat). */
  acquire(): Promise<void>;
  /** Drop interest; when the last holder releases, the task is deleted. */
  release(): Promise<void>;
}

/** A hold that does nothing — used off-Sprite and in tests. */
export const noopHold: SpriteHold = {
  async acquire() {},
  async release() {},
};

export interface SpriteHoldOptions {
  socketPath?: string;
  taskName?: string;
  /** Task expiry sent on each heartbeat. */
  expire?: string;
  /** Heartbeat interval in ms (must be comfortably shorter than `expire`). */
  heartbeatMs?: number;
}

export function createSpriteHold(opts: SpriteHoldOptions = {}): SpriteHold {
  const socketPath = opts.socketPath ?? "/.sprite/api.sock";
  const taskName = opts.taskName ?? "codex-turn";
  const expire = opts.expire ?? "5m";
  const heartbeatMs = opts.heartbeatMs ?? 60_000;

  let refcount = 0;
  let timer: NodeJS.Timeout | undefined;
  let disabled = false; // set once the Tasks API proves unavailable (e.g. off-Sprite)

  function call(method: "PUT" | "DELETE", path: string, body?: unknown): Promise<void> {
    return new Promise((resolve, reject) => {
      const req = request(
        { socketPath, method, path, headers: { "content-type": "application/json" } },
        (res) => {
          res.resume(); // drain
          res.on("end", () => {
            const code = res.statusCode ?? 0;
            if (code >= 400) reject(new Error(`Tasks API ${method} ${path} -> ${code}`));
            else resolve();
          });
        },
      );
      req.on("error", reject);
      if (body !== undefined) req.write(JSON.stringify(body));
      req.end();
    });
  }

  const heartbeat = (): Promise<void> => call("PUT", `/v1/tasks/${taskName}`, { expire });

  return {
    async acquire(): Promise<void> {
      if (disabled) return;
      refcount += 1;
      if (refcount !== 1) return; // already holding

      try {
        await heartbeat(); // PUT upserts: this creates the task
      } catch (err) {
        // No socket / not on a Sprite: degrade to a no-op for the rest of the process.
        disabled = true;
        refcount = 0;
        logger.warn("Sprite Tasks API unavailable; running without a keep-alive hold", {
          error: (err as Error).message,
        });
        return;
      }

      timer = setInterval(() => {
        heartbeat().catch((err) =>
          logger.debug("Keep-alive heartbeat failed (will retry)", {
            error: (err as Error).message,
          }),
        );
      }, heartbeatMs);
      timer.unref(); // don't keep the event loop alive just for the heartbeat
      logger.debug("Acquired Sprite keep-alive hold", { taskName, expire });
    },

    async release(): Promise<void> {
      if (disabled) return;
      if (refcount > 0) refcount -= 1;
      if (refcount !== 0) return; // others still holding

      if (timer) {
        clearInterval(timer);
        timer = undefined;
      }
      await call("DELETE", `/v1/tasks/${taskName}`).catch((err) =>
        logger.debug("Failed to release keep-alive hold (it will expire on its own)", {
          error: (err as Error).message,
        }),
      );
      logger.debug("Released Sprite keep-alive hold", { taskName });
    },
  };
}
