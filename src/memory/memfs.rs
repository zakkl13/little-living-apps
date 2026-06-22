//! MemFs — the storage backend behind the Lila MCP memory tools.
//! Implements the fixed command set (view/create/str_replace/insert/delete/rename) over a
//! `/memories` directory. Source of truth is markdown on disk in a git repo; the FTS index is
//! written through on every change. `system/` is auto-injected into the prompt in full.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::block::{MemoryBlock, indexable_text, parse_block, serialize_block};
use super::fts::{FtsIndex, SearchHit};
use super::git::{commit_all, ensure_repo};
use super::{MemoryCommand, MemoryError};

/// The tool-facing mount point for the memory store.
pub const MEMORY_MOUNT: &str = "/memories";

/// Options for opening a [`MemFs`].
pub struct MemFsOptions {
    /// The repo directory holding the markdown.
    pub dir: PathBuf,
    /// FTS db path (`:memory:` for an in-process index, or a file path to persist).
    pub fts_path: String,
}

/// The memory store: a git-tracked markdown repo with a derived FTS index.
pub struct MemFs {
    dir: PathBuf,
    fts: FtsIndex,
}

impl MemFs {
    /// Open (initializing repo + scaffold on first boot) and build the initial FTS index.
    pub fn open(opts: MemFsOptions) -> Result<Self, MemoryError> {
        ensure_repo(&opts.dir)?;
        ensure_gitignore(&opts.dir)?;
        seed_scaffold(&opts.dir)?;
        let fts = FtsIndex::open(&opts.fts_path)?;
        let me = Self { dir: opts.dir, fts };
        me.reindex()?;
        Ok(me)
    }

    // ---- path handling ----------------------------------------------------

    /// Map a tool path (`/memories/...` or a bare repo-relative path) to a safe repo-relative path,
    /// rejecting any traversal that escapes the mount.
    fn to_rel(&self, tool_path: &str) -> Result<String, MemoryError> {
        let mut p = tool_path.trim().replace('\\', "/");
        if p == MEMORY_MOUNT || p == format!("{MEMORY_MOUNT}/") {
            return Ok(String::new());
        }
        if let Some(rest) = p.strip_prefix(&format!("{MEMORY_MOUNT}/")) {
            p = rest.to_string();
        }
        normalize_rel(p.trim_start_matches('/'))
            .ok_or_else(|| MemoryError(format!("path escapes {MEMORY_MOUNT}: {tool_path}")))
    }

    fn abs(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    fn tool_path_of(rel: &str) -> String {
        if rel.is_empty() {
            MEMORY_MOUNT.to_string()
        } else {
            format!("{MEMORY_MOUNT}/{rel}")
        }
    }

    fn index_file(&self, rel: &str) -> Result<(), MemoryError> {
        let raw = read_to_string(&self.abs(rel))?;
        self.fts.upsert(
            &Self::tool_path_of(rel),
            &indexable_text(&parse_block(&raw)),
        )
    }

    fn commit(&self, message: &str) -> Result<(), MemoryError> {
        commit_all(&self.dir, message).map(|_| ())
    }

    // ---- command handlers -------------------------------------------------

    /// Dispatch a raw command to its handler.
    pub fn execute(&self, cmd: &MemoryCommand) -> Result<String, MemoryError> {
        match cmd {
            MemoryCommand::View { path, view_range } => self.view(path, *view_range),
            MemoryCommand::Create { path, file_text } => self.create(path, file_text),
            MemoryCommand::StrReplace {
                path,
                old_str,
                new_str,
            } => self.str_replace(path, old_str, new_str),
            MemoryCommand::Insert {
                path,
                insert_line,
                insert_text,
            } => self.insert(path, *insert_line, insert_text),
            MemoryCommand::Delete { path } => self.delete(path),
            MemoryCommand::Rename { old_path, new_path } => self.rename(old_path, new_path),
        }
    }

    /// Read a file (optionally a line range), or list a directory.
    pub fn view(&self, path: &str, view_range: Option<[i64; 2]>) -> Result<String, MemoryError> {
        let rel = self.to_rel(path)?;
        let target = self.abs(&rel);
        if !target.exists() {
            return Err(MemoryError(format!("no such path: {path}")));
        }
        if target.is_dir() {
            return Ok(list_dir(&target));
        }
        let content = read_to_string(&target)?;
        Ok(match view_range {
            Some([start, end]) => slice_lines(&content, start, end),
            None => content,
        })
    }

    /// Write `content` to `rel`, refresh its FTS entry, and commit — the shared tail of every write.
    fn write_indexed(&self, rel: &str, content: &str, message: &str) -> Result<(), MemoryError> {
        write_file(&self.abs(rel), content)?;
        self.index_file(rel)?;
        self.commit(message)
    }

    /// Resolve `path` to (rel, abs) and require that an existing file lives there.
    fn require_file(&self, path: &str) -> Result<(String, PathBuf), MemoryError> {
        let rel = self.to_rel(path)?;
        let target = self.abs(&rel);
        if target.exists() {
            Ok((rel, target))
        } else {
            Err(MemoryError(format!("no such file: {path}")))
        }
    }

    /// Create or overwrite a file with the given text.
    pub fn create(&self, path: &str, file_text: &str) -> Result<String, MemoryError> {
        let rel = self.to_rel(path)?;
        if rel.is_empty() {
            return Err(MemoryError("cannot create the root".into()));
        }
        if let Some(parent) = self.abs(&rel).parent() {
            std::fs::create_dir_all(parent).map_err(|e| MemoryError(format!("mkdir: {e}")))?;
        }
        self.write_indexed(
            &rel,
            file_text,
            &format!("create {rel} — {}", summary(file_text)),
        )?;
        Ok(format!(
            "Created {} ({} chars).",
            Self::tool_path_of(&rel),
            file_text.len()
        ))
    }

    /// Replace a unique substring in a file.
    pub fn str_replace(&self, path: &str, old: &str, new: &str) -> Result<String, MemoryError> {
        let (rel, target) = self.require_file(path)?;
        let before = read_to_string(&target)?;
        let after =
            replace_unique(&before, old, new).map_err(|e| MemoryError(format!("{e} in {path}")))?;
        self.write_indexed(
            &rel,
            &after,
            &format!("str_replace {rel} — {}", summary(new)),
        )?;
        Ok(format!("Edited {}.", Self::tool_path_of(&rel)))
    }

    /// Insert text after the given (0-based) line number.
    pub fn insert(&self, path: &str, line: usize, text: &str) -> Result<String, MemoryError> {
        let (rel, target) = self.require_file(path)?;
        let content = read_to_string(&target)?;
        let mut lines: Vec<&str> = content.split('\n').collect();
        lines.insert(line.min(lines.len()), text);
        self.write_indexed(
            &rel,
            &lines.join("\n"),
            &format!("insert {rel}:{line} — {}", summary(text)),
        )?;
        Ok(format!(
            "Inserted into {} at line {line}.",
            Self::tool_path_of(&rel)
        ))
    }

    /// Delete a file or directory subtree.
    pub fn delete(&self, path: &str) -> Result<String, MemoryError> {
        let rel = self.to_rel(path)?;
        if rel.is_empty() {
            return Err(MemoryError("cannot delete the root".into()));
        }
        let target = self.abs(&rel);
        if !target.exists() {
            return Err(MemoryError(format!("no such path: {path}")));
        }
        self.remove_and_forget(&rel, &target)?;
        Ok(format!("Deleted {}.", Self::tool_path_of(&rel)))
    }

    /// Delete `target` from disk + FTS index and commit. De-index paths are captured BEFORE deletion
    /// (walking a dir needs it to still exist).
    fn remove_and_forget(&self, rel: &str, target: &Path) -> Result<(), MemoryError> {
        let removed = index_paths_under(rel, target);
        remove_path(target)?;
        for r in &removed {
            self.fts.remove(&Self::tool_path_of(r))?;
        }
        self.commit(&format!("delete {rel}"))
    }

    /// Move `from` → `to_rel` on disk, re-point its FTS entry, and commit (the tail of `rename`).
    fn move_indexed(&self, from: &Path, from_rel: &str, to_rel: &str) -> Result<(), MemoryError> {
        let to = self.abs(to_rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| MemoryError(format!("mkdir: {e}")))?;
        }
        std::fs::rename(from, &to).map_err(|e| MemoryError(format!("rename: {e}")))?;
        self.fts
            .rename(&Self::tool_path_of(from_rel), &Self::tool_path_of(to_rel))?;
        self.commit(&format!("rename {from_rel} -> {to_rel}"))
    }

    /// Rename/move a file or directory.
    pub fn rename(&self, old_path: &str, new_path: &str) -> Result<String, MemoryError> {
        let from_rel = self.to_rel(old_path)?;
        let to_rel = self.to_rel(new_path)?;
        let from = self.abs(&from_rel);
        if !from.exists() {
            return Err(MemoryError(format!("no such path: {old_path}")));
        }
        self.move_indexed(&from, &from_rel, &to_rel)?;
        Ok(format!(
            "Renamed {} → {}.",
            Self::tool_path_of(&from_rel),
            Self::tool_path_of(&to_rel)
        ))
    }

    // ---- queries ----------------------------------------------------------

    /// Concatenated `system/` bodies, injected into the system prompt in full every turn.
    pub fn load_system(&self) -> String {
        let sys_dir = self.dir.join("system");
        if !sys_dir.exists() {
            return String::new();
        }
        let mut rels = walk_files(&sys_dir);
        rels.sort();
        let mut sections = Vec::new();
        for rel in rels {
            let Ok(raw) = read_to_string(&sys_dir.join(&rel)) else {
                continue;
            };
            let block = parse_block(&raw);
            let body = block.body.trim();
            if body.is_empty() && block.description.is_none() {
                continue;
            }
            sections.push(format!("### system/{rel}\n{body}"));
        }
        sections.join("\n\n")
    }

    /// Tree listing of non-system files with their frontmatter descriptions.
    pub fn tree_listing(&self) -> String {
        let mut rels = walk_files(&self.dir);
        rels.sort();
        let mut lines = Vec::new();
        for rel in rels {
            if rel.starts_with("system/") {
                continue;
            }
            let Ok(raw) = read_to_string(&self.abs(&rel)) else {
                continue;
            };
            let desc = parse_block(&raw)
                .description
                .map(|d| format!(" — {d}"))
                .unwrap_or_default();
            lines.push(format!("{}{desc}", Self::tool_path_of(&rel)));
        }
        if lines.is_empty() {
            "(no archival/recall files yet)".into()
        } else {
            lines.join("\n")
        }
    }

    /// Every memory file (repo-relative path + raw body), for read-only inspection.
    pub fn list_all(&self) -> Vec<(String, String)> {
        let mut rels = walk_files(&self.dir);
        rels.sort();
        rels.into_iter()
            .filter_map(|rel| read_to_string(&self.abs(&rel)).ok().map(|b| (rel, b)))
            .collect()
    }

    /// FTS over all memory files.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, MemoryError> {
        self.fts.search(query, limit, None)
    }

    /// FTS restricted to `recall/`.
    pub fn recall_search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, MemoryError> {
        self.fts
            .search(query, limit, Some(&format!("{MEMORY_MOUNT}/recall/")))
    }

    /// Write a summarized-conversation file under `recall/<month>/`.
    pub fn write_recall(
        &self,
        name: &str,
        body: &str,
        month_key: Option<&str>,
    ) -> Result<String, MemoryError> {
        let month = month_key
            .map(str::to_string)
            .unwrap_or_else(current_month_folder);
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let file = if safe.ends_with(".md") {
            safe
        } else {
            format!("{safe}.md")
        };
        self.create(&format!("{MEMORY_MOUNT}/recall/{month}/{file}"), body)
    }

    /// Read a file by repo-relative path (e.g. `system/persona.md`); `None` if absent.
    pub fn read_relative(&self, rel: &str) -> Option<String> {
        let target = self.abs(&self.to_rel(rel).ok()?);
        if target.is_file() {
            read_to_string(&target).ok()
        } else {
            None
        }
    }

    /// Rebuild the FTS index from disk (cold-start / corruption recovery).
    pub fn reindex(&self) -> Result<(), MemoryError> {
        self.fts.clear()?;
        for rel in walk_files(&self.dir) {
            self.index_file(&rel)?;
        }
        Ok(())
    }
}

// ---- free helpers ----------------------------------------------------------

/// Fold a slash path's components (handling `.`/`..`), returning `None` if it escapes the root.
fn normalize_rel(p: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            other => out.push(other),
        }
    }
    Some(out.join("/"))
}

/// Replace the unique occurrence of `old` with `new`, or describe why it can't (not found / not
/// unique). Pulled out so `str_replace` stays simple.
fn replace_unique(before: &str, old: &str, new: &str) -> Result<String, String> {
    let Some(idx) = before.find(old) else {
        return Err("old_str not found".to_string());
    };
    if before[idx + old.len()..].contains(old) {
        return Err("old_str is not unique; add more context".to_string());
    }
    Ok(format!(
        "{}{}{}",
        &before[..idx],
        new,
        &before[idx + old.len()..]
    ))
}

/// The tool paths to de-index for a deleted file/dir (every file under a dir; else the file itself).
fn index_paths_under(rel: &str, target: &Path) -> Vec<String> {
    if target.is_dir() {
        walk_files(target)
            .into_iter()
            .map(|r| format!("{rel}/{r}"))
            .collect()
    } else {
        vec![rel.to_string()]
    }
}

fn read_to_string(path: &Path) -> Result<String, MemoryError> {
    std::fs::read_to_string(path).map_err(|e| MemoryError(format!("read {path:?}: {e}")))
}

fn write_file(path: &Path, text: &str) -> Result<(), MemoryError> {
    std::fs::write(path, text).map_err(|e| MemoryError(format!("write {path:?}: {e}")))
}

fn remove_path(path: &Path) -> Result<(), MemoryError> {
    let res = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    res.map_err(|e| MemoryError(format!("remove {path:?}: {e}")))
}

/// First non-empty line of `text`, clipped to 72 chars.
fn summary(text: &str) -> String {
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let clipped: String = first.chars().take(72).collect();
    if clipped.is_empty() {
        "(empty)".into()
    } else {
        clipped
    }
}

/// Slice a 1-based inclusive line range; `end == -1` means to the end.
fn slice_lines(content: &str, start: i64, end: i64) -> String {
    let lines: Vec<&str> = content.split('\n').collect();
    let from = (start.max(1) - 1) as usize;
    let to = if end == -1 {
        lines.len()
    } else {
        (end.max(0) as usize).min(lines.len())
    };
    if from >= lines.len() || from >= to {
        return String::new();
    }
    lines[from..to].join("\n")
}

/// Directory listing (names, dirs suffixed with `/`), excluding `.git`.
fn list_dir(abs_dir: &Path) -> String {
    let Ok(read) = std::fs::read_dir(abs_dir) else {
        return "(empty directory)".into();
    };
    let mut entries: Vec<String> = read
        .flatten()
        .filter(|e| e.file_name() != ".git")
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    entries.sort();
    if entries.is_empty() {
        "(empty directory)".into()
    } else {
        entries.join("\n")
    }
}

/// All files (recursively) under `root`, as POSIX-relative paths, excluding dotfiles/.git.
fn walk_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in read.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let full = entry.path();
            if full.is_dir() {
                stack.push(full);
            } else if let Ok(rel) = full.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out
}

fn ensure_gitignore(dir: &Path) -> Result<(), MemoryError> {
    let gi = dir.join(".gitignore");
    if !gi.exists() {
        write_file(
            &gi,
            "# derived FTS index lives outside the repo by default\n",
        )?;
    }
    Ok(())
}

/// Lay down the empty core-memory scaffold on first boot.
fn seed_scaffold(dir: &Path) -> Result<(), MemoryError> {
    for sub in ["system", "archival", "recall"] {
        std::fs::create_dir_all(dir.join(sub)).map_err(|e| MemoryError(format!("mkdir: {e}")))?;
    }
    let persona = dir.join("system").join("persona.md");
    if !persona.exists() {
        let block = MemoryBlock {
            description: Some("who the manager is and how it works".into()),
            limit: None,
            body: "The manager plans, remembers, and delegates to workers. It has no shell/file/net\n\
                   tools of its own — its only hands are the worker and memory tools. It speaks to the\n\
                   owner simply by writing an ordinary message.\n"
                .into(),
        };
        write_file(&persona, &serialize_block(&block))?;
    }
    Ok(())
}

/// Current UTC `YYYY-MM` folder name for the recall tier.
fn current_month_folder() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, _) = civil_from_days((secs / 86_400) as i64);
    format!("{year:04}-{month:02}")
}

/// Convert days-since-Unix-epoch to a (year, month, day) civil date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::git::commit_count;
    use tempfile::TempDir;

    fn open_mem() -> (TempDir, MemFs) {
        let tmp = TempDir::new().unwrap();
        let mem = MemFs::open(MemFsOptions {
            dir: tmp.path().join("memory"),
            fts_path: ":memory:".into(),
        })
        .unwrap();
        (tmp, mem)
    }

    #[test]
    fn scaffolds_and_seeds_persona() {
        let (_t, mem) = open_mem();
        assert!(mem.read_relative("system/persona.md").is_some());
        assert!(mem.load_system().contains("delegates to workers"));
    }

    #[test]
    fn create_view_edit_delete_roundtrip() {
        let (_t, mem) = open_mem();
        mem.create("/memories/archival/note.md", "hello world")
            .unwrap();
        assert_eq!(
            mem.view("/memories/archival/note.md", None).unwrap(),
            "hello world"
        );
        mem.str_replace("/memories/archival/note.md", "world", "rust")
            .unwrap();
        assert_eq!(
            mem.view("/memories/archival/note.md", None).unwrap(),
            "hello rust"
        );
        mem.delete("/memories/archival/note.md").unwrap();
        assert!(mem.view("/memories/archival/note.md", None).is_err());
    }

    #[test]
    fn each_write_is_a_commit() {
        let (_t, mem) = open_mem();
        let before = commit_count(&mem.dir);
        mem.create("/memories/archival/a.md", "x").unwrap();
        mem.create("/memories/archival/b.md", "y").unwrap();
        assert_eq!(commit_count(&mem.dir), before + 2);
    }

    #[test]
    fn str_replace_requires_unique_match() {
        let (_t, mem) = open_mem();
        mem.create("/memories/archival/d.md", "dup dup").unwrap();
        assert!(
            mem.str_replace("/memories/archival/d.md", "dup", "x")
                .is_err()
        );
    }

    #[test]
    fn path_traversal_is_rejected() {
        let (_t, mem) = open_mem();
        assert!(mem.view("/memories/../../etc/passwd", None).is_err());
        assert!(mem.create("../escape.md", "x").is_err());
    }

    #[test]
    fn search_finds_created_file() {
        let (_t, mem) = open_mem();
        mem.create("/memories/archival/pluto.md", "the dwarf planet pluto")
            .unwrap();
        let hits = mem.search("pluto", 10).unwrap();
        assert!(hits.iter().any(|h| h.path.ends_with("pluto.md")));
    }

    #[test]
    fn view_range_slices_lines() {
        let (_t, mem) = open_mem();
        mem.create("/memories/archival/lines.md", "l1\nl2\nl3\nl4")
            .unwrap();
        assert_eq!(
            mem.view("/memories/archival/lines.md", Some([2, 3]))
                .unwrap(),
            "l2\nl3"
        );
        assert_eq!(
            mem.view("/memories/archival/lines.md", Some([3, -1]))
                .unwrap(),
            "l3\nl4"
        );
    }

    #[test]
    fn rename_moves_and_reindexes() {
        let (_t, mem) = open_mem();
        mem.create("/memories/archival/old.md", "movable content")
            .unwrap();
        mem.rename("/memories/archival/old.md", "/memories/archival/new.md")
            .unwrap();
        assert!(mem.view("/memories/archival/new.md", None).is_ok());
        let hits = mem.search("movable", 10).unwrap();
        assert!(hits.iter().all(|h| h.path.ends_with("new.md")));
    }

    #[test]
    fn reindex_recovers_from_empty_index() {
        let (_t, mem) = open_mem();
        mem.create("/memories/archival/r.md", "reindexable token")
            .unwrap();
        mem.fts.clear().unwrap();
        assert!(mem.search("reindexable", 10).unwrap().is_empty());
        mem.reindex().unwrap();
        assert_eq!(mem.search("reindexable", 10).unwrap().len(), 1);
    }

    #[test]
    fn execute_dispatches_every_command_variant() {
        let (_t, mem) = open_mem();
        let p = "/memories/archival/e.md".to_string();
        mem.execute(&MemoryCommand::Create {
            path: p.clone(),
            file_text: "alpha\nbeta".into(),
        })
        .unwrap();
        assert_eq!(
            mem.execute(&MemoryCommand::View {
                path: p.clone(),
                view_range: None,
            })
            .unwrap(),
            "alpha\nbeta"
        );
        mem.execute(&MemoryCommand::StrReplace {
            path: p.clone(),
            old_str: "alpha".into(),
            new_str: "gamma".into(),
        })
        .unwrap();
        mem.execute(&MemoryCommand::Insert {
            path: p.clone(),
            insert_line: 0,
            insert_text: "top".into(),
        })
        .unwrap();
        assert!(mem.view(&p, None).unwrap().starts_with("top\ngamma"));
        mem.execute(&MemoryCommand::Rename {
            old_path: p.clone(),
            new_path: "/memories/archival/e2.md".into(),
        })
        .unwrap();
        mem.execute(&MemoryCommand::Delete {
            path: "/memories/archival/e2.md".into(),
        })
        .unwrap();
        assert!(mem.view("/memories/archival/e2.md", None).is_err());
    }

    #[test]
    fn insert_clamps_past_end_and_rejects_missing_file() {
        let (_t, mem) = open_mem();
        mem.create("/memories/archival/i.md", "one\ntwo").unwrap();
        // A line far past the end clamps to append.
        mem.insert("/memories/archival/i.md", 999, "last").unwrap();
        assert!(
            mem.view("/memories/archival/i.md", None)
                .unwrap()
                .ends_with("last")
        );
        assert!(mem.insert("/memories/archival/missing.md", 0, "x").is_err());
    }

    #[test]
    fn delete_and_rename_reject_bad_targets() {
        let (_t, mem) = open_mem();
        assert!(mem.delete("/memories").is_err()); // refuses the root
        assert!(mem.delete("/memories/archival/nope.md").is_err());
        assert!(
            mem.rename("/memories/archival/nope.md", "/memories/archival/x.md")
                .is_err()
        );
    }

    #[test]
    fn write_recall_lands_under_month_and_is_recall_searchable() {
        let (_t, mem) = open_mem();
        // Explicit month key: deterministic placement.
        let msg = mem
            .write_recall("chat-summary", "owner asked about pluto", Some("2026-06"))
            .unwrap();
        assert!(msg.contains("recall/2026-06/chat-summary.md"), "got {msg}");
        let hits = mem.recall_search("pluto", 10).unwrap();
        assert!(hits.iter().any(|h| h.path.contains("recall/2026-06/")));
        // Default month exercises current_month_folder()/civil_from_days().
        let auto = mem.write_recall("auto", "second note", None).unwrap();
        assert!(auto.contains("/recall/"));
    }

    #[test]
    fn tree_listing_and_list_all_reflect_disk() {
        let (_t, mem) = open_mem();
        assert_eq!(mem.tree_listing(), "(no archival/recall files yet)");
        mem.create(
            "/memories/archival/desc.md",
            "---\ndescription: a noted fact\n---\nbody",
        )
        .unwrap();
        let tree = mem.tree_listing();
        assert!(tree.contains("archival/desc.md"));
        assert!(tree.contains("a noted fact"));
        // list_all includes the scaffolded system file plus the new one.
        let all = mem.list_all();
        assert!(all.iter().any(|(rel, _)| rel.starts_with("system/")));
        assert!(all.iter().any(|(rel, _)| rel.ends_with("desc.md")));
    }
}
