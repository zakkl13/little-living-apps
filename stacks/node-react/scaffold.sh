#!/usr/bin/env bash
# stacks/node-react/scaffold.sh — scaffold ONE instance's app as a minimal Node + React PWA: a small
# zero-dependency Node HTTP server that serves a no-build React frontend (React/ReactDOM from a CDN,
# transformed in the browser). No bundler, no `npm install` — a single file the agent builds on top of.
#
# Invoked by lila-new-app, with the cwd already at the app dir and these vars in
# the environment: LILA_INSTANCE, APP_DIR, APP_PORT, LILA_DOMAIN, SKIP_AUTH, SERVICE_USER, MISE.
# Idempotent: if the app is already scaffolded (server.js present) it is a no-op.
set -euo pipefail

log() { printf '\033[1;35m[scaffold:node-react]\033[0m %s\n' "$*"; }

if [[ -f "$APP_DIR/server.js" ]]; then
  log "Node + React app already present at $APP_DIR — skipping scaffold"
  exit 0
fi

log "Scaffolding a minimal Node + React PWA at $APP_DIR"
mkdir -p "$APP_DIR/test"

cat > "$APP_DIR/package.json" <<'JSON'
{
  "name": "lilapp",
  "version": "0.1.0",
  "private": true,
  "scripts": {
    "start": "node server.js",
    "test": "node --test"
  }
}
JSON

cat > "$APP_DIR/server.js" <<'JS'
// Lilapp — a tiny Node HTTP server (no dependencies) that serves a zero-build React PWA frontend.
const http = require("node:http");

const PAGE = `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <link rel="manifest" href="/manifest.webmanifest" />
    <title>Lilapp</title>
    <script crossorigin src="https://unpkg.com/react@18/umd/react.production.min.js"></script>
    <script crossorigin src="https://unpkg.com/react-dom@18/umd/react-dom.production.min.js"></script>
    <script src="https://unpkg.com/@babel/standalone/babel.min.js"></script>
  </head>
  <body>
    <div id="root"></div>
    <script type="text/babel" data-presets="react">
      function App() {
        return <main><h1>Lilapp is running</h1></main>;
      }
      ReactDOM.createRoot(document.getElementById("root")).render(<App />);
    </script>
  </body>
</html>
`;

const MANIFEST = JSON.stringify({
  name: "Lilapp",
  short_name: "Lilapp",
  start_url: "/",
  display: "standalone",
});

const server = http.createServer((req, res) => {
  try {
    const url = new URL(req.url, "http://localhost");
    if (req.method === "GET" && url.pathname === "/") {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      res.end(PAGE);
      return;
    }
    if (req.method === "GET" && url.pathname === "/manifest.webmanifest") {
      res.writeHead(200, { "content-type": "application/manifest+json" });
      res.end(MANIFEST);
      return;
    }
    if (req.method === "GET" && url.pathname === "/greet") {
      const name = url.searchParams.get("name") ?? "world";
      res.writeHead(200, { "content-type": "text/plain" });
      res.end(`Hello, ${name.trim()}!\n`);
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
  const port = Number(process.env.PORT || process.env.APP_PORT) || 3000;
  server.listen(port, () => console.log(`lilapp listening on http://127.0.0.1:${port}`));
}
JS

cat > "$APP_DIR/test/server.test.js" <<'JS'
const test = require("node:test");
const assert = require("node:assert/strict");
const server = require("../server.js");

test("root serves the React page", async () => {
  await new Promise((resolve) => server.listen(0, resolve));
  try {
    const res = await fetch(`http://127.0.0.1:${server.address().port}/`);
    assert.equal(res.status, 200);
    assert.match(await res.text(), /id="root"/);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});
JS

cat > "$APP_DIR/README.md" <<'MD'
# Lilapp

A tiny Node HTTP server that serves a zero-build React PWA frontend.

- `npm start` — serve on PORT (default 3000)
- `npm test` — run the test suite (`node --test`)
MD

printf 'node_modules/\nlog/\n' > "$APP_DIR/.gitignore"

log "Node + React app scaffolded (server.js, test, package.json)"
