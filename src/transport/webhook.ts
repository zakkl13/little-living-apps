// Webhook transport (DESIGN §8). A node:http server Telegram POSTs updates to. Verifies both the
// unguessable path AND the X-Telegram-Bot-Api-Secret-Token header before processing. Responds 200
// immediately and hands the update to `onUpdate` — which in v0.2 ENQUEUES an owner_message event
// and returns (no Codex on the webhook path anymore). The manager loop drains it out of band.

import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { logger } from "../logger.js";

export interface TelegramMessage {
  message_id: number;
  text?: string;
  chat: { id: number };
  from?: { id: number; username?: string; first_name?: string };
}

export interface TelegramUpdate {
  update_id?: number;
  message?: TelegramMessage;
  edited_message?: TelegramMessage;
}

export interface WebhookServerOptions {
  port: number;
  path: string;
  secret: string;
  /** Called for each verified update. Must be fast and non-throwing (it enqueues and returns). */
  onUpdate: (update: TelegramUpdate) => void;
}

export interface RunningServer {
  port: number;
  close(): Promise<void>;
}

const MAX_BODY_BYTES = 1_000_000; // Telegram updates are small; cap to avoid abuse.

export async function startWebhookServer(opts: WebhookServerOptions): Promise<RunningServer> {
  const server = createServer((req, res) => {
    handleRequest(req, res, opts).catch((err) => {
      logger.error("Webhook request error", { error: (err as Error).message });
      if (!res.headersSent) sendStatus(res, 500, "internal error");
    });
  });

  const port = await listen(server, opts.port);
  logger.info("Webhook server listening", { port, path: opts.path });
  return {
    port,
    close: () =>
      new Promise<void>((resolve, reject) => server.close((err) => (err ? reject(err) : resolve()))),
  };
}

async function handleRequest(
  req: IncomingMessage,
  res: ServerResponse,
  opts: WebhookServerOptions,
): Promise<void> {
  const url = new URL(req.url ?? "/", "http://localhost");

  // Health endpoint — also doubles as a wake ping for the Sprite.
  if (req.method === "GET" && (url.pathname === "/healthz" || url.pathname === "/")) {
    sendStatus(res, 200, "ok");
    return;
  }

  if (req.method !== "POST" || url.pathname !== opts.path) {
    sendStatus(res, 404, "not found");
    return;
  }

  // Verify Telegram's secret token header (DESIGN §8 security).
  if (req.headers["x-telegram-bot-api-secret-token"] !== opts.secret) {
    logger.warn("Rejected webhook with bad secret token");
    sendStatus(res, 401, "unauthorized");
    return;
  }

  let update: TelegramUpdate;
  try {
    update = JSON.parse(await readBody(req)) as TelegramUpdate;
  } catch {
    sendStatus(res, 400, "bad request");
    return;
  }

  // Acknowledge immediately; enqueue out of band (the loop drains it).
  sendStatus(res, 200, "{}");
  try {
    opts.onUpdate(update);
  } catch (err) {
    logger.error("onUpdate failed", { error: (err as Error).message });
  }
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let body = "";
    let size = 0;
    req.on("data", (chunk: Buffer) => {
      size += chunk.length;
      if (size > MAX_BODY_BYTES) {
        reject(new Error("body too large"));
        req.destroy();
        return;
      }
      body += chunk.toString();
    });
    req.on("end", () => resolve(body));
    req.on("error", reject);
  });
}

function sendStatus(res: ServerResponse, code: number, body: string): void {
  res.writeHead(code, { "content-type": "application/json" });
  res.end(body);
}

function listen(server: Server, port: number): Promise<number> {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, () => {
      const addr = server.address();
      resolve(typeof addr === "object" && addr ? addr.port : port);
    });
  });
}
