// Entrypoint: load config, wire dependencies, start the webhook server, optionally register
// the Telegram webhook, and handle graceful shutdown. Designed to run as a Sprite Service so
// it auto-restarts on wake (SPEC §6 / §11 rule 2).

import { loadConfig, ConfigError } from "./config.js";
import { createTelegramClient } from "./telegram.js";
import { openSessionStore } from "./sessions.js";
import { createCodexRunner } from "./codex.js";
import { createSpriteHold } from "./sprite.js";
import { startWebhookServer } from "./webhook.js";
import type { HandlerDeps } from "./handler.js";
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
  const store = openSessionStore(config.sessionStorePath);
  const codex = createCodexRunner(config);
  const hold = createSpriteHold();

  const deps: HandlerDeps = {
    config,
    sendMessage: (chatId, text) => telegram.sendMessage(chatId, text),
    editMessage: (chatId, messageId, text) => telegram.editMessageText(chatId, messageId, text),
    store,
    codex,
    hold,
  };

  // Boot probe: surface auth problems loudly rather than failing silently (SPEC §4).
  const auth = await codex.loginStatus();
  if (auth.ok) {
    logger.info("Codex auth OK (ChatGPT subscription)");
  } else {
    logger.warn("Codex auth probe failed — bot will start but Codex runs may fail", {
      detail: auth.detail.split("\n")[0],
    });
  }

  const server = await startWebhookServer(deps, {
    port: config.port,
    path: config.webhookPath,
    secret: config.webhookSecret,
  });

  if (config.publicUrl) {
    const url = `${config.publicUrl}${config.webhookPath}`;
    try {
      await telegram.setWebhook(url, config.webhookSecret);
    } catch (err) {
      logger.error("Failed to register webhook (continuing; register manually)", {
        error: (err as Error).message,
      });
    }
  } else {
    logger.info("PUBLIC_URL not set; skipping setWebhook. Register the webhook manually.", {
      path: config.webhookPath,
    });
  }

  const shutdown = async (sig: string): Promise<void> => {
    logger.info(`Received ${sig}, shutting down`);
    await server.close().catch(() => undefined);
    process.exit(0);
  };
  process.on("SIGINT", () => void shutdown("SIGINT"));
  process.on("SIGTERM", () => void shutdown("SIGTERM"));

  logger.info("sprite-codex-bot ready", {
    sandbox: config.sandboxMode,
    workspace: config.workspaceDir,
    allowed: config.allowedUserIds.length,
  });
}

void main();
