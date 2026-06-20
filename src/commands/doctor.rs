//! `lila doctor` — config + backend CLI availability. A light, side-effect-free health probe usable
//! over SSM on the live host.

use crate::config::{AgentBackend, Config};

pub async fn run() -> i32 {
    let cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("CONFIG ERROR: {err}");
            return 1;
        }
    };

    println!("config:   OK ({} backend)", cfg.agent_backend);

    let (bin, override_path) = match cfg.agent_backend {
        AgentBackend::Codex => ("codex", cfg.codex_path_override.clone()),
        AgentBackend::Claude => ("claude", None),
    };
    let found = match &override_path {
        Some(path) => std::path::Path::new(path).exists(),
        None => which_on_path(bin),
    };
    if found {
        println!("backend:  {bin} CLI found");
        0
    } else {
        eprintln!("backend:  {bin} CLI NOT found on PATH (auth/run will fail)");
        1
    }
}

/// True if `bin` resolves on `PATH` (a small, dependency-free `which`).
fn which_on_path(bin: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(bin).is_file())
}
