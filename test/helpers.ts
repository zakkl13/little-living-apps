// Shared test harness: starts the REAL bot (real config, store, handler, webhook server) wired
// against the fakes. The only things faked are the external boundaries — Telegram (base URL),
// Codex (an in-process CodexRunner), and the Sprite (we just run as a local process).

import { mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { loadConfig, type Config } from "../src/config.js";
import { createTelegramClient } from "../src/telegram.js";
import { openSessionStore, type SessionStore } from "../src/sessions.js";
import type { CodexRunner } from "../src/codex.js";
import type { SpriteHold } from "../src/sprite.js";
import { startWebhookServer, type RunningServer } from "../src/webhook.js";
import type { HandlerDeps, TelegramUpdate } from "../src/handler.js";
import { makeFakeCodex, type FakeCodex } from "./fakes/fakeCodex.js";

export const ALLOWED_USER_ID = 11111111;

export interface CountingHold extends SpriteHold {
  acquired: number;
  released: number;
  get held(): number;
}

/** A SpriteHold that records acquire/release counts (off-Sprite there's no real socket). */
export function makeCountingHold(): CountingHold {
  let acquired = 0;
  let released = 0;
  return {
    async acquire() {
      acquired += 1;
    },
    async release() {
      released += 1;
    },
    get acquired() {
      return acquired;
    },
    get released() {
      return released;
    },
    get held() {
      return acquired - released;
    },
  };
}

export function buildConfig(overrides: Record<string, string> = {}): Config {
  const dir = mkdtempSync(join(tmpdir(), "scb-"));
  const workspace = join(dir, "project");
  mkdirSync(workspace, { recursive: true });
  const env: NodeJS.ProcessEnv = {
    TELEGRAM_BOT_TOKEN: "test-token",
    ALLOWED_USER_IDS: String(ALLOWED_USER_ID),
    TELEGRAM_WEBHOOK_SECRET: "secret-xyz",
    WORKSPACE_DIR: workspace,
    SESSION_STORE_PATH: join(dir, ".sessions.json"),
    PORT: "0",
    TELEGRAM_API_BASE_URL: "http://127.0.0.1:1", // caller overrides with the fake's URL
    ...overrides,
  };
  return loadConfig(env);
}

export interface TestBot {
  config: Config;
  store: SessionStore;
  codex: CodexRunner;
  hold: CountingHold;
  server: RunningServer;
  url: string;
  postUpdate(update: TelegramUpdate, opts?: { secret?: string }): Promise<Response>;
  close(): Promise<void>;
}

export async function startBot(
  config: Config,
  codex: CodexRunner = makeFakeCodex(),
  hold: CountingHold = makeCountingHold(),
): Promise<TestBot> {
  const telegram = createTelegramClient({
    baseUrl: config.telegramApiBaseUrl,
    token: config.telegramBotToken,
  });
  const store = openSessionStore(config.sessionStorePath);

  const deps: HandlerDeps = {
    config,
    sendMessage: (chatId, text) => telegram.sendMessage(chatId, text),
    editMessage: (chatId, messageId, text) => telegram.editMessageText(chatId, messageId, text),
    store,
    codex,
    hold,
  };

  const server = await startWebhookServer(deps, {
    port: config.port,
    path: config.webhookPath,
    secret: config.webhookSecret,
  });
  const url = `http://127.0.0.1:${server.port}${config.webhookPath}`;

  return {
    config,
    store,
    codex,
    hold,
    server,
    url,
    postUpdate: (update, opts) =>
      fetch(url, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-telegram-bot-api-secret-token": opts?.secret ?? config.webhookSecret,
        },
        body: JSON.stringify(update),
      }),
    close: () => server.close(),
  };
}

export { makeFakeCodex, type FakeCodex };

let updateCounter = 1;

export function messageUpdate(
  text: string,
  opts: { fromId?: number; chatId?: number } = {},
): TelegramUpdate {
  const fromId = opts.fromId ?? ALLOWED_USER_ID;
  const chatId = opts.chatId ?? fromId;
  updateCounter += 1;
  return {
    update_id: updateCounter,
    message: {
      message_id: updateCounter,
      text,
      chat: { id: chatId },
      from: { id: fromId, username: "tester" },
    },
  };
}
