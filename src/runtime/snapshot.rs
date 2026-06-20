//! Crash-recovery snapshots. Port of `src/runtime/snapshot.ts` with a FRESH schema (v1 — no
//! backwards compatibility with the TS `version: 4` files).
//!
//! Written after each turn (cheap, small) so a crash/restart mid-conversation loses nothing. Memory
//! (`MEMORY_DIR`) is the *semantic* truth; this is the *mechanical* state. The backend owns the
//! manager session's rollout on disk, so we snapshot only the session id, plus the queue + usage.
//! Atomic write (tmp + fsync + rename) so a crash mid-write can never corrupt the file.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::event::ManagerEvent;
use super::telemetry::UsageMeter;
use crate::config::AgentBackend;

/// The persisted runtime state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerSnapshot {
    /// Schema version (fresh line; not compatible with the TS snapshots).
    pub version: u32,
    /// The manager session to resume on cold start; absent before the first turn / after `/new`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manager_session_id: Option<String>,
    /// Which backend produced `manager_session_id` (session ids are cross-incompatible across
    /// backends, so a restart into a different backend drops it and starts fresh).
    pub backend: String,
    pub queue: Vec<ManagerEvent>,
    /// Cumulative token-usage meter (lifetime totals survive a restart).
    #[serde(default)]
    pub usage: UsageMeter,
}

impl ManagerSnapshot {
    pub const VERSION: u32 = 1;

    /// Build a snapshot from live state.
    pub fn new(
        backend: AgentBackend,
        manager_session_id: Option<String>,
        queue: Vec<ManagerEvent>,
        usage: UsageMeter,
    ) -> Self {
        Self {
            version: Self::VERSION,
            manager_session_id,
            backend: backend.as_str().to_string(),
            queue,
            usage,
        }
    }
}

/// Reads/writes the snapshot file under a state directory.
#[derive(Debug, Clone)]
pub struct SnapshotStore {
    path: PathBuf,
}

impl SnapshotStore {
    /// A store writing `snapshot.json` under `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            path: dir.into().join("snapshot.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Atomically persist the snapshot (tmp + fsync + rename).
    pub fn save(&self, snap: &ManagerSnapshot) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec(snap).map_err(std::io::Error::other)?;
        atomic_write(&self.path, &json)
    }

    /// Load the snapshot, or `None` if absent/corrupt/wrong-version (→ fresh start).
    pub fn load(&self) -> Option<ManagerSnapshot> {
        let bytes = std::fs::read(&self.path).ok()?;
        match serde_json::from_slice::<ManagerSnapshot>(&bytes) {
            Ok(snap) if snap.version == ManagerSnapshot::VERSION => Some(snap),
            Ok(other) => {
                tracing::warn!(
                    version = other.version,
                    "snapshot wrong version; starting fresh"
                );
                None
            }
            Err(err) => {
                tracing::warn!(%err, "failed to parse snapshot; starting fresh");
                None
            }
        }
    }
}

/// Write `bytes` to `path` atomically (tmp file + fsync + rename).
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn save_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let store = SnapshotStore::new(tmp.path());
        let usage = UsageMeter {
            input_tokens: 42,
            ..Default::default()
        };
        let snap = ManagerSnapshot::new(
            AgentBackend::Codex,
            Some("sess_abc".into()),
            vec![ManagerEvent::owner(5, "hello", None)],
            usage,
        );
        store.save(&snap).unwrap();
        let loaded = store.load().expect("loads");
        assert_eq!(loaded.manager_session_id.as_deref(), Some("sess_abc"));
        assert_eq!(loaded.backend, "codex");
        assert_eq!(loaded.queue.len(), 1);
        assert_eq!(loaded.usage.input_tokens, 42);
    }

    #[test]
    fn missing_snapshot_is_none() {
        let tmp = TempDir::new().unwrap();
        assert!(SnapshotStore::new(tmp.path()).load().is_none());
    }

    #[test]
    fn corrupt_snapshot_is_none() {
        let tmp = TempDir::new().unwrap();
        let store = SnapshotStore::new(tmp.path());
        std::fs::write(store.path(), b"{not json").unwrap();
        assert!(store.load().is_none());
    }
}
