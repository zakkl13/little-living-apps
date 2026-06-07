// Core per-update logic (SPEC §7). Transport-agnostic: the webhook server calls handleUpdate
// for each inbound Telegram update. The Codex runner is injected so the whole flow can be
// driven against an in-process fake in tests (no subprocess, no real Codex).

import type { Config } from "./config.js";
import type { CodexRunner } from "./workers/runner.js";
import type { SessionStore } from "./sessions.js";
import type { SpriteHold } from "./runtime/hold.js";
import { logger } from "./logger.js";

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

export interface HandlerDeps {
  config: Config;
  /** Sends text (chunked); resolves to the first chunk's message id. */
  sendMessage: (chatId: number, text: string) => Promise<number | undefined>;
  /** Edits a previously sent message in place (live status). */
  editMessage: (chatId: number, messageId: number, text: string) => Promise<void>;
  store: SessionStore;
  codex: CodexRunner;
  /** Keeps the Sprite awake (and the streaming connection alive) for the duration of a turn. */
  hold: SpriteHold;
}

const STATUS_THROTTLE_MS = 1200;
const STATUS_MAX_LINES = 8;

export async function handleUpdate(update: TelegramUpdate, deps: HandlerDeps): Promise<void> {
  const msg = update.message ?? update.edited_message;
  if (!msg || typeof msg.text !== "string") return;

  const chatId = msg.chat.id;
  const fromId = msg.from?.id;
  const text = msg.text.trim();

  // 1. Authorize (SPEC §7.1).
  if (fromId === undefined || !deps.config.allowedUserIds.includes(fromId)) {
    logger.warn("Rejected unauthorized update", { fromId, chatId });
    await safeSend(deps, chatId, "⛔ You are not authorized to use this bot.");
    return;
  }

  try {
    if (text.startsWith("/")) {
      await handleCommand(text, chatId, deps);
      return;
    }
    await handlePrompt(text, chatId, deps);
  } catch (err) {
    logger.error("Handler error", { chatId, error: (err as Error).message });
    await safeSend(deps, chatId, `⚠️ Internal error: ${(err as Error).message}`);
  }
}

async function handleCommand(text: string, chatId: number, deps: HandlerDeps): Promise<void> {
  const command = text.split(/\s+/)[0]?.toLowerCase().replace(/@.*$/, "") ?? "";
  switch (command) {
    case "/start":
    case "/help":
      await deps.sendMessage(
        chatId,
        [
          "🤖 Codex bot ready.",
          "",
          "Send a message and I'll run it through Codex in the workspace repo, streaming",
          "progress as it works.",
          "",
          "Commands:",
          "/new — start a fresh Codex thread",
          "/status — show auth + current thread",
        ].join("\n"),
      );
      return;

    case "/new":
      deps.store.delete(chatId);
      await deps.sendMessage(chatId, "🆕 Started a fresh thread. The next message begins a new Codex conversation.");
      return;

    case "/status": {
      const threadId = deps.store.get(chatId);
      const auth = await deps.codex.loginStatus();
      const lines = [
        `Auth: ${auth.ok ? "✅ subscription (codex login status OK)" : "❌ not authenticated"}`,
        auth.detail ? `  ${auth.detail.split("\n")[0]}` : "",
        `Thread: ${threadId ?? "none (next message starts one)"}`,
        `Sandbox: ${deps.config.sandboxMode}`,
        `Workspace: ${deps.config.workspaceDir}`,
        "Note: subscription usage is rate-limited by your ChatGPT plan, not unlimited.",
      ].filter((l) => l !== "");
      await deps.sendMessage(chatId, lines.join("\n"));
      return;
    }

    default:
      await deps.sendMessage(chatId, `Unknown command: ${command}. Try /help.`);
  }
}

async function handlePrompt(text: string, chatId: number, deps: HandlerDeps): Promise<void> {
  // Post a live status message we edit as Codex streams progress (SPEC §7.3).
  const statusId = await deps.sendMessage(chatId, "⏳ Working…");

  const activity: string[] = [];
  let lastRendered = "";
  let lastEditAt = 0;

  const flush = async (force = false): Promise<void> => {
    if (statusId === undefined) return;
    const now = Date.now();
    if (!force && now - lastEditAt < STATUS_THROTTLE_MS) return;
    const rendered = ["⚙️ Working…", ...activity.slice(-STATUS_MAX_LINES)].join("\n");
    if (rendered === lastRendered) return;
    lastRendered = rendered;
    lastEditAt = now;
    try {
      await deps.editMessage(chatId, statusId, rendered);
    } catch (err) {
      logger.debug("Status edit failed (ignored)", { error: (err as Error).message });
    }
  };

  // Hold the Sprite awake for the whole turn: a paused Sprite would drop the streaming
  // connection to OpenAI mid-run (SPEC §8 / docs.sprites.dev keeping-sprites-running).
  await deps.hold.acquire();
  try {
    const result = await deps.codex.run({
      prompt: text,
      resumeThreadId: deps.store.get(chatId),
      onProgress: (note) => {
        activity.push(note);
        void flush();
      },
    });

    // Persist the (possibly new) thread id so the next message resumes it (SPEC §7.5).
    if (result.threadId) deps.store.set(chatId, result.threadId);

    // Collapse the live status, then send the final answer (chunked by the client).
    if (statusId !== undefined) {
      const done = activity.length ? `✅ Done (${activity.length} steps).` : "✅ Done.";
      try {
        await deps.editMessage(chatId, statusId, done);
      } catch {
        /* ignore */
      }
    }
    await deps.sendMessage(chatId, result.finalResponse);
  } finally {
    await deps.hold.release();
  }
}

async function safeSend(deps: HandlerDeps, chatId: number, text: string): Promise<void> {
  try {
    await deps.sendMessage(chatId, text);
  } catch (err) {
    logger.error("Failed to send message", { chatId, error: (err as Error).message });
  }
}
