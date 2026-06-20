//! Derived full-text search index over the memory files (git = truth, sqlite = query). Port of
//! `src/memory/fts.ts` onto `rusqlite` with an FTS5 virtual table. This is a *derived* index — it
//! can be dropped and rebuilt from the markdown at any time, so a corrupt/stale db is never a
//! data-loss event. It is written through on every memory write.

use rusqlite::Connection;

use super::MemoryError;

/// A single search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// Tool-facing path, e.g. `/memories/archival/facts/foo.md`.
    pub path: String,
    /// A short highlighted excerpt around the match.
    pub snippet: String,
}

/// Full-text index backed by SQLite FTS5.
pub struct FtsIndex {
    conn: Connection,
}

impl FtsIndex {
    /// Open an index. `path` of `:memory:` is an in-process index; a file path persists it.
    pub fn open(path: &str) -> Result<Self, MemoryError> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()
        } else {
            Connection::open(path)
        }
        .map_err(|e| MemoryError(format!("open fts {path}: {e}")))?;
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts \
             USING fts5(path UNINDEXED, body, tokenize = 'porter unicode61');",
        )
        .map_err(|e| MemoryError(format!("create fts table: {e}")))?;
        Ok(Self { conn })
    }

    /// Insert or replace the indexed text for `path`.
    pub fn upsert(&self, path: &str, text: &str) -> Result<(), MemoryError> {
        self.remove(path)?;
        self.conn
            .execute(
                "INSERT INTO memory_fts(path, body) VALUES(?1, ?2)",
                (path, text),
            )
            .map_err(|e| MemoryError(format!("fts upsert: {e}")))?;
        Ok(())
    }

    /// Remove a path from the index.
    pub fn remove(&self, path: &str) -> Result<(), MemoryError> {
        self.conn
            .execute("DELETE FROM memory_fts WHERE path = ?1", (path,))
            .map_err(|e| MemoryError(format!("fts remove: {e}")))?;
        Ok(())
    }

    /// Rename a path in the index.
    pub fn rename(&self, old_path: &str, new_path: &str) -> Result<(), MemoryError> {
        self.conn
            .execute(
                "UPDATE memory_fts SET path = ?1 WHERE path = ?2",
                (new_path, old_path),
            )
            .map_err(|e| MemoryError(format!("fts rename: {e}")))?;
        Ok(())
    }

    /// Drop all rows (used by reindex).
    pub fn clear(&self) -> Result<(), MemoryError> {
        self.conn
            .execute("DELETE FROM memory_fts", ())
            .map_err(|e| MemoryError(format!("fts clear: {e}")))?;
        Ok(())
    }

    /// Search the index. `prefix` (optional) restricts to paths under that folder.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        prefix: Option<&str>,
    ) -> Result<Vec<SearchHit>, MemoryError> {
        let Some(match_query) = to_match_query(query) else {
            return Ok(Vec::new());
        };
        // Over-fetch when filtering by prefix so the LIMIT applies after the filter.
        let fetch = if prefix.is_some() { limit * 5 } else { limit } as i64;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT path, snippet(memory_fts, 1, '[', ']', ' … ', 12) AS snippet \
                 FROM memory_fts WHERE memory_fts MATCH ?1 ORDER BY rank LIMIT ?2",
            )
            .map_err(|e| MemoryError(format!("fts prepare: {e}")))?;
        let rows = stmt
            .query_map((&match_query, fetch), |row| {
                Ok(SearchHit {
                    path: row.get(0)?,
                    snippet: row.get(1)?,
                })
            })
            .map_err(|e| MemoryError(format!("fts query: {e}")))?;
        collect_hits(rows, limit, prefix)
    }
}

/// Filter the mapped rows by `prefix` and cap at `limit` (pulled out to keep `search` simple).
fn collect_hits(
    rows: impl Iterator<Item = rusqlite::Result<SearchHit>>,
    limit: usize,
    prefix: Option<&str>,
) -> Result<Vec<SearchHit>, MemoryError> {
    let mut hits = Vec::new();
    for row in rows {
        let hit = row.map_err(|e| MemoryError(format!("fts row: {e}")))?;
        if prefix.is_none_or(|p| hit.path.starts_with(p)) {
            hits.push(hit);
            if hits.len() >= limit {
                break;
            }
        }
    }
    Ok(hits)
}

/// Turn a free-text query into a safe FTS5 MATCH expression: extract word tokens, quote each (so
/// punctuation/operators can't break the parser or inject syntax), and OR them for recall.
fn to_match_query(query: &str) -> Option<String> {
    let tokens: Vec<String> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" OR "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_search() {
        let fts = FtsIndex::open(":memory:").unwrap();
        fts.upsert("/memories/a.md", "the quick brown fox").unwrap();
        fts.upsert("/memories/b.md", "lazy dog sleeps").unwrap();
        let hits = fts.search("fox", 10, None).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "/memories/a.md");
    }

    #[test]
    fn prefix_filter() {
        let fts = FtsIndex::open(":memory:").unwrap();
        fts.upsert("/memories/recall/x.md", "shared term").unwrap();
        fts.upsert("/memories/archival/y.md", "shared term")
            .unwrap();
        let hits = fts.search("shared", 10, Some("/memories/recall/")).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.starts_with("/memories/recall/"));
    }

    #[test]
    fn punctuation_query_is_safe() {
        let fts = FtsIndex::open(":memory:").unwrap();
        fts.upsert("/memories/a.md", "alpha beta").unwrap();
        // A query full of FTS operators must not panic or error.
        let hits = fts.search("alpha OR (beta* AND \"x", 10, None).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn remove_and_rename() {
        let fts = FtsIndex::open(":memory:").unwrap();
        fts.upsert("/memories/a.md", "findme").unwrap();
        fts.rename("/memories/a.md", "/memories/b.md").unwrap();
        assert_eq!(
            fts.search("findme", 10, None).unwrap()[0].path,
            "/memories/b.md"
        );
        fts.remove("/memories/b.md").unwrap();
        assert!(fts.search("findme", 10, None).unwrap().is_empty());
    }
}
