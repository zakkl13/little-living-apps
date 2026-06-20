//! `lila status` — print persisted runtime state from the snapshot: the active backend, the
//! resumable session, pending queue depth, and the cumulative token breakdown (manager vs worker)
//! so an operator can see where the subscription budget is going at a glance.

use crate::config::Config;
use crate::runtime::{SnapshotStore, UsageMeter};

pub fn run() -> i32 {
    let cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("CONFIG ERROR: {err}");
            return 1;
        }
    };
    println!("backend:   {}", cfg.agent_backend);
    println!("state dir: {}", cfg.manager_state_dir);

    match SnapshotStore::new(&cfg.manager_state_dir).load() {
        Some(snap) => print_snapshot(&snap),
        None => println!("snapshot:  (none yet)"),
    }
    0
}

fn print_snapshot(snap: &crate::runtime::ManagerSnapshot) {
    println!("snapshot:  present (schema v{})", snap.version);
    println!(
        "session:   {}",
        snap.manager_session_id.as_deref().unwrap_or("(fresh)")
    );
    println!("pending:   {} queued event(s)", snap.queue.len());
    print_usage(&snap.usage);
}

/// The token breakdown that matters in prod: manager thread vs the work tier, and the whole-system
/// total. Mirrors the per-trial accounting the eval baseline records, so prod and eval read alike.
fn print_usage(m: &UsageMeter) {
    println!("usage — manager: {} turns", m.manager_turns);
    println!(
        "  manager tokens: {} total ({} in / {} out / {} reasoning, {} cached)",
        m.manager_total_tokens(),
        m.input_tokens,
        m.output_tokens,
        m.reasoning_tokens,
        m.cached_input_tokens,
    );
    println!("usage — workers: {} run(s)", m.worker_turns);
    println!(
        "  worker tokens:  {} total ({} in / {} out)",
        m.worker_total_tokens(),
        m.worker_input_tokens,
        m.worker_output_tokens,
    );
    println!("  grand total:    {} tokens", m.total_tokens());
}
