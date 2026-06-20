//! `lila config check` — validate env + billing guard. Exits 0 on success, 1 on error.

use crate::config::Config;

pub fn run() -> i32 {
    match Config::load() {
        Ok(cfg) => {
            println!("OK: configuration valid.");
            println!("  backend:    {}", cfg.agent_backend);
            println!("  sandbox:    {}", cfg.sandbox_mode.as_str());
            println!("  effort:     {}", cfg.manager_reasoning_effort.as_str());
            println!("  workspace:  {}", cfg.workspace_dir);
            println!("  memory:     {}", cfg.memory_dir);
            println!("  allowed:    {} user(s)", cfg.allowed_user_ids.len());
            println!(
                "  app url:    {}",
                if cfg.app_public_url.is_empty() {
                    "(unpublished)"
                } else {
                    &cfg.app_public_url
                }
            );
            println!(
                "  inspector:  {}",
                if cfg.inspector_enabled {
                    format!(
                        "on (loopback :{}, token {})",
                        cfg.inspector_port,
                        if cfg.inspector_token.is_some() {
                            "set"
                        } else {
                            "UNSET — guard with Caddy basic_auth"
                        }
                    )
                } else {
                    "off".to_string()
                }
            );
            0
        }
        Err(err) => {
            eprintln!("CONFIG ERROR: {err}");
            1
        }
    }
}
