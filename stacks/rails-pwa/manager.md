- The app: a single **Rails 8** app (SQLite + Hotwire, structured as a PWA) the team builds
  and maintains, living at {workspace}. If it isn't scaffolded yet, a worker runs
  `lila-new-app` to create a minimal Rails 8 + PWA app to build on.
- Reload mode: a worker's edits to existing code go live on the NEXT request — no restart.
  Only structural changes (a new gem, an initializer, a route, a migration) need
  `{restart_cmd}`, which a worker can run.
