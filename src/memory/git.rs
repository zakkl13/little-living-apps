//! Git changelog for the memory repo. Port of `src/memory/git.ts`: every backend write is a commit,
//! so history is a literal changelog of what the manager has learned. We shell out to `git` (no
//! native git library) — keeps the dependency tree light and the static binary musl-friendly.

use std::path::Path;
use std::process::Command;

use super::MemoryError;

/// Run a git subcommand in `dir`, returning stdout on success.
fn git(dir: &Path, args: &[&str]) -> Result<String, MemoryError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| MemoryError(format!("git {args:?} failed to spawn: {e}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(MemoryError(format!(
            "git {args:?} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Ensure `dir` exists and is a git repo with a committer identity (idempotent).
pub fn ensure_repo(dir: &Path) -> Result<(), MemoryError> {
    std::fs::create_dir_all(dir).map_err(|e| MemoryError(format!("mkdir {dir:?}: {e}")))?;
    if dir.join(".git").exists() {
        return Ok(());
    }
    git(dir, &["init", "-q"])?;
    // Local identity so commits never depend on the host's global git config.
    git(dir, &["config", "user.name", "little-living-apps manager"])?;
    git(dir, &["config", "user.email", "manager@lila.local"])?;
    Ok(())
}

/// Stage everything and commit iff there is a change. Returns `true` when a commit was made.
/// `git diff --cached --quiet` exits non-zero when there ARE staged changes — that's the commit
/// signal.
pub fn commit_all(dir: &Path, message: &str) -> Result<bool, MemoryError> {
    git(dir, &["add", "-A"])?;
    let clean = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(dir)
        .status()
        .map_err(|e| MemoryError(format!("git diff failed: {e}")))?
        .success();
    if clean {
        return Ok(false); // nothing staged
    }
    git(dir, &["commit", "-q", "-m", message])?;
    Ok(true)
}

/// Number of commits on HEAD (0 if no commits yet). Used by tests/diagnostics.
pub fn commit_count(dir: &Path) -> i64 {
    git(dir, &["rev-list", "--count", "HEAD"])
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(0)
}
