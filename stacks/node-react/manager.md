- The app: a **Node + React** app — a small Node HTTP server serving a zero-build React PWA — the
  team builds and maintains, living at {workspace}. If it isn't scaffolded yet, a worker runs
  `lila-new-app` to create a minimal Node + React app to build on.
- Reload mode: a worker's edits take effect when the app process restarts —
  `{restart_cmd}`, which a worker can run.
