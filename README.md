# little-living-apps

Build useful, stateful little apps you never have to read the code for. You talk to a **Claude
manager** over Telegram; it orchestrates headless **OpenAI Codex** workers that build and maintain
**one app** on a Linux host you control. The manager plans, remembers, and delegates; the workers do
the concrete work in the app's repo. "Living" = the agent team keeps maintaining it, not a one-shot
generator.

Two billing planes: Anthropic (metered — the manager's brain) and your **ChatGPT subscription**
(the Codex workers — **no OpenAI API key**, by design).

> **Bring your own everything.** This is an open pattern, not a service: your host, your API keys,
> your Telegram bot. Not a money-making endeavor — adopt it, fork it, run it on a box you own.

```
 Telegram ──long-poll (outbound)──▶ EVENT QUEUE ──▶ manager turn (SERIALIZED)
  (owner)   getUpdates              owner_msg | worker_event      │
        ▲                                                         │ Anthropic Messages (Opus)
        │ reply text                                              │ tool loop
        │                    ┌── tools (the only hands) ──────────┘
        │                    │  memory  → MemFS (git markdown + sqlite FTS)
        │                    │  subagent_* → Codex workers (async, parallel, scoped)
        └────────────────────┤  the manager's plain text IS the reply (NO_REPLY = stay silent)
                             └── workers operate the box: build & maintain the app at $WORKSPACE_DIR
```

The bot talks to Telegram by **outbound long-poll** — it opens no inbound port and needs no public
URL or TLS, so it runs behind NAT, on a home box, or on a bare cloud VM. The manager has **no
shell/file/network tools**; that boundary is enforced by its tool surface (only `memory`,
`subagent_*`, `memory_search`/`recall_search`). Its plain assistant text is delivered straight to
Telegram; `NO_REPLY` lets it absorb an event silently (Hermes / Letta-v1 style). Workers have full
access under standing rules (`provision/AGENTS.md`). Everything survives a restart via per-turn
snapshots + a git-backed memory repo.

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
  instead of the ChatGPT subscription. The bot (and `bootstrap.sh`) refuse to start if either is set.

## Quickstart (fresh Ubuntu 22.04+/Debian 12 host)

```bash
git clone <this repo> && cd little-living-apps
cp .env.example .env && $EDITOR .env     # TELEGRAM_BOT_TOKEN, ALLOWED_USER_IDS, ANTHROPIC_API_KEY
sudo bash bootstrap.sh                    # mise -> Ruby+Node, Codex CLI, build, data dirs, systemd
# then, one-time, authenticate Codex on the ChatGPT subscription:
sudo -u <you> -H CODEX_HOME=/var/lib/lila/codex ~/.local/bin/mise exec -- codex login --device-auth
sudo systemctl start lila-manager        # bootstrap starts it automatically once auth is present
journalctl -u lila-manager -f            # watch it
```

Then message your bot. The manager delegates to workers and reports back; `/status` shows workers +
state; `/new` clears the working transcript (long-term memory is kept). `bootstrap.sh` is idempotent
— re-run it after pulling changes (then `sudo systemctl restart lila-manager`).

## Layout

| Path | What |
|---|---|
| `bootstrap.sh` | One-shot host setup: mise (Ruby+Node), Codex CLI, build, data dirs, systemd unit. |
| `deploy/lila-manager.service` | systemd unit template (`bootstrap.sh` fills it in). |
| `.mise.toml` | Pinned Ruby + Node versions. |
| `src/config.ts` | Env loading + validation; requires `ANTHROPIC_API_KEY`; **refuses to start if `OPENAI_API_KEY`/`CODEX_API_KEY` is set** (billing-flip guard). |
| `src/app.ts` | Composition root: wires memory + orchestrator + tool registry + loop + snapshots; `ingestTelegramUpdate`, `persist`/`restore`. |
| `src/manager/anthropic.ts` | `ManagerModel` seam + real `@anthropic-ai/sdk` wrapper (compaction + context-editing betas; compaction blocks round-trip verbatim). |
| `src/manager/manager.ts` | One serialized manager turn: event → tool loop → deliver. |
| `src/manager/prompt.ts` · `tools/` | System prompt (persona + rules + core memory + host facts) and the tool registry (memory, orchestration). |
| `src/memory/` | `memfs.ts` (`memory_20250818` backend over `/memories`), `fts.ts` (sqlite FTS5), `git.ts` (commit-per-write), `block.ts`. |
| `src/runtime/` | `eventQueue.ts`, `loop.ts` (one turn at a time), `snapshot.ts` (cold-restart recovery). |
| `src/workers/` | `runner.ts` (`CodexRunner` over `@openai/codex-sdk`), `orchestrator.ts` (async `subagent_*`; steer = abort+resume), `registry.ts`, `summarize.ts`. |
| `src/transport/` | `telegram.ts` (chunked Bot API client + `getUpdates`), `poller.ts` (outbound long-poll). |
| `provision/` | `AGENTS.md` (worker standing rules), `memory-bank/` templates (seeded into the app repo). |
| `test/` | Per-subsystem tests + the fake-driven **headline e2e**. |

## Tests — the best e2e we can run without deploying

Every external boundary is injectable, so the **real** runtime loop runs against fakes while memory
runs for real:

- **Anthropic** → `test/fakes/fakeAnthropic.ts`, a scripted `ManagerModel` returning predetermined
  `tool_use` / text / `compaction` blocks. Records every request (asserts the compaction round-trip).
- **Codex** → `test/fakes/fakeCodex.ts`, an in-process `CodexRunner` (async runs, `AbortSignal`,
  early thread-id, `WORKER_FAIL`/`WAIT_FOR_ABORT`/`LONG_OUTPUT` sentinels).
- **Telegram** → `test/fakes/fakeTelegram.ts`, an in-process HTTP server: records sends/edits and
  serves `getUpdates` as a real long-poll (tests inject inbound updates via `pushUpdate`).
- **Memory** → the **real** `node:sqlite` FTS + a **real** tmp git repo (high-fidelity).

`test/e2e.test.ts` is the headline: owner message → manager turn → `subagent_start` ×2 (parallel,
prompt-scoped) → workers complete → `worker_event`s → manager narrates as plain text → Telegram; it
asserts memory-tool writes land in MemFS, compaction blocks round-trip, and a simulated **cold
restart** restores memory + transcript losslessly. Subsystem suites cover memory, the manager loop,
workers, durability, config, and the long-poll transport.

```bash
npm install
npm run typecheck
npm test           # runs with --experimental-sqlite (node:sqlite)
```

> The e2e never calls a real model — it validates *our* contracts (loop, memory, compaction
> round-trip, durability, the poll seam), not Anthropic's beta behavior.

## Status

The opinionated app runtime (Rails 8: SQLite + Solid stack, auth, Hotwire, PWA, reload mode) is the
next milestone — see `MIGRATION.md`. Today the agent system runs on a host and can build whatever a
Codex worker can scaffold; the runtime template is what will make the apps it builds good-by-default.
