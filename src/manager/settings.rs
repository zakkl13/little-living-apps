//! The manager's typed **settings** registry — the structured counterpart to open-ended memories.
//!
//! Each setting is a named, described knob the manager reads (and, when writable, changes) directly
//! over the Lila MCP server, instead of delegating fiddly state changes to a worker. Today the only
//! writable setting is `design` (the app's locked look); the registry is built to grow — read-only
//! environment facts (domain, workspace dir, app service, port) are the obvious next entries, modeled
//! here as non-writable settings.

use std::path::Path;

use crate::design::{self, DesignLock, Pool, catalog_dir, load_index};

/// The settings the registry knows about, for the header/overview. (Extend as settings are added.)
const KNOWN: &str = "design";

/// Read settings: all of them, or just `key`. Returns a human-readable block for the manager.
pub fn get(key: Option<&str>, workspace: &Path) -> String {
    match key {
        Some("design") => design_get(workspace),
        Some(other) => format!("error: unknown setting '{other}' (known: {KNOWN})"),
        None => format!(
            "Settings — structured app config (read here; change writable ones with settings_set):\n\n{}",
            design_get(workspace)
        ),
    }
}

/// Change a writable setting. Returns a summary for the manager to act on/relay, or an error string-ish
/// `anyhow` for unknown/read-only keys and failed applies.
pub fn set(key: &str, value: &str, workspace: &Path) -> anyhow::Result<String> {
    match key {
        "design" => design_set(workspace, value.trim()),
        other => Err(anyhow::anyhow!(
            "'{other}' is not a writable setting (writable: design)"
        )),
    }
}

/// The `design` setting: the app's locked look + the owner-facing options to switch to.
fn design_get(workspace: &Path) -> String {
    let current = DesignLock::parse(
        &std::fs::read_to_string(workspace.join("design.lock")).unwrap_or_default(),
    )
    .map(|l| format!("{} (source={})", l.brand, l.source))
    .unwrap_or_else(|_| "(no locked look yet — the app has no user-visible screen)".to_string());

    format!(
        "design  [writable]  — the app's locked look (one curated Open Design system).\n  \
         current: {current}\n  \
         change it with `settings_set design <brand>`, picking from the owner-facing browsable pool:\n{}",
        browsable_options()
    )
}

/// The browsable pool (default neutrals + the curated slice) as `brand · category · voice` rows, so the
/// manager can propose a couple of fitting options without knowing the catalog.
fn browsable_options() -> String {
    match load_index(&catalog_dir()) {
        Ok(entries) => entries
            .iter()
            .filter(|e| Pool::Browsable.admits(e.pool))
            .map(|e| format!("    {} · {} · {}", e.brand, e.category, e.voice))
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => format!("    (could not read the catalog: {e})"),
    }
}

/// Apply an owner pick: stage the system (stack-agnostic) and tell the manager to hand the stack-fit to
/// a worker.
fn design_set(workspace: &Path, brand: &str) -> anyhow::Result<String> {
    let lock = design::apply_design(workspace, brand)?;
    Ok(format!(
        "Applied design '{}' ({}, source=chosen): refreshed .lila/ with its curated package and \
         re-locked design.lock. The look won't show until a worker fits it to the app — hand one the \
         stack-fit: install .lila/tokens.css where this app's styles live, re-fit the components from \
         .lila/components.html, restart the app, and screenshot-validate.",
        lock.brand,
        lock.pool.as_str()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_default_lock(ws: &Path) {
        let lock = DesignLock {
            brand: "default".into(),
            pool: Pool::Default,
            source: "default".into(),
            seed: 1,
            commit: String::new(),
        };
        std::fs::write(ws.join("design.lock"), lock.to_toml()).unwrap();
    }

    #[test]
    fn set_design_applies_and_relocks_chosen() {
        let ws = tempfile::tempdir().unwrap();
        seed_default_lock(ws.path());
        let summary = set("design", " stripe ", ws.path()).expect("apply");
        assert!(summary.contains("stripe"));
        let lock =
            DesignLock::parse(&std::fs::read_to_string(ws.path().join("design.lock")).unwrap())
                .unwrap();
        assert_eq!(lock.brand, "stripe");
        assert_eq!(lock.source, "chosen");
    }

    #[test]
    fn set_rejects_unknown_or_readonly_keys() {
        let ws = tempfile::tempdir().unwrap();
        assert!(set("domain", "x.example.com", ws.path()).is_err());
        assert!(set("nope", "1", ws.path()).is_err());
    }

    #[test]
    fn get_design_lists_browsable_options() {
        let ws = tempfile::tempdir().unwrap();
        seed_default_lock(ws.path());
        let out = get(Some("design"), ws.path());
        assert!(out.contains("current: default"));
        assert!(out.contains("stripe"), "browsable options include stripe");
    }

    #[test]
    fn get_unknown_key_is_handled() {
        let ws = tempfile::tempdir().unwrap();
        assert!(get(Some("bogus"), ws.path()).contains("unknown setting"));
    }
}
