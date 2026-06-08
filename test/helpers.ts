// v0.2 test harness: boots the REAL manager app (real memory/git/sqlite, real loop, real webhook)
// wired to fakes at the three external boundaries — Anthropic (scripted model), Codex (in-process
// runner), Telegram (fake HTTP server). Nothing is deployed.

import { mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { loadConfig, type Config } from "../src/config.js";
import { createTelegramClient } from "../src/transport/telegram.js";
import { startWebhookServer, type RunningServer, type TelegramUpdate } from "../src/transport/webhook.js";
import { createManagerApp, type ManagerApp } from "../src/app.js";
import { clipSummarizer } from "../src/workers/summarize.js";

import { startFakeTelegram, type FakeTelegram } from "./fakes/fakeTelegram.js";
import { makeFakeCodex, type FakeCodex } from "./fakes/fakeCodex.js";
import { makeFakeAnthropic, type FakeAnthropic, type ScriptStep } from "./fakes/fakeAnthropic.js";

export const ALLOWED_USER_ID = 11111111;

export function buildConfig(overrides: Record<string, string> = {}): Config {
  const dir = mkdtempSync(join(tmpdir(), "scb-"));
  const workspace = join(dir, "project");
  mkdirSync(workspace, { recursive: true });
  const env: NodeJS.ProcessEnv = {
    TELEGRAM_BOT_TOKEN: "test-token",
    ALLOWED_USER_IDS: String(ALLOWED_USER_ID),
    TELEGRAM_WEBHOOK_SECRET: "secret-xyz",
    ANTHROPIC_API_KEY: "sk-ant-test",
    WORKSPACE_DIR: workspace,
    MEMORY_DIR: join(dir, "memory"),
    MANAGER_STATE_DIR: join(dir, "state"),
    PORT: "0",
    TELEGRAM_API_BASE_URL: "http://127.0.0.1:1", // overridden with the fake's URL below
    ...overrides,
  };
  return loadConfig(env);
}

export interface TestBot {
  config: Config;
  app: ManagerApp;
  telegram: FakeTelegram;
  anthropic: FakeAnthropic;
  codex: FakeCodex;
  url: string;
  postUpdate(update: TelegramUpdate, opts?: { secret?: string }): Promise<Response>;
  close(): Promise<void>;
}

export interface StartBotOptions {
  script?: ScriptStep[];
  anthropic?: FakeAnthropic;
  codex?: FakeCodex;
  config?: Config;
  configOverrides?: Record<string, string>;
}

export async function startBot(opts: StartBotOptions = {}): Promise<TestBot> {
  const telegram = await startFakeTelegram();
  const config = opts.config ?? buildConfig({ TELEGRAM_API_BASE_URL: telegram.baseUrl, ...opts.configOverrides });
  const client = createTelegramClient({ baseUrl: config.telegramApiBaseUrl, token: config.telegramBotToken });

  const anthropic = opts.anthropic ?? makeFakeAnthropic(opts.script ?? []);
  const codex = opts.codex ?? makeFakeCodex();

  const app = createManagerApp({
    config,
    model: anthropic,
    runner: codex,
    deliver: async (chatId, text) => {
      await client.sendMessage(chatId, text);
    },
    summarize: clipSummarizer(),
  });
  app.restore();
  app.start();

  const server: RunningServer = await startWebhookServer({
    port: config.port,
    path: config.webhookPath,
    secret: config.webhookSecret,
    onUpdate: (update) => app.ingestTelegramUpdate(update),
  });
  const url = `http://127.0.0.1:${server.port}${config.webhookPath}`;

  return {
    config,
    app,
    telegram,
    anthropic,
    codex,
    url,
    postUpdate: (update, o) =>
      fetch(url, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-telegram-bot-api-secret-token": o?.secret ?? config.webhookSecret,
        },
        body: JSON.stringify(update),
      }),
    close: async () => {
      await server.close();
      await app.close();
      await telegram.close();
    },
  };
}

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
