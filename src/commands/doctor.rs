//! `lila doctor` — config + backend CLI availability. A light, side-effect-free health probe usable
//! over SSM on the live host.

use crate::backend_cli::backend_cli_status;
use crate::config::Config;

pub async fn run() -> i32 {
    let cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("CONFIG ERROR: {err}");
            return 1;
        }
    };

    println!("config:   OK ({} backend)", cfg.agent_backend);

    let status = backend_cli_status(&cfg, cfg.agent_backend);
    if status.found() {
        println!("backend:  {}", status.found_message());
        0
    } else {
        eprintln!("backend:  {}", status.missing_message());
        1
    }
}
