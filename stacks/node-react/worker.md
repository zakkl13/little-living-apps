## Runtime conventions (this app is a Node + React app)
- The app is a small **Node HTTP server** that serves a **zero-build React** PWA frontend (React +
  ReactDOM loaded from a CDN and transformed in the browser — no bundler, no `npm install`). Keep it
  dependency-light: reach for Node's built-ins before adding packages.
- **Reload mode:** the app process is managed by `systemd`, so changes to server code take effect on
  the next process start: `sudo systemctl restart "${LILA_APP_SERVICE:-lila-app@primary}"`.
- **Reserved path:** `/_agent/*` is reserved. Never route app paths under it.
- If the app isn't scaffolded yet, create it with `lila-new-app` (a minimal Node + React app).
- **Validate** with `node --test`; the app's health route is `/`.
