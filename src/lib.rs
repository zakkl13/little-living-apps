//! little-living-apps agent.
//!
//! A Telegram-driven manager agent that delegates to ephemeral workers, keeps durable git-backed
//! memory, and rides a Codex *or* Claude subscription (never metered API billing).
#![forbid(unsafe_code)]
#![warn(clippy::all)]
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]
#![warn(clippy::dbg_macro, clippy::todo)]
// Cyclomatic complexity is capped at 6/function and enforced in CI by `rust-code-analysis` (see
// .github/workflows + scripts/check-complexity.sh). clippy's `cognitive_complexity` (clippy.toml
// threshold) is a stricter, noisier cousin we leave as a manual guide rather than a hard gate.

pub mod cli;
pub mod commands;
pub mod config;
pub mod eval;
pub mod inspector;
pub mod logging;
pub mod manager;
pub mod memory;
pub mod runtime;
pub mod transport;
pub mod workers;
