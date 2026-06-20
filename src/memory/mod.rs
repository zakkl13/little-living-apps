//! The manager's durable memory: a `/memories` directory of git-tracked markdown with a derived
//! FTS5 index. Port of `src/memory/*`.

pub mod block;
pub mod fts;
pub mod git;
mod memfs;

use std::fmt;

pub use fts::SearchHit;
pub use memfs::{MEMORY_MOUNT, MemFs, MemFsOptions};

/// Raised on bad tool input (path traversal, missing file, non-unique replace, …). Surfaced to the
/// model as a tool error it can recover from, never a crash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryError(pub String);

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MemoryError {}

/// The six memory-tool commands (shapes mirror the Anthropic memory-tool command union).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryCommand {
    View {
        path: String,
        view_range: Option<[i64; 2]>,
    },
    Create {
        path: String,
        file_text: String,
    },
    StrReplace {
        path: String,
        old_str: String,
        new_str: String,
    },
    Insert {
        path: String,
        insert_line: usize,
        insert_text: String,
    },
    Delete {
        path: String,
    },
    Rename {
        old_path: String,
        new_path: String,
    },
}
