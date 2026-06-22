//! The durable in-memory queue feeding the serialized loop (the storage half — async wakeup is
//! provided by an `mpsc` channel in the app).
//!
//! Serializing turns is the core invariant that keeps memory + transcript coherent without locks:
//! one consumer drains this, one turn at a time.

use std::collections::VecDeque;

use super::event::ManagerEvent;

/// A FIFO queue of pending events, snapshot-able for cold-restart recovery.
#[derive(Debug, Default)]
pub struct EventQueue {
    items: VecDeque<ManagerEvent>,
}

impl EventQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an event to the back of the queue.
    pub fn push(&mut self, event: ManagerEvent) {
        self.items.push_back(event);
    }

    /// Remove and return the front event.
    pub fn pop(&mut self) -> Option<ManagerEvent> {
        self.items.pop_front()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// A clone of the current contents (for snapshotting).
    pub fn snapshot(&self) -> Vec<ManagerEvent> {
        self.items.iter().cloned().collect()
    }

    /// Replace the contents (for restore).
    pub fn load(&mut self, events: Vec<ManagerEvent>) {
        self.items = events.into();
    }

    /// True if any queued event is a worker event (used by the reply gate).
    pub fn has_worker_event(&self) -> bool {
        self.items
            .iter()
            .any(|e| matches!(e, ManagerEvent::WorkerEvent { .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_order_and_snapshot_roundtrip() {
        let mut q = EventQueue::new();
        q.push(ManagerEvent::owner(1, "a", None));
        q.push(ManagerEvent::owner(1, "b", None));
        let snap = q.snapshot();
        assert_eq!(snap.len(), 2);

        let mut restored = EventQueue::new();
        restored.load(snap);
        assert_eq!(restored.len(), 2);
        let first = restored.pop().unwrap();
        assert!(matches!(first, ManagerEvent::OwnerMessage { text, .. } if text == "a"));
    }

    #[test]
    fn detects_worker_events_for_reply_gate() {
        let mut q = EventQueue::new();
        q.push(ManagerEvent::owner(1, "hi", None));
        assert!(!q.has_worker_event());
        q.push(ManagerEvent::worker(
            "w1",
            "obj",
            crate::runtime::event::WorkerStatus::Completed,
            "done",
        ));
        assert!(q.has_worker_event());
    }
}
