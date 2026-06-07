// Disk-backed chat_id -> codex session_id store (SPEC §8).
//
// RAM does not survive Sprite hibernation, so this MUST live on the persistent volume.
// Backed by a single JSON file with atomic writes (write tmp, fsync, rename) so a crash
// mid-write can never corrupt the store.

import { closeSync, existsSync, mkdirSync, openSync, readFileSync, renameSync, writeSync, fsyncSync } from "node:fs";
import { dirname } from "node:path";
import { logger } from "./logger.js";

interface SessionEntry {
  sessionId: string;
  updatedAt: string;
}

interface StoreFile {
  version: 1;
  sessions: Record<string, SessionEntry>;
}

export interface SessionStore {
  get(chatId: number): string | undefined;
  set(chatId: number, sessionId: string): void;
  delete(chatId: number): void;
}

function emptyStore(): StoreFile {
  return { version: 1, sessions: {} };
}

export function openSessionStore(path: string): SessionStore {
  let data = load(path);

  function persist(): void {
    const dir = dirname(path);
    mkdirSync(dir, { recursive: true });
    const tmp = `${path}.tmp`;
    const fd = openSync(tmp, "w");
    try {
      writeSync(fd, JSON.stringify(data, null, 2));
      fsyncSync(fd);
    } finally {
      closeSync(fd);
    }
    renameSync(tmp, path);
  }

  return {
    get(chatId: number): string | undefined {
      return data.sessions[String(chatId)]?.sessionId;
    },
    set(chatId: number, sessionId: string): void {
      data.sessions[String(chatId)] = { sessionId, updatedAt: new Date().toISOString() };
      persist();
    },
    delete(chatId: number): void {
      if (data.sessions[String(chatId)]) {
        delete data.sessions[String(chatId)];
        persist();
      }
    },
  };
}

function load(path: string): StoreFile {
  if (!existsSync(path)) return emptyStore();
  try {
    const parsed = JSON.parse(readFileSync(path, "utf8")) as Partial<StoreFile>;
    if (parsed && parsed.version === 1 && parsed.sessions && typeof parsed.sessions === "object") {
      return { version: 1, sessions: parsed.sessions };
    }
    logger.warn("Session store had unexpected shape; starting fresh", { path });
    return emptyStore();
  } catch (err) {
    // Corrupt file (e.g. truncated on a hard kill) — don't crash the bot, just start fresh.
    logger.warn("Failed to parse session store; starting fresh", {
      path,
      error: (err as Error).message,
    });
    return emptyStore();
  }
}
