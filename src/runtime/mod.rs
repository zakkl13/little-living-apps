//! Runtime: the event model, the durable queue, crash snapshots, and passive telemetry. The
//! serialized loop that consumes the queue lives in `manager::app` (it owns all the pieces).

pub mod event;
pub mod queue;
pub mod snapshot;
pub mod telemetry;
pub mod trace;

pub use event::{ManagerEvent, WorkerStatus};
pub use queue::EventQueue;
pub use snapshot::{ManagerSnapshot, SnapshotStore};
pub use telemetry::{Telemetry, TokenUsage, UsageMeter};
pub use trace::{EvalTrace, TraceBlock, TraceExt, TraceRecord};
