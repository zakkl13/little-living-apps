// Derived full-text search index over the memory files (DESIGN §5: "git = truth, sqlite = query").
//
// node:sqlite (Node 22 built-in) with an FTS5 virtual table. This is a *derived* index — it can
// be dropped and rebuilt from the markdown at any time (see MemFs.reindex), so a corrupt or stale
// db is never a data-loss event. The index is written through on every memory write.

import { DatabaseSync } from "node:sqlite";

export interface SearchHit {
  /** Tool-facing path, e.g. "/memories/archival/facts/foo.md". */
  path: string;
  /** A short highlighted excerpt around the match. */
  snippet: string;
}

export interface FtsIndex {
  upsert(path: string, text: string): void;
  remove(path: string): void;
  rename(oldPath: string, newPath: string): void;
  /** Search all indexed files. `prefix` (optional) restricts to paths under that folder. */
  search(query: string, opts?: { limit?: number; prefix?: string }): SearchHit[];
  clear(): void;
  close(): void;
}

export function openFts(dbPath = ":memory:"): FtsIndex {
  const db = new DatabaseSync(dbPath);
  let closed = false;
  db.exec(`
    CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts
    USING fts5(path UNINDEXED, body, tokenize = 'porter unicode61');
  `);

  const delStmt = db.prepare("DELETE FROM memory_fts WHERE path = ?");
  const insStmt = db.prepare("INSERT INTO memory_fts(path, body) VALUES(?, ?)");

  return {
    upsert(path, text) {
      delStmt.run(path);
      insStmt.run(path, text);
    },
    remove(path) {
      delStmt.run(path);
    },
    rename(oldPath, newPath) {
      db.prepare("UPDATE memory_fts SET path = ? WHERE path = ?").run(newPath, oldPath);
    },
    search(query, opts = {}) {
      const match = toMatchQuery(query);
      if (!match) return [];
      const limit = opts.limit ?? 10;
      // Over-fetch when filtering by prefix so the LIMIT applies after the filter.
      const fetch = opts.prefix ? limit * 5 : limit;
      const rows = db
        .prepare(
          `SELECT path, snippet(memory_fts, 1, '[', ']', ' … ', 12) AS snippet
           FROM memory_fts WHERE memory_fts MATCH ? ORDER BY rank LIMIT ?`,
        )
        .all(match, fetch) as Array<{ path: string; snippet: string }>;
      const filtered = opts.prefix ? rows.filter((r) => r.path.startsWith(opts.prefix!)) : rows;
      return filtered.slice(0, limit).map((r) => ({ path: r.path, snippet: r.snippet }));
    },
    clear() {
      db.exec("DELETE FROM memory_fts");
    },
    close() {
      if (closed) return;
      closed = true;
      db.close();
    },
  };
}

/**
 * Turn a free-text query into a safe FTS5 MATCH expression: extract word tokens, quote each (so
 * punctuation/operators can't break the parser or inject syntax), and OR them for recall.
 */
function toMatchQuery(query: string): string | undefined {
  const tokens = query.match(/[\p{L}\p{N}_]+/gu);
  if (!tokens || tokens.length === 0) return undefined;
  return tokens.map((t) => `"${t}"`).join(" OR ");
}
