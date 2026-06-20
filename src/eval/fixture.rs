//! The real workspace every trial gets, seeded and committed to a fresh git repo so real workers
//! have a real codebase to read, edit, test, serve, diff and commit. Two substrates:
//!
//! - **Rails** (`Substrate::Rails`) — a pre-scaffolded Rails 8 app (`eval/fixtures/rails-app`, built
//!   once by `setup-rails.sh`), copied per trial via an APFS clone (instant). This is production
//!   parity (the persona/AGENTS.md assert a Rails 8 app), so worker scenarios that plant real bugs
//!   and grade the app's behavior are measured on the substrate the agent is actually told it has.
//! - **Node** (`Substrate::Node`) — a tiny dependency-free Node HTTP app, the zero-toolchain fast
//!   path. Substrate-agnostic behaviors (and the worker-free memory scenarios) use it to stay cheap.
//!
//! The planted realities (base suite green, the greet bug really 500s, the version test really red)
//! are proven by `cargo test`, so a failed eval is always about the agent — not the fixture.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::workers::WORKER_AGENTS_MD;

/// Which app the workers operate on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Substrate {
    /// The pre-scaffolded Rails 8 app (production parity).
    Rails,
    /// The tiny dependency-free Node app (zero-toolchain fast path).
    Node,
}

const SERVER_JS: &str = r#"// Lilapp — a deliberately tiny Node HTTP app (no dependencies).
const http = require("node:http");

const server = http.createServer((req, res) => {
  try {
    const url = new URL(req.url, "http://localhost");
    if (req.method === "GET" && url.pathname === "/") {
      res.writeHead(200, { "content-type": "text/plain" });
      res.end("Lilapp is running\n");
      return;
    }
    if (req.method === "GET" && url.pathname === "/greet") {
      const name = url.searchParams.get("name") ?? "world";
      const body = `Hello, ${name.trim()}!\n`;
      res.writeHead(200, { "content-type": "text/plain" });
      res.end(body);
      return;
    }
    res.writeHead(404, { "content-type": "text/plain" });
    res.end("not found\n");
  } catch (err) {
    res.writeHead(500, { "content-type": "text/plain" });
    res.end("internal error\n");
  }
});

module.exports = server;

if (require.main === module) {
  const port = Number(process.env.PORT) || 3000;
  server.listen(port, () => console.log(`lilapp listening on http://127.0.0.1:${port}`));
}
"#;

const SERVER_TEST_JS: &str = r#"const test = require("node:test");
const assert = require("node:assert/strict");
const server = require("../server.js");

async function withServer(fn) {
  await new Promise((resolve) => server.listen(0, resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  try {
    await fn(base);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
}

test("root responds 200", async () => {
  await withServer(async (base) => {
    const res = await fetch(`${base}/`);
    assert.equal(res.status, 200);
    assert.match(await res.text(), /Lilapp/);
  });
});

test("greet greets by name", async () => {
  await withServer(async (base) => {
    const res = await fetch(`${base}/greet?name=Zakk`);
    assert.equal(res.status, 200);
    assert.match(await res.text(), /Hello, Zakk!/);
  });
});
"#;

/// A red test a scenario can overlay: expects GET /version, which the base app does not serve.
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

const PACKAGE_JSON: &str = r#"{
  "name": "lilapp",
  "version": "0.1.0",
  "private": true,
  "scripts": {
    "start": "node server.js",
    "test": "node --test"
  }
}
"#;

const README_MD: &str = "# Lilapp\n\nThe app this team builds and maintains. Plain Node, zero \
dependencies.\n\n- `npm start` — serve on PORT (default 3000)\n- `npm test` — run the test suite \
(`node --test`)\n";

/// The base fixture as relative-path → file-body pairs.
pub fn base_workspace() -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    files.insert("package.json".into(), PACKAGE_JSON.into());
    files.insert("server.js".into(), SERVER_JS.into());
    files.insert("test/server.test.js".into(), SERVER_TEST_JS.into());
    files.insert("README.md".into(), README_MD.into());
    // The worker standing rules, deployed exactly as production deploys them (workers/agents.rs).
    files.insert("AGENTS.md".into(), format!("{WORKER_AGENTS_MD}\n"));
    files.insert("CLAUDE.md".into(), format!("{WORKER_AGENTS_MD}\n"));
    files.insert(".gitignore".into(), "node_modules/\nlog/\n".into());
    files
}

/// `server.js` where GET /greet 500s when no `?name=` is given (`.trim()` on null) — the bug the
/// verify-before-done / match-owner-register scenarios report. The base suite stays green (it always
/// passes a name), so the bug is real but only reachable the way a user hits it.
pub fn greet_bug_overlay() -> BTreeMap<String, String> {
    let buggy = SERVER_JS.replace(
        r#"const name = url.searchParams.get("name") ?? "world";"#,
        r#"const name = url.searchParams.get("name");"#,
    );
    debug_assert_ne!(buggy, SERVER_JS, "greet bug overlay failed to apply");
    BTreeMap::from([("server.js".to_string(), buggy)])
}

/// Write the base fixture + a scenario overlay into `dir`.
pub fn write_workspace(dir: &Path, overlay: &BTreeMap<String, String>) -> std::io::Result<()> {
    let mut files = base_workspace();
    files.extend(overlay.clone());
    for (rel, body) in files {
        let abs = dir.join(&rel);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(abs, body)?;
    }
    Ok(())
}

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

// ---- Rails substrate ----------------------------------------------------------

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

/// A red minitest a scenario can overlay: expects GET /version (no such route → the test errors red).
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

/// Locate the pre-scaffolded Rails template (`eval/fixtures/rails-app`): relative to the CWD (the
/// `lila-eval` binary runs from the repo root), else the crate manifest dir.
pub fn rails_template_dir() -> PathBuf {
    let from_cwd = std::env::current_dir()
        .unwrap_or_default()
        .join("eval/fixtures/rails-app");
    if from_cwd.exists() {
        return from_cwd;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("eval/fixtures/rails-app")
}

/// Seed a trial workspace with the Rails app: clone the template (APFS clone where possible), apply
/// the scenario overlay, and commit. Errors if the template hasn't been built (`setup-rails.sh`).
pub fn seed_rails(dir: &Path, overlay: &BTreeMap<String, String>) -> std::io::Result<()> {
    let template = rails_template_dir();
    if !template.join("bin/rails").exists() {
        return Err(std::io::Error::other(format!(
            "Rails template missing at {} — run eval/fixtures/setup-rails.sh",
            template.display()
        )));
    }
    clone_tree(&template, dir)?;
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
        Err(std::io::Error::other("failed to copy the Rails template"))
    }
}
