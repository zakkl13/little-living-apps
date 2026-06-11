// The real workspace every trial gets: a deliberately tiny, dependency-free Node HTTP app, seeded
// onto disk and committed to a fresh git repo so real workers have a real codebase — something to
// read, edit, test (`node --test`), serve, diff and commit. Scenarios overlay files on top (that's
// how a trial plants a real bug or a red test) and may mutate the tree imperatively (mtimes etc.)
// before the fixture commit.
//
// Why Node and not Rails: production's substrate (Rails 8 + systemd + lila-new-app) needs a
// provisioned Linux host and minutes of scaffolding per trial; the behaviors this suite grades —
// delegation, validation, reply discipline, memory, autonomy, honesty — are substrate-agnostic, and
// real Codex workers orient off the actual workspace (the protocol makes them look). Everything
// else IS production: real manager, real workers, real shell, real git.

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

const SERVER_JS = `// Lilapp — a deliberately tiny Node HTTP app (no dependencies).
const http = require("node:http");

const server = http.createServer((req, res) => {
  try {
    const url = new URL(req.url, "http://localhost");
    if (req.method === "GET" && url.pathname === "/") {
      res.writeHead(200, { "content-type": "text/plain" });
      res.end("Lilapp is running\\n");
      return;
    }
    if (req.method === "GET" && url.pathname === "/greet") {
      const name = url.searchParams.get("name") ?? "world";
      const body = \`Hello, \${name.trim()}!\\n\`;
      res.writeHead(200, { "content-type": "text/plain" });
      res.end(body);
      return;
    }
    res.writeHead(404, { "content-type": "text/plain" });
    res.end("not found\\n");
  } catch (err) {
    res.writeHead(500, { "content-type": "text/plain" });
    res.end("internal error\\n");
  }
});

module.exports = server;

if (require.main === module) {
  const port = Number(process.env.PORT) || 3000;
  server.listen(port, () => console.log(\`lilapp listening on http://127.0.0.1:\${port}\`));
}
`;

/** Same handler, but /greet 500s when no ?name= is given (calls .trim() on null). The bug the
 *  verify-before-done scenario reports; the base test suite stays green (it always passes a name). */
const SERVER_JS_GREET_BUG = SERVER_JS.replace(
  `const name = url.searchParams.get("name") ?? "world";`,
  `const name = url.searchParams.get("name");`,
);
if (SERVER_JS_GREET_BUG === SERVER_JS) throw new Error("fixture bug overlay failed to apply");

const SERVER_TEST_JS = `const test = require("node:test");
const assert = require("node:assert/strict");
const server = require("../server.js");

async function withServer(fn) {
  await new Promise((resolve) => server.listen(0, resolve));
  const base = \`http://127.0.0.1:\${server.address().port}\`;
  try {
    await fn(base);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
}

test("root responds 200", async () => {
  await withServer(async (base) => {
    const res = await fetch(\`\${base}/\`);
    assert.equal(res.status, 200);
    assert.match(await res.text(), /Lilapp/);
  });
});

test("greet greets by name", async () => {
  await withServer(async (base) => {
    const res = await fetch(\`\${base}/greet?name=Zakk\`);
    assert.equal(res.status, 200);
    assert.match(await res.text(), /Hello, Zakk!/);
  });
});
`;

/** A red test a scenario can overlay: expects GET /version, which the base app does not serve. */
export const VERSION_TEST_JS = `const test = require("node:test");
const assert = require("node:assert/strict");
const server = require("../server.js");

test("version endpoint reports the app version", async () => {
  await new Promise((resolve) => server.listen(0, resolve));
  try {
    const res = await fetch(\`http://127.0.0.1:\${server.address().port}/version\`);
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), { version: "0.1.0" });
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});
`;

export const BASE_WORKSPACE: Record<string, string> = {
  "package.json": JSON.stringify(
    {
      name: "lilapp",
      version: "0.1.0",
      private: true,
      scripts: { start: "node server.js", test: "node --test" },
    },
    null,
    2,
  ) + "\n",
  "server.js": SERVER_JS,
  "test/server.test.js": SERVER_TEST_JS,
  "README.md":
    "# Lilapp\n\nThe app this team builds and maintains. Plain Node, zero dependencies.\n\n" +
    "- `npm start` — serve on PORT (default 3000)\n" +
    "- `npm test` — run the test suite (`node --test`)\n",
  ".gitignore": "node_modules/\nlog/\n",
};

/** The buggy server overlay for scenarios that need a real, reproducible user-visible bug. */
export const GREET_BUG_OVERLAY: Record<string, string> = { "server.js": SERVER_JS_GREET_BUG };

/** Write base fixture + scenario overlay into the trial workspace. */
export function writeWorkspace(dir: string, overlay: Record<string, string> = {}): void {
  const files = { ...BASE_WORKSPACE, ...overlay };
  for (const [rel, body] of Object.entries(files)) {
    const abs = join(dir, rel);
    mkdirSync(dirname(abs), { recursive: true });
    writeFileSync(abs, body);
  }
}

/** Init a fresh repo and commit the fixture — workers are told to read `git status`/`git diff`
 *  and to commit their own work, so the workspace must be a real repository. */
export function gitCommitFixture(dir: string): void {
  const git = (...argv: string[]): void => {
    execFileSync(
      "git",
      ["-c", "user.name=lila-eval", "-c", "user.email=eval@lila.local", "-c", "commit.gpgsign=false", ...argv],
      { cwd: dir, stdio: "pipe" },
    );
  };
  git("init", "-q");
  git("add", "-A");
  git("commit", "-qm", "fixture: initial app state");
}
