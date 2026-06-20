//! The Inspector: a read-only, loopback-bound observability plane over the manager's live state
//! (off by default; enabled with `INSPECTOR_ENABLED`). It only observes — it is never a model tool,
//! so the manager's "no hands" boundary stays airtight. See [`server`] for the data sources.

pub mod html;
pub mod server;

pub use server::{InspectorConfig, start};
