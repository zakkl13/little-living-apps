//! Events that feed the serialized manager loop.
//!
//! Two producers — owner messages (from the Telegram poller) and worker-completion events (from the
//! orchestrator). One consumer drains them, one turn at a time. Ids are random (uuid) rather than a
//! module-global counter — ordering comes from the queue, ids are only for tracing.

use serde::{Deserialize, Serialize};

/// How a finished worker settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Completed,
    Failed,
}

impl WorkerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkerStatus::Completed => "completed",
            WorkerStatus::Failed => "failed",
        }
    }
}

/// An item on the manager queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManagerEvent {
    /// A message from the owner (optionally with a local image path for vision).
    OwnerMessage {
        id: String,
        chat_id: i64,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image_path: Option<String>,
    },
    /// A single worker's completion (the worker is ephemeral; this is its only report).
    WorkerEvent {
        id: String,
        /// Trace correlation only — the manager never addresses a worker.
        worker_id: String,
        /// The objective the worker ran; the manager-visible text leads with its first line.
        objective: String,
        status: WorkerStatus,
        summary: String,
    },
}

impl ManagerEvent {
    /// Build an owner message with a fresh id.
    pub fn owner(chat_id: i64, text: impl Into<String>, image_path: Option<String>) -> Self {
        ManagerEvent::OwnerMessage {
            id: new_id(),
            chat_id,
            text: text.into(),
            image_path,
        }
    }

    /// Build a worker-completion event with a fresh id.
    pub fn worker(
        worker_id: impl Into<String>,
        objective: impl Into<String>,
        status: WorkerStatus,
        summary: impl Into<String>,
    ) -> Self {
        ManagerEvent::WorkerEvent {
            id: new_id(),
            worker_id: worker_id.into(),
            objective: objective.into(),
            status,
            summary: summary.into(),
        }
    }

    /// The event's id.
    pub fn id(&self) -> &str {
        match self {
            ManagerEvent::OwnerMessage { id, .. } | ManagerEvent::WorkerEvent { id, .. } => id,
        }
    }

    /// A short label for logs/telemetry.
    pub fn kind_str(&self) -> &'static str {
        match self {
            ManagerEvent::OwnerMessage { .. } => "owner_message",
            ManagerEvent::WorkerEvent { .. } => "worker_event",
        }
    }
}

fn new_id() -> String {
    format!("evt_{}", uuid::Uuid::new_v4().simple())
}
