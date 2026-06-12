// The owner delivery channel: manager text plus optional image attachments → Telegram. This is the
// host-side half of the ATTACH contract (driver.ts strips `ATTACH: /path` lines out of the reply;
// this validates each path against disk and uploads the survivors as photos). Validation lives HERE,
// not in the model: the manager can only name paths, so a hallucinated/missing/non-image file is
// dropped with a visible note rather than failing the whole turn.

import { readFileSync, statSync } from "node:fs";
import { basename, extname } from "node:path";

import type { DeliverFn } from "../manager/driver.js";
import type { TelegramClient, OutboundPhoto } from "./telegram.js";
import { logger } from "../logger.js";

/** Telegram caps bot photo uploads at 10 MB. */
const MAX_PHOTO_BYTES = 10 * 1024 * 1024;

/** Only ever attach images — the attachment channel is proof-of-work screenshots, not file export. */
const IMAGE_EXTENSIONS = new Set([".png", ".jpg", ".jpeg", ".gif", ".webp"]);

export function createTelegramDeliver(telegram: TelegramClient): DeliverFn {
  return async (chatId, text, attachments = []) => {
    const photos: OutboundPhoto[] = [];
    const notes: string[] = [];
    for (const path of attachments) {
      try {
        if (!IMAGE_EXTENSIONS.has(extname(path).toLowerCase())) throw new Error("not an image file");
        const size = statSync(path).size;
        if (size > MAX_PHOTO_BYTES) throw new Error(`too large for Telegram (${size} bytes)`);
        photos.push({ bytes: readFileSync(path), filename: basename(path) });
      } catch (err) {
        logger.warn("Dropping undeliverable attachment", { path, error: (err as Error).message });
        notes.push(`⚠️ (couldn't attach ${basename(path)})`);
      }
    }
    const body = [text, ...notes].filter(Boolean).join("\n");
    if (body) await telegram.sendMessage(chatId, body);
    for (const photo of photos) await telegram.sendPhoto(chatId, photo);
  };
}
