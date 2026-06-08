// In-process fake of the Telegram Bot API. Records outbound calls (sendMessage/editMessageText) so
// tests can assert what the user would have received, and serves getUpdates as a real long-poll:
// tests inject inbound updates via pushUpdate() and the bot's poller pulls them out of band. No
// real Telegram involved.

import { createServer, type Server, type ServerResponse } from "node:http";

import type { TelegramUpdate } from "../../src/transport/telegram.js";

export interface SentMessage {
  chatId: number;
  messageId: number;
  text: string;
}

export interface EditedMessage {
  chatId: number;
  messageId: number;
  text: string;
}

export interface FakeTelegram {
  baseUrl: string;
  port: number;
  sent: SentMessage[];
  edits: EditedMessage[];
  /** Inject an inbound update the bot's poller will fetch via getUpdates. */
  pushUpdate(update: TelegramUpdate): void;
  /** Resolves once `predicate` is true or rejects after `timeoutMs`. */
  waitFor(predicate: () => boolean, timeoutMs?: number): Promise<void>;
  reset(): void;
  close(): Promise<void>;
}

interface Waiter {
  respond: () => void;
  timer: NodeJS.Timeout;
}

export async function startFakeTelegram(token = "test-token"): Promise<FakeTelegram> {
  const sent: SentMessage[] = [];
  const edits: EditedMessage[] = [];
  let inbound: TelegramUpdate[] = []; // unconfirmed updates awaiting getUpdates
  let waiters: Waiter[] = []; // long-poll requests parked until a push or timeout
  let messageSeq = 0;

  const visibleFrom = (offset?: number): TelegramUpdate[] =>
    inbound.filter((u) => offset === undefined || (u.update_id ?? 0) >= offset);

  function wakeWaiters(): void {
    const woken = waiters;
    waiters = [];
    for (const w of woken) {
      clearTimeout(w.timer);
      w.respond();
    }
  }

  const server: Server = createServer((req, res) => {
    let body = "";
    req.on("data", (c: Buffer) => (body += c.toString()));
    req.on("end", () => {
      const path = req.url ?? "";
      const method = path.split("/").pop() ?? "";
      let payload: Record<string, unknown> = {};
      try {
        payload = body ? (JSON.parse(body) as Record<string, unknown>) : {};
      } catch {
        /* ignore malformed */
      }

      if (method === "sendMessage") {
        messageSeq += 1;
        sent.push({
          chatId: Number(payload.chat_id),
          messageId: messageSeq,
          text: String(payload.text ?? ""),
        });
        return ok(res, { message_id: messageSeq, text: payload.text });
      }
      if (method === "editMessageText") {
        edits.push({
          chatId: Number(payload.chat_id),
          messageId: Number(payload.message_id),
          text: String(payload.text ?? ""),
        });
        return ok(res, { message_id: Number(payload.message_id), text: payload.text });
      }
      if (method === "deleteWebhook") {
        return ok(res, true);
      }
      if (method === "getUpdates") {
        const offset = payload.offset === undefined ? undefined : Number(payload.offset);
        const timeoutSec = Number(payload.timeout ?? 0);
        // The offset confirms (drops) everything below it — Telegram's ack mechanism.
        if (offset !== undefined) inbound = inbound.filter((u) => (u.update_id ?? 0) >= offset);

        if (visibleFrom(offset).length > 0) return ok(res, visibleFrom(offset));

        // Park the request: respond when an update is pushed, or after the (capped) long-poll.
        const respond = (): void => {
          if (!res.writableEnded) ok(res, visibleFrom(offset));
        };
        const timer = setTimeout(() => {
          waiters = waiters.filter((w) => w.timer !== timer);
          respond();
        }, Math.min(timeoutSec * 1000, 2000));
        timer.unref();
        waiters.push({ respond, timer });
        return;
      }
      if (method === "getMe") {
        return ok(res, { id: 42, is_bot: true, username: "fake_bot" });
      }
      return ok(res, {});
    });
  });

  const port = await new Promise<number>((resolve) => {
    server.listen(0, () => {
      const addr = server.address();
      resolve(typeof addr === "object" && addr ? addr.port : 0);
    });
  });

  void token; // token is part of the path the bot builds; we accept any here.

  return {
    baseUrl: `http://127.0.0.1:${port}`,
    port,
    sent,
    edits,
    pushUpdate(update) {
      inbound.push(update);
      wakeWaiters();
    },
    async waitFor(predicate, timeoutMs = 4000) {
      const start = Date.now();
      while (!predicate()) {
        if (Date.now() - start > timeoutMs) {
          throw new Error(`waitFor timed out after ${timeoutMs}ms (sent=${sent.length})`);
        }
        await new Promise((r) => setTimeout(r, 10));
      }
    },
    reset() {
      sent.length = 0;
      edits.length = 0;
      inbound = [];
    },
    close: () =>
      new Promise<void>((resolve) => {
        wakeWaiters(); // release any parked long-poll so the server can close
        server.close(() => resolve());
      }),
  };
}

function ok(res: ServerResponse, result: unknown): void {
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify({ ok: true, result }));
}
