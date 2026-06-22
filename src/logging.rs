//! Tracing setup: structured logs honoring `LOG_LEVEL`
//! (default `info`). The Inspector subscribes to the same `tracing` stream.

use std::sync::Once;

use tracing_subscriber::EnvFilter;

static INIT: Once = Once::new();

/// Initialize the global tracing subscriber from `LOG_LEVEL` (default `info`). Idempotent, so tests
/// and the binary can both call it freely.
pub fn init() {
    INIT.call_once(|| {
        let level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let filter = EnvFilter::try_new(&level)
            .or_else(|_| EnvFilter::try_new("info"))
            .unwrap_or_default();
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .with_writer(std::io::stderr)
            .try_init();
    });
}
