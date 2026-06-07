// Minimal fetch-based Telegram Bot API client (SPEC §7 step 6).
//
// Deliberately tiny and dependency-free so the base URL can be pointed at a fake server in
// tests. Handles the one hard requirement from the spec: responses > 4096 chars are chunked,
// never truncated.

import { logger } from "../logger.js";

export const TELEGRAM_MAX_MESSAGE_LENGTH = 4096;

export interface TelegramClient {
  /** Sends text (chunked >4096). Returns the message id of the FIRST chunk, if any. */
  sendMessage(chatId: number, text: string): Promise<number | undefined>;
  /** Edits an existing message in place (used for the live "working…" status). */
  editMessageText(chatId: number, messageId: number, text: string): Promise<void>;
  setWebhook(url: string, secretToken: string): Promise<void>;
  getMe(): Promise<{ id: number; username?: string }>;
}

export interface TelegramClientOptions {
  baseUrl: string;
  token: string;
}

/**
 * Split text into Telegram-sized chunks (<= 4096 chars), preferring to break on newline
 * boundaries and never splitting a chunk larger than the limit.
 */
export function chunkText(text: string, max = TELEGRAM_MAX_MESSAGE_LENGTH): string[] {
  if (text.length === 0) return [];
  const chunks: string[] = [];
  let remaining = text;
  while (remaining.length > max) {
    let cut = remaining.lastIndexOf("\n", max);
    // Only break on a newline if it leaves a reasonably full chunk; otherwise hard-cut.
    if (cut < max * 0.5) cut = max;
    chunks.push(remaining.slice(0, cut));
    remaining = remaining.slice(cut).replace(/^\n/, "");
  }
  if (remaining.length > 0) chunks.push(remaining);
  return chunks;
}

export function createTelegramClient(opts: TelegramClientOptions): TelegramClient {
  const apiBase = `${opts.baseUrl}/bot${opts.token}`;

  async function call<T>(method: string, body: unknown): Promise<T> {
    const res = await fetch(`${apiBase}/${method}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    const json = (await res.json().catch(() => ({}))) as {
      ok?: boolean;
      result?: T;
      description?: string;
    };
    if (!res.ok || json.ok === false) {
      throw new Error(
        `Telegram ${method} failed: ${res.status} ${json.description ?? res.statusText}`,
      );
    }
    return json.result as T;
  }

  return {
    async sendMessage(chatId: number, text: string): Promise<number | undefined> {
      const parts = chunkText(text);
      // Edge case: an empty agent reply should still produce a visible message.
      if (parts.length === 0) parts.push("(empty response)");
      let firstId: number | undefined;
      for (const part of parts) {
        const msg = await call<{ message_id: number }>("sendMessage", {
          chat_id: chatId,
          text: part,
        });
        if (firstId === undefined) firstId = msg?.message_id;
      }
      return firstId;
    },

    async editMessageText(chatId: number, messageId: number, text: string): Promise<void> {
      // Telegram caps edits at 4096 chars and rejects no-op edits; the caller keeps the live
      // status short and throttled, but swallow benign errors so a turn never fails on an edit.
      await call("editMessageText", {
        chat_id: chatId,
        message_id: messageId,
        text: chunkText(text)[0] ?? "",
      });
    },

    async setWebhook(url: string, secretToken: string): Promise<void> {
      await call("setWebhook", {
        url,
        secret_token: secretToken,
        allowed_updates: ["message"],
      });
      logger.info("Registered Telegram webhook", { url });
    },

    async getMe(): Promise<{ id: number; username?: string }> {
      return call("getMe", {});
    },
  };
}
