//! Pluggable stack profiles: the data-driven "what kind of app the team builds" plugin.
//!
//! Today the team's default app is a Rails 8 + PWA; that is now *one* stack among many. A stack lives
//! in `stacks/<name>/` as a `stack.toml` contract (see `stacks/README.md`) plus the fragment files it
//! references — a scaffold script and two prompt fragments. [`StackProfile::load`] reads the contract
//! and inlines the prompt fragments, so every consumer (the manager/worker prompts, the eval graders,
//! and the generic `bin/new-app`) reads one struct instead of re-encoding "Rails PWA" in six places.
//!
//! `stacks/` resolves the same way the eval fixture does: the current working directory first (dev,
//! on-box, and tests all run from the repo root), then the crate manifest dir as a fallback.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

/// A loaded stack profile: the `stack.toml` contract with its prompt fragments inlined.
#[derive(Debug, Clone)]
pub struct StackProfile {
    /// Canonical stack name (matches the directory under `stacks/`).
    pub name: String,
    /// Human-readable label (e.g. "Rails 8 + PWA").
    pub display: String,
    /// App-language toolchain pins, merged into bootstrap's `mise use -g …` (node is always added).
    pub toolchain: BTreeMap<String, String>,
    /// Absolute path to the per-stack scaffold script `bin/new-app` runs at instance creation.
    pub scaffold_script: PathBuf,
    /// Portable serve command (binds localhost, reads `${APP_PORT}`); the systemd unit + eval probe.
    pub serve_exec: String,
    /// Process environment for the serve unit (rendered as systemd `Environment=` lines).
    pub serve_env: BTreeMap<String, String>,
    /// The app's own test command (drives the eval `tests_green` grader).
    pub test_cmd: String,
    /// A route the app serves once booted (the eval probe waits on it before probing the target).
    pub health_path: String,
    /// Optional failure-tolerant step the eval probe runs before booting the app (e.g. Rails'
    /// `bin/rails db:prepare`). Empty for stacks that need no preparation.
    pub probe_prepare: String,
    /// The "## Runtime conventions" fragment spliced into the worker `AGENTS.md`.
    pub worker_prompt: String,
    /// The "the app" fragment spliced into the manager's runtime-environment section.
    pub manager_prompt: String,
    /// The optional design contract: where this stack keeps its tokens + how a worker applies the
    /// locked system in this stack's idiom. `None` ⇒ the stack opted out (the design skill + visual
    /// check no-op for it, exactly like a stack that omits `[validate].prepare`).
    pub design: Option<DesignSpec>,
    /// Absolute path to this stack's directory (`stacks/<name>/`).
    pub dir: PathBuf,
}

/// The per-stack design contract (`[design]` in `stack.toml`), parallel to `[validate]`.
#[derive(Debug, Clone)]
pub struct DesignSpec {
    /// Canonical token sink the render writes and the design skill edits (e.g.
    /// `app/assets/stylesheets/tokens.css`).
    pub tokens_path: String,
    /// The fragment (`design.md`) spliced into the worker `AGENTS.md`: how to apply tokens +
    /// components in THIS stack's idiom. Read via the same `read_fragment` helper as the prompts.
    pub apply_prompt: String,
}

/// The `stack.toml` contract, as parsed before fragment inlining.
#[derive(Deserialize)]
struct StackToml {
    name: String,
    display: String,
    #[serde(default)]
    toolchain: BTreeMap<String, String>,
    scaffold: ScaffoldToml,
    serve: ServeToml,
    validate: ValidateToml,
    prompt: PromptToml,
    /// Optional: a stack with no `[design]` block opts out of the design system.
    design: Option<DesignToml>,
}

#[derive(Deserialize)]
struct DesignToml {
    tokens_path: String,
    apply: String,
}

#[derive(Deserialize)]
struct ScaffoldToml {
    script: String,
}

#[derive(Deserialize)]
struct ServeToml {
    exec: String,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct ValidateToml {
    test_cmd: String,
    health_path: String,
    #[serde(default)]
    prepare: String,
}

#[derive(Deserialize)]
struct PromptToml {
    worker: String,
    manager: String,
}

/// Resolve the `stacks/` directory: CWD first (dev / on-box / tests run from the repo root), then the
/// crate manifest dir. Mirrors `eval::fixture`'s template resolution.
pub fn stacks_dir() -> PathBuf {
    let from_cwd = std::env::current_dir().unwrap_or_default().join("stacks");
    if from_cwd.exists() {
        return from_cwd;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stacks")
}

impl StackProfile {
    /// Load the named stack from the resolved `stacks/` dir.
    pub fn load(name: &str) -> anyhow::Result<Self> {
        Self::load_dir(&stacks_dir().join(name))
    }

    /// Load a stack from an explicit `stacks/<name>/` directory (used by tests).
    pub fn load_dir(dir: &Path) -> anyhow::Result<Self> {
        let toml_path = dir.join("stack.toml");
        let raw = std::fs::read_to_string(&toml_path)
            .with_context(|| format!("reading stack contract {}", toml_path.display()))?;
        let parsed: StackToml = toml::from_str(&raw)
            .with_context(|| format!("parsing stack contract {}", toml_path.display()))?;
        let worker_prompt = read_fragment(dir, &parsed.prompt.worker)?;
        let manager_prompt = read_fragment(dir, &parsed.prompt.manager)?;
        let design = load_design(dir, parsed.design)?;
        Ok(Self {
            name: parsed.name,
            display: parsed.display,
            toolchain: parsed.toolchain,
            scaffold_script: dir.join(&parsed.scaffold.script),
            serve_exec: parsed.serve.exec,
            serve_env: parsed.serve.env,
            test_cmd: parsed.validate.test_cmd,
            health_path: parsed.validate.health_path,
            probe_prepare: parsed.validate.prepare,
            worker_prompt,
            manager_prompt,
            design,
            dir: dir.to_path_buf(),
        })
    }

    /// This stack's pre-scaffolded eval fixture template (`stacks/<name>/eval/fixture`).
    pub fn eval_fixture_dir(&self) -> PathBuf {
        self.dir.join("eval/fixture")
    }
}

/// Resolve the optional `[design]` block into a [`DesignSpec`], inlining its `apply` fragment.
fn load_design(dir: &Path, design: Option<DesignToml>) -> anyhow::Result<Option<DesignSpec>> {
    match design {
        Some(d) => Ok(Some(DesignSpec {
            tokens_path: d.tokens_path,
            apply_prompt: read_fragment(dir, &d.apply)?,
        })),
        None => Ok(None),
    }
}

/// Read a fragment file referenced by `stack.toml`, trimming trailing whitespace so the caller
/// controls the spacing when splicing it into a prompt.
fn read_fragment(dir: &Path, rel: &str) -> anyhow::Result<String> {
    let path = dir.join(rel);
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading prompt fragment {}", path.display()))?;
    Ok(body.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rails_pwa_profile_parses() {
        let p = StackProfile::load("rails-pwa").expect("rails-pwa loads");
        assert_eq!(p.name, "rails-pwa");
        assert_eq!(p.display, "Rails 8 + PWA");
        assert_eq!(p.test_cmd, "bin/rails test");
        assert_eq!(p.health_path, "/up");
        assert_eq!(p.toolchain.get("ruby").map(String::as_str), Some("3.3"));
        assert!(p.serve_exec.contains("bin/rails server"));
        assert_eq!(
            p.serve_env.get("RAILS_ENV").map(String::as_str),
            Some("development")
        );
        assert!(p.worker_prompt.contains("Rails 8"));
        assert!(p.manager_prompt.contains("{workspace}"));
        assert!(p.scaffold_script.ends_with("scaffold.sh"));
        assert!(p.eval_fixture_dir().join("bin/rails").exists());

        // rails-pwa opts into the design system: tokens sink + an apply fragment.
        let design = p.design.expect("rails-pwa has a [design] block");
        assert_eq!(design.tokens_path, "app/assets/stylesheets/tokens.css");
        assert!(
            design.apply_prompt.contains("tokens"),
            "apply fragment explains how to apply the tokens"
        );
    }

    #[test]
    fn node_react_profile_parses() {
        let p = StackProfile::load("node-react").expect("node-react loads");
        assert_eq!(p.name, "node-react");
        assert_eq!(p.test_cmd, "node --test");
        assert_eq!(p.health_path, "/");
        assert_eq!(p.serve_exec, "node server.js");
        assert!(p.toolchain.is_empty(), "node-react adds no app toolchain");
        assert!(p.serve_env.is_empty());
        assert!(p.worker_prompt.contains("React"));
        assert!(p.manager_prompt.contains("{service}"));
        assert!(
            p.design.is_none(),
            "node-react has no [design] block ⇒ opts out gracefully"
        );
    }

    #[test]
    fn unknown_stack_errors() {
        assert!(StackProfile::load("nope-not-a-stack").is_err());
    }

    /// Every directory under `stacks/` is a valid stack — guards a newly added stack against a
    /// malformed contract or a missing fragment file.
    #[test]
    fn every_in_repo_stack_loads() {
        let mut loaded = 0;
        for entry in std::fs::read_dir(stacks_dir())
            .expect("stacks/ dir exists")
            .flatten()
        {
            let dir = entry.path();
            if !dir.join("stack.toml").exists() {
                continue; // skip stacks/README.md and any non-stack entries
            }
            let profile = StackProfile::load_dir(&dir)
                .unwrap_or_else(|e| panic!("stack at {} must load: {e}", dir.display()));
            assert!(!profile.worker_prompt.is_empty());
            assert!(!profile.manager_prompt.is_empty());
            assert!(
                profile.scaffold_script.exists(),
                "scaffold script must exist"
            );
            loaded += 1;
        }
        assert!(loaded >= 2, "expected at least rails-pwa + node-react");
    }
}
