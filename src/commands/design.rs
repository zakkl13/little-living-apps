//! `lila design draw <choice>` / `lila design tokens <file>` — the bridge that lets `lila-new-app` and
//! a stack's `scaffold.sh` resolve and render the vendored design catalog without re-encoding the
//! draw/lock logic in bash. The Rust side (`crate::design`) owns the pool-bounded draw and the token
//! render; the shell just consumes the `LILA_DESIGN_*` assignments and the rendered `tokens.css`.

use crate::cli::DesignAction;
use crate::design::{Draw, Pool, catalog_commit, catalog_dir, draw_system, load_index};

/// Dispatch a `lila design` action. Exit 0 on success, 1 on failure.
pub fn run(action: DesignAction) -> i32 {
    match action {
        DesignAction::Draw { choice } => draw(&choice),
        DesignAction::List { pool } => list(pool.as_deref().unwrap_or("browsable")),
    }
}

/// List the systems in `pool` (with nesting) as `brand · category · voice` rows for the look-change flow.
fn list(pool: &str) -> i32 {
    let Some(want) = Pool::parse(pool) else {
        eprintln!("unknown pool '{pool}' (want default | browsable | full)");
        return 1;
    };
    let entries = match load_index(&catalog_dir()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("could not read the catalog index: {e:#}");
            return 1;
        }
    };
    for e in entries.iter().filter(|e| want.admits(e.pool)) {
        println!("{} · {} · {}", e.brand, e.category, e.voice);
    }
    0
}

/// Resolve a choice and print the draw as shell assignments. Single-quoted (like `lila stack`) so a
/// path with odd characters survives `eval`.
fn draw(choice: &str) -> i32 {
    let dir = catalog_dir();
    let drawn = match draw_system(&dir, choice) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("could not draw design '{choice}': {e:#}");
            return 1;
        }
    };
    let commit = catalog_commit(&dir);
    print_draw(&drawn, &commit);
    0
}

fn print_draw(d: &Draw, commit: &str) {
    // The system's vendored package dir — the scaffold copies its curated tokens.css + reference
    // assets from here (parent of DESIGN.md).
    let dir = d
        .design_md
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    println!("{}", assign("LILA_DESIGN_BRAND", &d.brand));
    println!(
        "{}",
        assign("LILA_DESIGN_FILE", &d.design_md.to_string_lossy())
    );
    println!("{}", assign("LILA_DESIGN_DIR", &dir));
    println!("{}", assign("LILA_DESIGN_POOL", d.pool.as_str()));
    println!("{}", assign("LILA_DESIGN_SOURCE", &d.source));
    println!("{}", assign("LILA_DESIGN_SEED", &d.seed.to_string()));
    println!("{}", assign("LILA_DESIGN_COMMIT", commit));
}

/// `KEY='value'` with single quotes escaped so the line is safe to `eval` (mirrors `commands::stack`).
fn assign(key: &str, value: &str) -> String {
    let escaped = value.replace('\'', r"'\''");
    format!("{key}='{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::DesignAction;

    #[test]
    fn draw_succeeds_for_a_real_system() {
        assert_eq!(
            run(DesignAction::Draw {
                choice: "random:1".into()
            }),
            0
        );
    }

    #[test]
    fn draw_fails_for_unknown_pin() {
        assert_eq!(
            run(DesignAction::Draw {
                choice: "nope-not-real".into()
            }),
            1
        );
    }

    #[test]
    fn list_succeeds_for_a_pool_and_rejects_garbage() {
        assert_eq!(
            run(DesignAction::List {
                pool: Some("browsable".into())
            }),
            0
        );
        assert_eq!(run(DesignAction::List { pool: None }), 0);
        assert_eq!(
            run(DesignAction::List {
                pool: Some("nope".into())
            }),
            1
        );
    }
}
