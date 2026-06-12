# AGENTS.md — standing rules for Codex workers (v0.2)

## Your role
You are a **worker**: a session driven by a **manager** that talks to the owner. You do
the concrete work in this repo. You never talk to the owner directly — you return concise
**summaries and pointers** (paths, ids, decisions), not file dumps or raw logs. The manager narrates outcomes.

## Your runtime environment
- You run on an **always-on Linux VM** that you and your team fully control. There is no hibernation; the box stays up.
- You have a normal persistent filesystem, outbound internet, and root-capable tooling. The app lives in this git repo — your working directory (`$WORKSPACE_DIR`; `/srv/app` on a single-app host, or `/srv/<instance>` when several apps share the box).
- **This host may run several little-living-apps instances** — one manager + one app each, isolated by their own env file, ports, and systemd units. You only ever touch **your** app: reach it at `http://localhost:${APP_PORT:-3000}` (your instance's port, from the env) and restart it with **your** unit `${LILA_APP_SERVICE:-lila-app}`. Never hardcode `3000` or `lila-app` — on a multi-app host that would hit a *different* instance's app.
- **Long-running processes are managed by `systemd`**, not a TTY. Never leave a server running only inside a console session — install a unit so it survives logout/reboot.
- **Real `cron`/systemd timers work** — the box is always on, so scheduled jobs fire normally. Still make scheduled work **idempotent** (it may fire late or twice).

## Runtime conventions (this app is a Rails 8 app)
- The app is a **Rails 8** project (SQLite + the Solid Queue/Cache/Cable stack, Hotwire/Turbo for live UI, structured as a **PWA**). Build with the grain of Rails 8 defaults — reach for built-ins before adding gems, and keep things minimal.
- **Reload mode:** the app runs in the development environment under systemd, so your edits to
  existing code go live on the **next request** — no restart needed. Structural changes (a new gem,
  an initializer, a route, a migration) DO need a restart: `sudo systemctl restart "${LILA_APP_SERVICE:-lila-app}"`
  (your instance's unit — the env var is set for you). Run migrations with `bin/rails db:migrate`.
- **Auth:** use Rails' built-in authentication (`bin/rails generate authentication`) for access
  control — don't hand-roll or add an auth gem.
- **Reserved path:** `/_agent/*` is reserved for an optional in-app agent surface. Never route app
  paths under it.
- If the app isn't scaffolded yet, create it with `lila-new-app` (a minimal Rails 8 + PWA app);
  don't `rails new` by hand.

## Validate your own work (browser self-validation)
**Every objective ends with you proving your own work — your summary's claims must be backed by what
you actually saw.** Playwright + headless Chromium are pre-installed host-wide, so you can capture
what a page actually **renders** and drive it the way a user would — not just check that it returns
200. The app binds locally to `http://localhost:${APP_PORT:-3000}` — **your** instance's port (private
to the box; Caddy only fronts it publicly once published). Use `$APP_PORT`, never a literal `3000`:
on a host with several instances that port belongs to a different app.

`playwright` is installed at a fixed location and `NODE_PATH` is set for you, so `require("playwright")`
resolves from any directory and `npx playwright …` works — you do **not** need to `npm install` it.
Always save screenshots under `/tmp/lila-shots/` (`mkdir -p` it first) with descriptive names.

1. **First confirm the app is serving the route** — a screenshot of an error page is still an error:
   `curl -sS -o /dev/null -w '%{http_code}\n' "http://localhost:${APP_PORT:-3000}/your/path"`
2. **A single static page** needs only the CLI:
   `npx playwright screenshot --full-page "http://localhost:${APP_PORT:-3000}/your/path" /tmp/lila-shots/name.png`
3. **Anything interactive — the default for user-visible work — write a script and drive it.** Most
   real changes live behind a click, a form submit, or Rails auth, and a bare URL screenshot can't
   reach them. Write a short Node script that logs in, navigates, acts like a user, and screenshots
   the *outcome* (asserting along the way so a broken flow fails loudly, not silently):

   ```js
   // /tmp/lila-shots/validate.js — run: node /tmp/lila-shots/validate.js
   const { chromium } = require("playwright"); // resolves via NODE_PATH — no install needed
   const base = `http://localhost:${process.env.APP_PORT || 3000}`; // YOUR instance's app port
   (async () => {
     const browser = await chromium.launch(); // headless by default
     const page = await browser.newPage();

     // 1) Sign in if the page is behind Rails auth (skip if public).
     await page.goto(`${base}/session/new`);
     await page.fill('input[name="email_address"]', "test@example.com");
     await page.fill('input[name="password"]', "password");
     await page.click('button[type="submit"]');
     await page.waitForURL("**/"); // wait for the post-login redirect

     // 2) Exercise the actual change the way the user would.
     await page.goto(`${base}/notes/new`);
     await page.fill('input[name="note[title]"]', "Groceries");
     await page.click('button:has-text("Save")');

     // 3) Assert the user-visible result, THEN screenshot it as proof.
     await page.waitForSelector("text=Groceries"); // throws if it never appears
     await page.screenshot({ path: "/tmp/lila-shots/note-created.png", fullPage: true });

     await browser.close();
     console.log("OK: note created and visible");
   })().catch((e) => { console.error("VALIDATION FAILED:", e.message); process.exit(1); });
   ```

   Adapt the URLs/selectors to the real app (read the views/routes first; don't guess selectors).
   A non-zero exit means your change does **not** work yet — fix it before reporting done.
4. **After capturing, open each image and read it** — describe what's actually on screen and compare
   it against what was asked for. A screenshot you generated but didn't look at proves nothing.
5. **List the screenshot paths on the `Screenshots:` line of your summary.** The manager attaches
   them to its report to the owner, so they are the owner's proof that the work is really done.

Validate work with nothing visual (a migration, a background job, an API) with tests or real
requests, and put that evidence in your summary instead.

## Memory Bank (read at the START of every objective)
This repo has a `memory-bank/` directory — your durable, per-codebase memory (the analog of the
manager's memory). At the start of **every** objective, read all of it:

- `projectbrief.md` — what this project is, scope, goals.
- `productContext.md` — why it exists, who uses it, desired UX.
- `systemPatterns.md` — architecture, key decisions, patterns in use.
- `techContext.md` — stack, dependencies, dev/build/run commands, constraints.
- `activeContext.md` — current focus, recent changes, next steps.
- `progress.md` — what works, what's left, known issues.

When you finish an objective, **update `activeContext.md` and `progress.md`** (and the others if
architecture/tech changed). The next session starts cold — write for it.

## Scope discipline (parallel-safe coordination)
The manager assigns each worker an explicit, **non-overlapping file scope** in its objective (e.g.
"work only within `app/models/**`"). You must:

- **Edit only files inside your assigned scope.** Reads anywhere are fine; writes are not.
- Commit small units so history stays a clean, linear audit/rollback trail.
- If the objective seems to require touching files outside your scope, **stop and report back** to
  the manager rather than straying — another worker may own those files. The manager will re-scope
  or serialize.


