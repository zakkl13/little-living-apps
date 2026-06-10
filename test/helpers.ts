// v0.3 test harness: boots the REAL manager app (real memory/git/sqlite, real loop, real poller,
// real Lila MCP tool handlers, real orchestrator over an in-process fake Codex runner) wired to a
// fake manager backend at the model boundary. The fake backend lets each scripted turn act directly
// against memory + the orchestrator (via the real MCP tool handlers) and reply to the owner — so the
// whole runtime is exercised with everything real except the manager's Codex thread. Nothing is
// deployed.

import { mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { loadConfig, type Config } from "../src/config.js";
import { createTelegramClient, type TelegramUpdate } from "../src/transport/telegram.js";
import { startPoller, type Poller } from "../src/transport/poller.js";
import { createManagerApp, type ManagerApp } from "../src/app.js";
import { clipSummarizer } from "../src/workers/summarize.js";

import { startFakeTelegram, type FakeTelegram } from "./fakes/fakeTelegram.js";
import { makeFakeCodex, type FakeCodex } from "./fakes/fakeCodex.js";
import { makeFakeManager, type FakeManager, type ManagerStep } from "./fakes/fakeManager.js";

export const ALLOWED_USER_ID = 11111111;

export function buildConfig(overrides: Record<string, string> = {}): Config {
  const dir = mkdtempSync(join(tmpdir(), "scb-"));
  const workspace = join(dir, "project");
  mkdirSync(workspace, { recursive: true });
  const env: NodeJS.ProcessEnv = {
    TELEGRAM_BOT_TOKEN: "test-token",
    ALLOWED_USER_IDS: String(ALLOWED_USER_ID),
    WORKSPACE_DIR: workspace,
    MEMORY_DIR: join(dir, "memory"),
    MANAGER_STATE_DIR: join(dir, "state"),
    TELEGRAM_API_BASE_URL: "http://127.0.0.1:1", // overridden with the fake's URL below
    ...overrides,
  };
  return loadConfig(env);
}

export interface TestBot {
  config: Config;
  app: ManagerApp;
  telegram: FakeTelegram;
  manager: FakeManager;
  codex: FakeCodex;
  /** Inject an inbound update the poller will pull and ingest (the long-poll seam). */
  sendUpdate(update: TelegramUpdate): void;
  close(): Promise<void>;
}

export interface StartBotOptions {
  script?: ManagerStep[];
  manager?: FakeManager;
  codex?: FakeCodex;
  config?: Config;
  configOverrides?: Record<string, string>;
}

export async function startBot(opts: StartBotOptions = {}): Promise<TestBot> {
  const telegram = await startFakeTelegram();
  const config = opts.config ?? buildConfig({ TELEGRAM_API_BASE_URL: telegram.baseUrl, ...opts.configOverrides });
  const client = createTelegramClient({ baseUrl: config.telegramApiBaseUrl, token: config.telegramBotToken });

  const manager = opts.manager ?? makeFakeManager(opts.script ?? []);
  const codex = opts.codex ?? makeFakeCodex();

  const app = await createManagerApp({
    config,
    runner: codex,
    deliver: async (chatId, text) => {
      await client.sendMessage(chatId, text);
    },
    summarize: clipSummarizer(),
    backendFactory: manager.factory,
  });
  app.restore();
  app.start();

  const poller: Poller = startPoller({
    getUpdates: (opts) => client.getUpdates(opts),
    onUpdate: (update) => void app.ingestTelegramUpdate(update),
    timeoutSeconds: 1, // short so idle polls turn over quickly; pushUpdate wakes them instantly
  });

  return {
    config,
    app,
    telegram,
    manager,
    codex,
    sendUpdate: (update) => telegram.pushUpdate(update),
    close: async () => {
      await poller.stop();
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
