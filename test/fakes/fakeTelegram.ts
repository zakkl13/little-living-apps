// In-process fake of the Telegram Bot API. Records every call the bot makes so tests can
// assert on what the user would have received — no real Telegram involved.

import { createServer, type Server } from "node:http";

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
  setWebhookCalls: Array<{ url: string; secret?: string }>;
  /** Resolves once `predicate` is true or rejects after `timeoutMs`. */
  waitFor(predicate: () => boolean, timeoutMs?: number): Promise<void>;
  reset(): void;
  close(): Promise<void>;
}

export async function startFakeTelegram(token = "test-token"): Promise<FakeTelegram> {
  const sent: SentMessage[] = [];
  const edits: EditedMessage[] = [];
  const setWebhookCalls: Array<{ url: string; secret?: string }> = [];
  let messageSeq = 0;

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
      if (method === "setWebhook") {
        setWebhookCalls.push({ url: String(payload.url), secret: payload.secret_token as string });
        return ok(res, true);
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
    setWebhookCalls,
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
      setWebhookCalls.length = 0;
    },
    close: () => new Promise<void>((resolve) => server.close(() => resolve())),
  };
}

function ok(res: import("node:http").ServerResponse, result: unknown): void {
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify({ ok: true, result }));
}
