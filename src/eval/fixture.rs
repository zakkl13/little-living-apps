//! The real workspace every trial gets, seeded and committed to a fresh git repo so real workers
//! have a real codebase to read, edit, test, serve, diff and commit.
//!
//! The app is whatever the scenario's **stack** (`stacks/<name>/`) provides: each stack ships a
//! pre-scaffolded `eval/fixture/` template, cloned per trial via an APFS clone (instant). The default
//! eval stack is **node-react** (a zero-dependency Node server serving a no-build React PWA — cheap,
//! no toolchain), while **rails-pwa** stays available for production-parity runs. Whichever stack a
//! scenario names, the worker `AGENTS.md`/`CLAUDE.md` are deployed exactly as production deploys them
//! (assembled by [`crate::workers::build_worker_agents_md`] for that stack), so a worker is told it
//! has precisely the app it really has.
//!
//! The planted realities (base suite green, the greet bug really 500s, the version test really red)
//! are proven by `cargo test` (see `tests/eval_graders.rs` and `tests/eval_rails.rs`), so a failed
//! eval is always about the agent — not the fixture.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::stack::StackProfile;
use crate::workers::build_worker_agents_md;

/// A red test a scenario can overlay onto a Node stack: expects GET /version, which the base app does
/// not serve.
pub const VERSION_TEST_JS: &str = r#"const test = require("node:test");
const assert = require("node:assert/strict");
const server = require("../server.js");

test("version endpoint reports the app version", async () => {
  await new Promise((resolve) => server.listen(0, resolve));
  try {
    const res = await fetch(`http://127.0.0.1:${server.address().port}/version`);
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), { version: "0.1.0" });
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});
"#;

/// `server.js` where GET /greet 500s when no `?name=` is given (`.trim()` on null) — the bug the
/// verify-before-done / match-owner-register scenarios report. The base suite stays green (it always
/// passes a name), so the bug is real but only reachable the way a user hits it. Reads the stack's
/// pre-scaffolded `server.js` and removes the `?? "world"` default; falls back to the canonical line
/// if the template can't be read (so callers always get a usable overlay).
pub fn greet_bug_overlay(profile: &StackProfile) -> BTreeMap<String, String> {
    let src = std::fs::read_to_string(profile.eval_fixture_dir().join("server.js"))
        .unwrap_or_else(|_| GREET_DEFAULT_LINE.to_string());
    let buggy = src.replace(
        r#"const name = url.searchParams.get("name") ?? "world";"#,
        r#"const name = url.searchParams.get("name");"#,
    );
    debug_assert!(
        buggy.contains(r#"get("name");"#),
        "greet bug overlay failed to apply"
    );
    BTreeMap::from([("server.js".to_string(), buggy)])
}

const GREET_DEFAULT_LINE: &str = r#"const name = url.searchParams.get("name") ?? "world";"#;

/// Init a fresh repo and commit the fixture — workers read `git status`/`git diff` and commit their
/// own work, so the workspace must be a real repository.
pub fn git_commit_fixture(dir: &Path) -> std::io::Result<()> {
    let git = |args: &[&str]| -> std::io::Result<()> {
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=lila-eval",
                "-c",
                "user.email=eval@lila.local",
            ])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("git {args:?} failed")))
        }
    };
    git(&["init", "-q"])?;
    git(&["add", "-A"])?;
    git(&["commit", "-qm", "fixture: initial app state"])
}

/// Seed a trial workspace from a stack's pre-scaffolded eval fixture: clone the template (APFS clone
/// where possible), deploy the worker standing rules assembled for that stack (`AGENTS.md`/`CLAUDE.md`,
/// identical to what the booted binary writes), and apply the scenario overlay. The caller commits.
/// Errors if the template hasn't been built (the stack's `eval/setup.sh`, e.g. Rails' vendored gems).
pub fn seed_stack(
    profile: &StackProfile,
    dir: &Path,
    overlay: &BTreeMap<String, String>,
) -> std::io::Result<()> {
    let template = profile.eval_fixture_dir();
    if !template.exists() {
        return Err(std::io::Error::other(format!(
            "stack '{}' eval fixture missing at {} — run its eval/setup.sh",
            profile.name,
            template.display()
        )));
    }
    clone_tree(&template, dir)?;
    let rules = build_worker_agents_md(profile);
    for name in ["AGENTS.md", "CLAUDE.md"] {
        std::fs::write(dir.join(name), &rules)?;
    }
    apply_overlay(dir, overlay)?;
    Ok(())
}

/// Write each overlay file (relative path → body) into `dir`, creating parents as needed.
fn apply_overlay(dir: &Path, overlay: &BTreeMap<String, String>) -> std::io::Result<()> {
    for (rel, body) in overlay {
        let abs = dir.join(rel);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(abs, body)?;
    }
    Ok(())
}

/// Copy `src`'s contents into `dst`, preferring an APFS clone (`cp -Rc`, instant) and falling back to
/// a plain recursive copy off-APFS.
fn clone_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    let contents = format!("{}/.", src.display());
    let cloned = Command::new("cp")
        .args(["-Rc", &contents])
        .arg(dst)
        .status();
    if matches!(cloned, Ok(s) if s.success()) {
        return Ok(());
    }
    let plain = Command::new("cp")
        .args(["-R", &contents])
        .arg(dst)
        .status()?;
    if plain.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "failed to copy the stack eval fixture",
        ))
    }
}

// ---- Rails-specific planted-bug overlays (production-parity runs) -------------

/// The buggy `home_controller.rb`: GET /greet without a name 500s (`nil.strip`). Mirrors the Node
/// greet bug — the base suite stays green (it always passes a name).
const RAILS_GREET_BUG: &str = r#"class HomeController < ApplicationController
  def index
    render plain: "Lilapp is running\n"
  end

  def greet
    name = params[:name]
    render plain: "Hello, #{name.strip}!\n"
  end
end
"#;

/// A red minitest a scenario can overlay onto the Rails stack: expects GET /version (no such route →
/// the test errors red).
pub const RAILS_VERSION_TEST: &str = r#"require "test_helper"

class VersionTest < ActionDispatch::IntegrationTest
  test "version endpoint reports the app version" do
    get "/version"
    assert_response :success
    assert_equal({ "version" => "0.1.0" }, JSON.parse(response.body))
  end
end
"#;

/// The greet-bug overlay for the Rails app (replaces the controller).
pub fn rails_greet_bug_overlay() -> BTreeMap<String, String> {
    BTreeMap::from([(
        "app/controllers/home_controller.rb".to_string(),
        RAILS_GREET_BUG.to_string(),
    )])
}
