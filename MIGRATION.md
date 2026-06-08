# Migration: Sprite-Codex-Bot → "Little Living Apps" (host-native)

> **Status: implemented.** Phases 1–6 are done and on `main` (Phase 6 = continuous: `typecheck`
> clean, `npm test` green, `npm run build` loads). Phase 4 shipped as a deliberately thin Rails 8 +
> PWA scaffold (`bin/new-app`), not a heavy template. The one cosmetic leftover is the on-disk
> directory name (`sprite-codex-bot/`); the package + docs are renamed to `little-living-apps`.

Moving off the Fly Sprite onto a plain always-on Linux VM (trial target: a small AWS EC2
Ubuntu instance). No Docker, no hibernation, no webhook. The agent-manager + Codex-worker core
is the keeper; everything Sprite-, webhook-, and keep-alive-specific is **deleted, not adapted**.

**No backwards compatibility.** Every section below lists what to DELETE explicitly. We rip the
old path out — no compat shims, no dead branches, no "off by default" Sprite code.

---

## Target architecture

```
A disposable Ubuntu VM (the host IS the security boundary)
├─ lila-manager           Node service. Claude Opus manager + Codex workers + markdown memory.
│                         Talks to Telegram by LONG-POLLING (outbound only — no inbound port).
├─ the app                One Rails 8 app the agent builds & maintains, run in reload mode so
│                         the agent's edits go live on the next request.
├─ mise                   pins Ruby + Node versions (one static binary, no daemon).
├─ systemd                keeps lila-manager (and later the app) alive across crashes/reboots.
└─ Caddy (optional)       only when you publicly expose the app, purely for auto-HTTPS.
```

Substrate decision is **final**: one substrate, the adopter's Linux host. No substrate
abstraction, no scale-rung migrations — those were rejected. The host-native model is also a
better fit for the agent's edit→live loop than containers.

---

## Keep / Delete / Rewrite ledger

**KEEP unchanged (the actual value):**
- `src/manager/**` — Opus loop, memory tool, prompt assembly (one edit, see Phase 5).
- `src/memory/**` — MemFS + FTS + git. Fully substrate-agnostic.
- `src/workers/**` — Codex runner + orchestrator (minus the keep-alive plumbing, Phase 1).
- `src/runtime/eventQueue.ts`, `src/runtime/snapshot.ts` — queue + crash snapshots still apply.
- `provision/AGENTS.md`, `provision/memory-bank/**` — worker durability docs, substrate-neutral.
- Codex billing-plane guard (refuse `OPENAI_API_KEY`/`CODEX_API_KEY`) — keep verbatim.

**DELETE entirely:**
- `src/runtime/hold.ts` — Sprite keep-alive. An always-on VM never hibernates; nothing to hold.
- `src/transport/webhook.ts` — replaced by long-polling.
- `provision/provision.sh` — Sprite create/push/service orchestration.
- `provision/bootstrap.sh` — Sprite-on-`/workspace`-volume assumptions (replaced, Phase 3).
- `test/sprite.test.ts` — tests the keep-alive hold.
- `DEFECTS.md` → entry **D16** (the single-public-port "dual porting" constraint) — moot off Sprite.

**REWRITE:**
- `src/index.ts` — start a poller instead of a webhook server; drop hold + setWebhook.
- `src/config.ts` — drop webhook/port/Sprite-URL fields; repurpose `publicUrl`.
- `src/manager/prompt.ts` — `SpriteFacts`/`renderSprite()` → host/runtime facts.
- `src/transport/telegram.ts` — drop `setWebhook`; add `getUpdates` + `deleteWebhook`.
- `src/runtime/loop.ts`, `src/workers/orchestrator.ts`, `src/workers/registry.ts` — excise hold.
- `src/app.ts` — drop the `hold` dependency and the sprite-facts wiring.
- Docs: `README.md`, `DESIGN.md`, `AGENTS.md` (root), `package.json` name/description.

---

## Phase 1 — Strip the Sprite keep-alive

The hold was needed because a paused Sprite drops TCP connections mid-turn. An always-on VM has
no pause, so the entire refcount/heartbeat machinery is dead weight.

**Delete:**
- `src/runtime/hold.ts` (the whole file: `SpriteHold`, `createSpriteHold`, `noopHold`).
- `test/sprite.test.ts`.

**Rewrite (excise every `hold` reference):**
- `src/runtime/loop.ts` — remove the `SpriteHold` import, the `hold` dep field, and the
  `acquire()`/`release()` calls around draining. The loop just drains.
- `src/workers/orchestrator.ts` — remove `hold` dep, `ensureHold()`, `dropHold()`, and their
  call sites (start/settle/error).
- `src/workers/registry.ts` — remove the `holding` field from `WorkerRecord` and both
  initializers (lines ~16-17, ~54, ~82).
- `src/app.ts` — remove `hold` from `ManagerAppDeps` and stop passing it to the orchestrator/loop.
- `src/index.ts` — remove `createSpriteHold` import and its use.
- `test/runtime.test.ts` — drop any `noopHold`/hold assertions; loop tests no longer inject a hold.

**Done when:** `grep -ri "hold" src/ | grep -iv household` returns nothing, typecheck clean.

---

## Phase 2 — Transport: webhook → Telegram long-polling

Long-polling makes the box **outbound-only** — no public URL, no inbound port, no TLS for the bot
itself. This is the single biggest adoption win (works behind NAT, on a home box, on a bare EC2
instance with no domain).

**Delete:**
- `src/transport/webhook.ts` (`startWebhookServer`, request handling, health endpoint).
- Any `test/webhook*.test.ts` (none currently; if a webhook test lives inside another file, cut it).

**Rewrite `src/transport/telegram.ts`:**
- Remove `setWebhook`.
- Add `deleteWebhook()` — call once at startup so a stale webhook can't swallow updates
  (Telegram refuses `getUpdates` while a webhook is set).
- Add `getUpdates({ offset, timeout })` — long-poll (e.g. `timeout: 50`).
- Keep `TelegramUpdate`/`TelegramMessage` types (move them here from `webhook.ts`).

**Add `src/transport/poller.ts`:**
- Loop: `getUpdates(offset, timeout=50)` → for each update call `app.ingestTelegramUpdate(update)`
  → advance `offset = update_id + 1`. Catch/log network errors, back off, continue. Stop on a
  shutdown signal. (`app.ingestTelegramUpdate` is already transport-agnostic — the seam holds, no
  app.ts change needed for ingest.)

**Rewrite `src/index.ts`:**
- Remove `startWebhookServer` + the `setWebhook`/`PUBLIC_URL` block.
- At startup: `await telegram.deleteWebhook()`, then start the poller. `app.restore()` + `app.start()`
  stay. Shutdown stops the poller instead of closing the server.

**Rewrite `src/config.ts`:**
- DELETE fields + parsing: `webhookSecret`, `webhookPath`, `port` (no inbound server for the bot).
- Repurpose `publicUrl` → `appPublicUrl` (env `APP_PUBLIC_URL`): "where the app the agent builds is
  served," surfaced to the manager prompt. Empty = not yet published.
- Drop `TELEGRAM_WEBHOOK_SECRET` from required vars; drop the webhook health/port checks.
- `workspaceDir` default `/workspace/project` → `/srv/app`. `memoryDir`/`managerStateDir` defaults
  `/workspace/.manager/*` → `/var/lib/lila/{memory,state}` (or `~/.lila/*`). Keep them env-overridable.

**Add `test/poller.test.ts`** (against `fakeTelegram`): scripts a `getUpdates` batch, asserts each
update is ingested in order and the offset advances. Update `test/config.test.ts` for the dropped
fields.

**Done when:** the bot receives + replies to Telegram messages with no inbound port open.

---

## Phase 3 — Host-native provisioning (TRIAL-READY MILESTONE)

Replace the Sprite scripts with a single bootstrap for a fresh Ubuntu/Debian box. **Declare one
target OS** (Ubuntu 22.04+/Debian 12) so the script stays simple.

**Delete:** `provision/provision.sh`, `provision/bootstrap.sh` (old contents).

**Add `bootstrap.sh`** (run once on the EC2 box, idempotent):
1. Refuse `OPENAI_API_KEY`/`CODEX_API_KEY` (port the existing guard).
2. `apt-get install -y git build-essential libyaml-dev libffi-dev` (Ruby build deps).
3. Install **mise** (single curl install); `.mise.toml` pins Ruby (3.3+) + Node (22+); `mise install`.
4. `npm install -g @openai/codex`; `npm ci && npm run build` for the manager.
5. Create dirs: `WORKSPACE_DIR` (`/srv/app`), `MEMORY_DIR`, `MANAGER_STATE_DIR`; `git init` the
   workspace; copy `provision/AGENTS.md` + `provision/memory-bank/` into the workspace.
6. Install a systemd unit `deploy/lila-manager.service` → enable + start it.
7. Print the `codex login --device-auth` instruction (interactive, one-time; `CODEX_HOME` persists
   on the VM disk — no hibernation concern anymore).

**Add `deploy/lila-manager.service`** — `ExecStart` runs the built manager under mise's Ruby/Node,
`Restart=always`, `EnvironmentFile=/etc/lila/lila.env`, runs as a non-root `lila` user.

**Add `.mise.toml`** at repo root (Ruby + Node pins).

**Milestone:** after Phase 3 the agent system runs on EC2 via long-poll, survives reboots via
systemd, and the host has Ruby+Rails ready so a worker can `rails new`. **This is the version to
trial first** — Phase 4 makes the app half opinionated, but you can already tell the agent to build
an app and watch it work.

---

## Phase 4 — The opinionated Rails 8 runtime template

The standardized "body" every living app starts as — what makes apps come out good-by-default and
what makes the agent able to build/maintain any app reliably.

**Add `runtime/`** — a Rails 8 template (or a `bin/new-app` that runs `rails new` with a fixed
flag set + an opinionated overlay):
- SQLite + Solid Queue/Cache/Cable (zero extra infra on one box).
- Rails' built-in **authentication generator** wired (covers the "scale to 2, auth-protected" case).
- **Hotwire/Turbo** for live UI (the app reacts to its users in real time).
- **PWA** manifest + service worker (installable, mobile chat + mobile app).
- **Reserved paths convention:** `/_agent/*` reserved for an (opt-in) in-app agent chat surface;
  the app owns `/*`. Document it so a worker never collides with it.
- **Reload mode:** run the app with code-reloading on so the agent's edits are live on the next
  request (deliberate iteration-over-throughput trade for personal apps). New-gem/initializer
  changes still need a quick `rails restart` — the worker triggers it.
- An app systemd unit template (`deploy/lila-app@.service`) the agent installs to make the app live.

**Optional `deploy/Caddyfile`** — only used when publicly exposing the app, for auto-HTTPS.
Local/private use (Tailscale/LAN) needs no Caddy.

Update `provision/AGENTS.md` so workers know the runtime conventions (Rails 8, SQLite, reserved
paths, reload mode, how to restart the app).

---

## Phase 5 — Prompt, config rationale, docs

**`src/manager/prompt.ts`:** replace `SpriteFacts`/`renderSprite()` with host/runtime facts —
"you run on a Linux VM you fully control; the app lives at `$WORKSPACE_DIR`, served at
`$APP_PUBLIC_URL` (if published); workers operate the box directly; the app runs in reload mode so
edits go live; restart it with …". Keep the dynamic-injection-from-config pattern. Update the
matching tests in `test/manager.test.ts` (`describe("sprite facts …")` → host facts).

**`src/config.ts` / runner prose:** keep the sandbox config (`danger-full-access` + `never`); change
the rationale comment from "the Sprite is the isolation boundary" → "the disposable VM is the
isolation boundary."

**Docs:**
- `DEFECTS.md` — delete **D16**.
- `README.md` — rewrite around "Little Living Apps": what it is, the 90-sec demo, `bootstrap.sh`
  quickstart, and the **security model up top** (run on a throwaway host you'd hand an agent full
  control of; single-owner allowlist; the app is private until you choose to expose it).
- `DESIGN.md` — update §2/§3/§8/§10/§11 for host-native + long-poll + Rails runtime; drop Sprite/
  webhook/keep-alive sections.
- `package.json` — rename `sprite-codex-bot` → `little-living-apps`; fix `description`; drop the
  Sprite mention. (Directory rename optional/cosmetic.)
- `AGENTS.md` (root) + `SPEC.md` — purge Sprite/webhook references or supersede (SPEC is largely
  v0.1; mark superseded by DESIGN).

---

## Phase 6 — Verify

- `npm run typecheck` + `npm test` green (suite shrinks: sprite/webhook tests gone, poller test in).
- `test/e2e.test.ts` still passes against fakes (owner → workers → narrate), now over the poller seam.
- **Manual EC2 smoke:** provision a fresh instance with `bootstrap.sh`, `codex login`, message the
  bot, confirm it builds and serves a trivial Rails app on the box.

---

## Open decisions — resolved

1. **Runtime archetypes:** ONE minimal Rails 8 + PWA scaffold, "as little code as possible." Not a
   CRUD skeleton, not a second no-UI archetype. The only opinion is Rails 8 with PWA in mind; the
   agent builds on top. → `bin/new-app` leans on Rails 8 defaults + generators.
2. **App process ownership:** the scaffolder installs + starts the app's systemd unit. →
   `bin/new-app` installs `deploy/lila-app.service`.
3. **Private access:** Rails' built-in auth; the host is exposed behind the owner's own domain, so
   Caddy (auto-HTTPS) IS in the default path → `deploy/Caddyfile` + docs.
4. **Project/dir rename:** package + docs renamed to `little-living-apps`; the on-disk directory
   rename is deferred (cosmetic).
```
