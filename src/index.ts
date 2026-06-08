// Entrypoint (DESIGN §2): load config, wire the real boundaries into the manager app, restore any
// snapshot, start the loop, and long-poll Telegram for updates. Designed to run under systemd so it
// auto-restarts on crash/reboot; the snapshot + persistent memory make a cold restart lossless.

import { loadConfig, ConfigError } from "./config.js";
import { createTelegramClient } from "./transport/telegram.js";
import { startPoller } from "./transport/poller.js";
import { createCodexRunner } from "./workers/runner.js";
import { createAnthropicModel } from "./manager/anthropic.js";
import { createManagerApp } from "./app.js";
import { logger } from "./logger.js";

async function main(): Promise<void> {
  let config;
  try {
    config = loadConfig();
  } catch (err) {
    if (err instanceof ConfigError) {
      logger.error(`Configuration error: ${err.message}`);
      process.exit(1);
    }
    throw err;
  }

  const telegram = createTelegramClient({
    baseUrl: config.telegramApiBaseUrl,
    token: config.telegramBotToken,
  });
  const runner = createCodexRunner(config);
  const model = createAnthropicModel({
    apiKey: config.anthropicApiKey,
    ...(config.anthropicBaseUrl ? { baseUrl: config.anthropicBaseUrl } : {}),
  });

  const app = createManagerApp({
    config,
    model,
    runner,
    deliver: async (chatId, text) => {
      await telegram.sendMessage(chatId, text);
    },
  });

  // Boot probe: surface Codex auth problems loudly rather than failing silently (DESIGN §10).
  const auth = await runner.loginStatus();
  logger.info(auth.ok ? "Codex auth OK (ChatGPT subscription)" : "Codex auth probe failed", {
    detail: auth.detail.split("\n")[0],
  });

  app.restore(); // cold-restart recovery: transcript + queue + workers
  app.start();

  // Outbound-only transport: clear any stale webhook, then long-poll. No inbound port is opened.
  await telegram.deleteWebhook().catch((err) =>
    logger.warn("deleteWebhook failed (continuing)", { error: (err as Error).message }),
  );
  const poller = startPoller({
    getUpdates: (opts) => telegram.getUpdates(opts),
    onUpdate: (update) => app.ingestTelegramUpdate(update),
  });

  const shutdown = async (sig: string): Promise<void> => {
    logger.info(`Received ${sig}, shutting down`);
    await poller.stop().catch(() => undefined);
    await app.close().catch(() => undefined);
    process.exit(0);
  };
  process.on("SIGINT", () => void shutdown("SIGINT"));
  process.on("SIGTERM", () => void shutdown("SIGTERM"));

  logger.info("little-living-apps (v0.2 manager) ready", {
    sandbox: config.sandboxMode,
    workspace: config.workspaceDir,
    memory: config.memoryDir,
    allowed: config.allowedUserIds.length,
  });
}

void main();
