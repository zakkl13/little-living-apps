## Runtime conventions (this app is a Rails 8 app)
- The app is a **Rails 8** project (SQLite + the Solid stack, Hotwire/Turbo, structured as a PWA).
  Build with the grain of Rails 8 defaults — reach for built-ins before adding gems.
- **Reload mode:** edits to existing code go live on the **next request** — no restart. Structural
  changes (a new gem, an initializer, a route, a migration) DO need a restart:
  `sudo systemctl restart "${LILA_APP_SERVICE:-lila-app@primary}"`. Run migrations with
  `bin/rails db:migrate`.
- **Auth:** use Rails' built-in authentication (`bin/rails generate authentication`).
- **Reserved path:** `/_agent/*` is reserved. Never route app paths under it.
- If the app isn't scaffolded yet, create it with `lila-new-app` (a minimal Rails 8 + PWA app).
