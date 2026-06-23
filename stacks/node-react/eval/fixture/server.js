// Lilapp — a tiny Node HTTP server (no dependencies) that serves a zero-build React PWA frontend.
// React/ReactDOM and an in-browser Babel transform are loaded from a CDN, so there is no bundler and
// no `npm install` — the app is a single file the team builds on top of.
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
  const port = Number(process.env.PORT || process.env.APP_PORT) || 3000;
  server.listen(port, () => console.log(`lilapp listening on http://127.0.0.1:${port}`));
}
