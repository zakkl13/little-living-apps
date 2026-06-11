// Entrypoint (DESIGN §2): load config, wire the real boundaries into the manager app, restore any
// snapshot, start the loop, and long-poll Telegram for updates. Designed to run under systemd so it
// auto-restarts on crash/reboot; the snapshot + persistent memory make a cold restart lossless.

import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { loadConfig, ConfigError } from "./config.js";
import { createTelegramClient } from "./transport/telegram.js";
import { startPoller } from "./transport/poller.js";
import { createCodexRunner } from "./workers/runner.js";
import { createManagerApp } from "./app.js";
import { startInspector, type InspectorServer } from "./inspector/server.js";
import { openAppFiles } from "./inspector/appfiles.js";
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

  // Owner-sent photos (view_image is on): resolve the file id, download it to a temp path, and hand
  // that path to the manager as a local_image input.
  const photoDir = mkdtempSync(join(tmpdir(), "lila-photos-"));
  const downloadPhoto = async (fileId: string): Promise<string | undefined> => {
    const { file_path } = await telegram.getFile(fileId);
    if (!file_path) return undefined;
    const bytes = await telegram.downloadFile(file_path);
    const ext = file_path.includes(".") ? file_path.slice(file_path.lastIndexOf(".")) : ".jpg";
    const dest = join(photoDir, `${Date.now()}-${Math.random().toString(36).slice(2)}${ext}`);
    writeFileSync(dest, bytes);
    return dest;
  };

  const app = await createManagerApp({
    config,
    runner,
    deliver: async (chatId, text) => {
      await telegram.sendMessage(chatId, text);
    },
    downloadPhoto,
  });

  // Boot probe: surface Codex auth problems loudly rather than failing silently (DESIGN §10).
  const auth = await runner.loginStatus();
  logger.info(auth.ok ? "Codex auth OK (ChatGPT subscription)" : "Codex auth probe failed", {
    detail: auth.detail.split("\n")[0],
  });

  app.restore(); // cold-restart recovery: manager thread id + queue + usage meter
  app.start();

  // Inspector: read-only observability plane (off by default). Bound to 127.0.0.1; Caddy fronts it
  // at /_inspect. It only observes — never a model tool, never mutates runtime state.
  let inspector: InspectorServer | undefined;
  if (config.inspectorEnabled) {
    if (!config.inspectorToken) {
      logger.warn("Inspector enabled without INSPECTOR_TOKEN — relying on Caddy basic_auth as the guard");
    }
    inspector = startInspector({
      port: config.inspectorPort,
      ...(config.inspectorToken ? { token: config.inspectorToken } : {}),
      managerModel: config.managerModel ?? "(codex default)",
      workspaceDir: config.workspaceDir,
      appPublicUrl: config.appPublicUrl,
      telemetry: app.telemetry,
      conversation: () => app.telemetry.conversation(),
      memories: () => app.mem.listAll(),
      appFiles: openAppFiles(config.workspaceDir),
    });
  }

  // Outbound-only transport: clear any stale webhook, then long-poll. No inbound port is opened.
  await telegram.deleteWebhook().catch((err) =>
    logger.warn("deleteWebhook failed (continuing)", { error: (err as Error).message }),
  );
  const poller = startPoller({
    getUpdates: (opts) => telegram.getUpdates(opts),
    onUpdate: (update) => void app.ingestTelegramUpdate(update),
  });

  const shutdown = async (sig: string): Promise<void> => {
    logger.info(`Received ${sig}, shutting down`);
    await poller.stop().catch(() => undefined);
    await inspector?.close().catch(() => undefined);
    await app.close().catch(() => undefined);
    process.exit(0);
  };
  process.on("SIGINT", () => void shutdown("SIGINT"));
  process.on("SIGTERM", () => void shutdown("SIGTERM"));

  logger.info("little-living-apps (v0.3 Codex manager) ready", {
    sandbox: config.sandboxMode,
    workspace: config.workspaceDir,
    memory: config.memoryDir,
    managerModel: config.managerModel ?? "(codex default)",
    allowed: config.allowedUserIds.length,
  });
}

void main();
