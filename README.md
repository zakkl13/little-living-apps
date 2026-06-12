# little-living-apps

Build useful, stateful little apps you never have to read the code for. You talk to a **Codex
manager** over Telegram; it orchestrates headless **OpenAI Codex** workers that build and maintain
**one app** on a Linux host you control. The manager plans, remembers, and delegates; the workers do
the concrete work in the app's repo. "Living" = the agent team keeps maintaining it, not a one-shot
generator.

**One billing plane.** As of v0.3 the manager is itself a long-lived Codex thread, so *manager and
workers both ride your single **ChatGPT subscription*** — **no OpenAI API key, no metered plane.**
The cost story collapses to "the subscription."

> **Bring your own everything.** This is an open pattern, not a service: your host, your Telegram
> bot, your ChatGPT login. Not a money-making endeavor — adopt it, fork it, run it on a box you own.

```
 Telegram ──long-poll (outbound)──▶ EVENT QUEUE ──▶ manager turn (SERIALIZED)
  (owner)   getUpdates              owner_msg | worker_event      │  Codex thread (one, long-lived)
        ▲                                                         │  read-only sandbox, no shell/net
        │ reply text (agent_message)                              │  reasoning = xhigh
        │                    ┌── Lila MCP tools (the only hands) ──┘  (loopback HTTP, bearer token)
        │                    │  memory_*  → MemFS (git markdown + sqlite FTS)
        │                    │  subagent_start → Codex workers (single-shot, async, parallel, scoped)
        └────────────────────┤  an ordinary message IS the reply (NO_REPLY = stay silent)
                             └── workers operate the box: build & maintain the app at $WORKSPACE_DIR
```

The bot talks to Telegram by **outbound long-poll** — it opens no inbound port and needs no public
URL or TLS, so it runs behind NAT, on a home box, or on a bare cloud VM. The manager has **no
shell/file/network tools**: it runs in a **read-only sandbox with `shell_tool` and `web_search`
off**, so its only hands are the **Lila MCP** tools (`memory_*`, `subagent_start`,
`memory_search`/`recall_search`) served in-process over loopback HTTP. Workers are **purely
ephemeral**: each `subagent_start` births a fresh Codex thread for one objective; it reports back
once as an event and is gone — no roster, no resume, no steer. Continuity lives in the workspace,
the git history, and memory, never in worker state. `view_image` stays on, so it
can see owner-sent screenshots. Its plain agent message is delivered straight to Telegram; `NO_REPLY`
lets it absorb an event silently. Workers have full access under standing rules
(`provision/AGENTS.md`). Everything survives a restart: Codex owns the manager thread's rollout on
disk, and per-turn snapshots + a git-backed memory repo carry the rest.

## Security model — read this first

- **The host IS the security boundary.** Codex workers run with **`danger-full-access`** and
  `approvalPolicy: "never"`. The manager hands them full control of the box. Run this **only on a
  disposable host you are willing to hand an agent** — a fresh VM, not your laptop or a box with
  other people's data on it.
- **Single owner.** Only the Telegram user IDs in `ALLOWED_USER_IDS` can talk to the bot;
  everyone else gets a refusal and never reaches the model.
- **The app is private until you publish it.** Long-poll means nothing about the box is reachable
  from outside unless you deliberately expose the app the agent builds.
- **Never set `OPENAI_API_KEY` / `CODEX_API_KEY`.** Either flips Codex to metered API billing
  instead of the ChatGPT subscription. Now that the manager rides the subscription too, this is the
  *only* billing protection left — the bot (and `bootstrap.sh`) refuse to start if either is set.

## Quickstart (fresh Ubuntu 22.04+/Debian 12 host)

```bash
git clone <this repo> && cd little-living-apps
cp .env.example .env && $EDITOR .env     # TELEGRAM_BOT_TOKEN, ALLOWED_USER_IDS (no API key needed)
sudo bash bootstrap.sh                    # mise -> Ruby+Node, Codex CLI, build, data dirs, systemd
# then, one-time, authenticate Codex on the ChatGPT subscription:
sudo -u <you> -H CODEX_HOME=/var/lib/lila/codex ~/.local/bin/mise exec -- codex login --device-auth
sudo systemctl start lila-manager        # bootstrap starts it automatically once auth is present
journalctl -u lila-manager -f            # watch it
```

Then message your bot. The manager delegates to workers and reports back; `/status` shows workers +
state; `/new` starts a fresh manager thread (long-term memory is kept). `bootstrap.sh` is idempotent
— re-run it after pulling changes (then `sudo systemctl restart lila-manager`).

**The app.** Ask the bot to build something and a worker scaffolds the app with `lila-new-app` — a
minimal **Rails 8 + PWA** project (SQLite + the Solid stack, Hotwire, Rails' built-in auth) run in
**reload mode** (edits go live on the next request). It binds to `127.0.0.1:3000`, private to the
box. To publish it behind your own domain with automatic HTTPS, point DNS at the host and run Caddy
with `deploy/Caddyfile`.

**Several apps on one host.** The model stays *one brain → one app*; you just run it more than once.
After `bootstrap.sh`, add an independent instance — its own Codex manager, Telegram bot, workspace,
data dir, ports, and domain — with `bin/new-instance`:

```bash
sudo LILA_DOMAIN=cm.example.com APP_PORT=3001 INSPECTOR_PORT=9091 \
     TELEGRAM_BOT_TOKEN=<new-bot-token> \
     bash bin/new-instance cm
```

Each instance runs under the systemd **template units** `lila-manager@<name>` / `lila-app@<name>`
(reading `/etc/lila/<name>.env`), and gets its own Caddy site block at `/etc/caddy/sites/<name>.caddy`.
Create a separate bot via @BotFather per instance (one bot can't be long-polled twice). Codex rides
one ChatGPT subscription, so authenticate each instance's `CODEX_HOME` to the **same** account — watch
concurrency if you run many at once.

## Layout

| Path | What |
|---|---|
| `bootstrap.sh` | One-shot host setup: mise (Ruby+Node), Codex CLI, build, data dirs, systemd unit. |
| `bin/new-app` | Thin scaffolder for the app: minimal Rails 8 + PWA (+ built-in auth), installs+starts its service. Run via `lila-new-app`. |
| `bin/new-instance` | Stand up an *additional* living app on the same host (one brain → one app, multiplied): own env file, workspace, data dir, ports, bot, domain, under `lila-{manager,app}@<name>`. |
| `deploy/` | `lila-manager.service`/`lila-app.service` (single-instance units) · `lila-manager@.service`/`lila-app@.service` (multi-instance templates) · `Caddyfile` (publishes the primary + imports per-instance site blocks). |
| `.mise.toml` | Pinned Ruby + Node versions. |
| `src/config.ts` | Env loading + validation; no API key required; **refuses to start if `OPENAI_API_KEY`/`CODEX_API_KEY` is set** (billing-flip guard — the only billing protection left). |
| `src/app.ts` | Composition root: wires memory + orchestrator + manager backend + loop + snapshots; `ingestTelegramUpdate` (incl. photo intake), `persist`/`restore`. |
| `src/manager/managerCodex.ts` · `driver.ts` | The locked-down Codex thread factory (the model seam) + the `ManagerDriver` that turns one event into one streamed turn (deliver `agent_message`, honor `NO_REPLY`, record usage). |
| `src/manager/backend.ts` · `mcp/` | Assembles AGENTS.md + the Lila MCP server + the driver. `mcp/tools.ts` is the manager's entire capability (memory + subagent tools); `mcp/server.ts` is the loopback bearer-guarded streamable-HTTP server. |
| `src/manager/prompt.ts` | Splits the instructions: static persona/rules/host-facts/tools → `AGENTS.md`; volatile core memory + index → the per-turn context header. |
| `src/memory/` | `memfs.ts` (markdown backend over `/memories`), `fts.ts` (sqlite FTS5), `git.ts` (commit-per-write), `block.ts`. |
| `src/runtime/` | `eventQueue.ts`, `loop.ts` (one turn at a time), `snapshot.ts` (v4: thread id + queue), `telemetry.ts`. |
| `src/workers/` | `runner.ts` (`CodexRunner` over `@openai/codex-sdk`; every run a fresh thread), `orchestrator.ts` (single-shot ephemeral workers: start → one event → gone), `summarize.ts`. |
| `src/transport/` | `telegram.ts` (chunked Bot API client + `getUpdates`), `poller.ts` (outbound long-poll). |
| `provision/` | `AGENTS.md` (worker standing rules), `memory-bank/` templates (seeded into the app repo). |
| `test/` | Per-subsystem tests + the fake-driven **headline e2e**. |

## Tests — the best e2e we can run without deploying

Every external boundary is injectable, so the **real** runtime loop runs against fakes while memory
runs for real:

- **Manager backend** → `test/fakes/fakeManager.ts`, a scripted backend whose each "turn" acts
  directly against the **real** Lila MCP tool handlers (memory + orchestrator) and replies to the
  owner — so the whole runtime runs with everything real except the Codex thread itself. The real
  `ManagerDriver` is exercised separately in `test/driver.test.ts` against a scripted `ThreadEvent`
  stream (delivery, `NO_REPLY`, usage, context header, `local_image`, resume/reset, failure).
- **Codex** → `test/fakes/fakeCodex.ts`, an in-process `CodexRunner` (async runs, `AbortSignal`,
  early thread-id, `WORKER_FAIL`/`WAIT_FOR_ABORT`/`LONG_OUTPUT` sentinels).
- **Telegram** → `test/fakes/fakeTelegram.ts`, an in-process HTTP server: records sends/edits and
  serves `getUpdates` as a real long-poll (tests inject inbound updates via `pushUpdate`).
- **Memory + MCP** → the **real** `node:sqlite` FTS + a **real** tmp git repo; `test/mcp.test.ts`
  also boots the real Lila MCP HTTP server and asserts bearer-token gating + the MCP handshake.

`test/e2e.test.ts` is the headline: owner message → manager turn → `subagent_start` ×2 (parallel,
prompt-scoped) → workers complete → `worker_event`s → manager records a decision in memory and
narrates → Telegram; it asserts memory writes land in MemFS, the worker prompts carried their scopes,
the manager thread id is snapshotted, and a simulated **cold restart** loses no memory. Subsystem
suites cover memory, the manager driver, the MCP tools, workers, durability, config, and transport.

```bash
npm install
npm run typecheck
npm test           # runs with --experimental-sqlite (node:sqlite)
```

> The e2e never calls a real model — it validates *our* contracts (the loop, memory, MCP tools,
> durability, the driver's delivery/`NO_REPLY`/usage handling, the poll seam), not Codex's behavior.

## Status

Host-native and runnable: the manager runs on a plain VM via long-poll, and the opinionated app
runtime is a minimal **Rails 8 + PWA** scaffold (`lila-new-app`) in reload mode. **v0.3** cut the
manager brain off Claude Opus onto a long-lived **Codex thread** — manager and workers now share the
one ChatGPT subscription, with no metered plane (see `MIGRATION-CODEX.md`). See `MIGRATION.md` for
the earlier host-native migration. The runtime is deliberately thin — Rails 8 defaults plus PWA,
built-in auth, and a reserved `/_agent/*` path — so the agent builds *on top* rather than fighting a
heavy template.
