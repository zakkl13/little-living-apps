// Minimal fetch-based Telegram Bot API client (SPEC §7 step 6).
//
// Deliberately tiny and dependency-free so the base URL can be pointed at a fake server in
// tests. Handles the one hard requirement from the spec: responses > 4096 chars are chunked,
// never truncated.

import { logger } from "../logger.js";

export const TELEGRAM_MAX_MESSAGE_LENGTH = 4096;
export const TELEGRAM_MAX_CAPTION_LENGTH = 1024;

/** An image to upload with sendPhoto. The caller reads the file — the transport stays fs-free. */
export interface OutboundPhoto {
  bytes: Buffer;
  filename: string;
  caption?: string;
}

/** One size of an inbound photo. Telegram orders the array ascending, so the last is the largest. */
export interface TelegramPhotoSize {
  file_id: string;
  file_unique_id?: string;
  width?: number;
  height?: number;
}

export interface TelegramMessage {
  message_id: number;
  text?: string;
  /** Caption that accompanies a photo (used as the turn's text). */
  caption?: string;
  /** Photo sizes when the owner sends an image (view_image intake). */
  photo?: TelegramPhotoSize[];
  chat: { id: number };
  from?: { id: number; username?: string; first_name?: string };
}

export interface TelegramUpdate {
  update_id?: number;
  message?: TelegramMessage;
  edited_message?: TelegramMessage;
}

export interface GetUpdatesOptions {
  /** Return updates with update_id >= offset; pass last_update_id + 1 to confirm prior ones. */
  offset?: number;
  /** Long-poll timeout in SECONDS (Telegram holds the request open this long when idle). */
  timeout?: number;
  /** Aborts the in-flight long-poll so shutdown is immediate. */
  signal?: AbortSignal;
}

export interface TelegramClient {
  /** Sends text (chunked >4096). Returns the message id of the FIRST chunk, if any. */
  sendMessage(chatId: number, text: string): Promise<number | undefined>;
  /** Uploads an image (multipart sendPhoto). Captions are clipped to Telegram's 1024-char cap. */
  sendPhoto(chatId: number, photo: OutboundPhoto): Promise<number | undefined>;
  /** Edits an existing message in place (used for the live "working…" status). */
  editMessageText(chatId: number, messageId: number, text: string): Promise<void>;
  /** Remove any registered webhook so getUpdates is allowed (they are mutually exclusive). */
  deleteWebhook(): Promise<void>;
  /** Long-poll for new updates (outbound only — no inbound port needed). */
  getUpdates(opts?: GetUpdatesOptions): Promise<TelegramUpdate[]>;
  getMe(): Promise<{ id: number; username?: string }>;
  /** Resolve a file_id to a server-side file path (Bot API getFile). */
  getFile(fileId: string): Promise<{ file_path?: string }>;
  /** Download a file (by its getFile path) as raw bytes. */
  downloadFile(filePath: string): Promise<Buffer>;
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

  async function parseReply<T>(method: string, res: Response): Promise<T> {
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

  async function call<T>(method: string, body: unknown, signal?: AbortSignal): Promise<T> {
    const res = await fetch(`${apiBase}/${method}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
      ...(signal ? { signal } : {}),
    });
    return parseReply(method, res);
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

    async sendPhoto(chatId: number, photo: OutboundPhoto): Promise<number | undefined> {
      // Photo uploads use multipart/form-data, not the JSON surface (the bytes ride as a file part).
      const form = new FormData();
      form.append("chat_id", String(chatId));
      if (photo.caption) form.append("caption", photo.caption.slice(0, TELEGRAM_MAX_CAPTION_LENGTH));
      form.append("photo", new Blob([new Uint8Array(photo.bytes)]), photo.filename);
      const res = await fetch(`${apiBase}/sendPhoto`, { method: "POST", body: form });
      const msg = await parseReply<{ message_id: number }>("sendPhoto", res);
      return msg?.message_id;
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

    async deleteWebhook(): Promise<void> {
      // Telegram refuses getUpdates while a webhook is set; clear a stale one on startup.
      // drop_pending_updates stays false so queued messages survive the switch.
      await call("deleteWebhook", { drop_pending_updates: false });
      logger.info("Cleared any registered Telegram webhook (long-poll mode)");
    },

    async getUpdates(opts: GetUpdatesOptions = {}): Promise<TelegramUpdate[]> {
      const timeout = opts.timeout ?? 50;
      return call<TelegramUpdate[]>(
        "getUpdates",
        {
          ...(opts.offset !== undefined ? { offset: opts.offset } : {}),
          timeout,
          allowed_updates: ["message"],
        },
        opts.signal,
      );
    },

    async getMe(): Promise<{ id: number; username?: string }> {
      return call("getMe", {});
    },

    async getFile(fileId: string): Promise<{ file_path?: string }> {
      return call("getFile", { file_id: fileId });
    },

    async downloadFile(filePath: string): Promise<Buffer> {
      // File downloads use the /file/bot<token>/<path> endpoint, not the JSON API surface.
      const res = await fetch(`${opts.baseUrl}/file/bot${opts.token}/${filePath}`);
      if (!res.ok) throw new Error(`Telegram file download failed: ${res.status} ${res.statusText}`);
      return Buffer.from(await res.arrayBuffer());
    },
  };
}
