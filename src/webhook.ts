// Webhook transport (SPEC §6 webhook column, §7). A node:http server that Telegram POSTs
// updates to. Verifies both the unguessable path AND the X-Telegram-Bot-Api-Secret-Token
// header before processing. Responds 200 immediately and runs Codex asynchronously, since a
// Codex turn can take far longer than Telegram's webhook timeout.

import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { handleUpdate, type HandlerDeps, type TelegramUpdate } from "./handler.js";
import { logger } from "./logger.js";

export interface WebhookServerOptions {
  port: number;
  path: string;
  secret: string;
}

export interface RunningServer {
  port: number;
  close(): Promise<void>;
}

const MAX_BODY_BYTES = 1_000_000; // Telegram updates are small; cap to avoid abuse.

export async function startWebhookServer(
  deps: HandlerDeps,
  opts: WebhookServerOptions,
): Promise<RunningServer> {
  const server = createServer((req, res) => {
    handleRequest(req, res, deps, opts).catch((err) => {
      logger.error("Webhook request error", { error: (err as Error).message });
      if (!res.headersSent) sendStatus(res, 500, "internal error");
    });
  });

  const port = await listen(server, opts.port);
  logger.info("Webhook server listening", { port, path: opts.path });
  return {
    port,
    close: () =>
      new Promise<void>((resolve, reject) =>
        server.close((err) => (err ? reject(err) : resolve())),
      ),
  };
}

async function handleRequest(
  req: IncomingMessage,
  res: ServerResponse,
  deps: HandlerDeps,
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

  // Verify Telegram's secret token header (SPEC §6 security row).
  const headerSecret = req.headers["x-telegram-bot-api-secret-token"];
  if (headerSecret !== opts.secret) {
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

  // Acknowledge to Telegram immediately; process out of band.
  sendStatus(res, 200, "{}");
  void handleUpdate(update, deps).catch((err) =>
    logger.error("handleUpdate failed", { error: (err as Error).message }),
  );
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
