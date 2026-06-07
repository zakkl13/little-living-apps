# sprite-codex-bot

A Telegram bot where you talk to a **Claude manager** that orchestrates headless **OpenAI Codex**
workers on a **Fly Sprite**. The manager plans, remembers, and delegates; workers do the concrete
work in the workspace. Two billing planes: Anthropic (metered, the manager) and your **ChatGPT
subscription** (the workers — no OpenAI API key).

This is **v0.2** from [`DESIGN.md`](./DESIGN.md). v0.1 (a thin Telegram→Codex relay, see
[`SPEC.md`](./SPEC.md)) is superseded: the relay is replaced by a manager runtime + memory +
worker-coordination layer. The Sprite keep-alive, Codex SDK integration, and Telegram transport
carry over.

```
 Telegram ──webhook POST──▶ EVENT QUEUE ──▶ manager turn (SERIALIZED)
  (owner)                   owner_msg | worker_event | tick      │
        ▲                                                        │ Anthropic Messages (Opus)
        │ reply text                                             │ tool loop
        │                    ┌── tools (the only hands) ─────────┘
        │                    │  memory  → MemFS (git markdown + sqlite FTS)
        │                    │  subagent_* → Codex workers (async, parallel, scoped)
        └────────────────────┤  the manager's plain text IS the reply (NO_REPLY = stay silent)
                             └── workers run in the shared tree under prompt-assigned scopes
```

The manager has **no shell/file/network tools** — that boundary is enforced by its tool surface
(only `memory`, `subagent_*`, `memory_search`/`recall_search`). It talks to the owner with no comms
tool at all: its plain assistant text is delivered straight to Telegram, and the `NO_REPLY` sentinel
lets it absorb an event silently (Hermes / Letta-v1 style). Workers have full
access under standing rules (`provision/AGENTS.md`). Everything survives Sprite cold-wake via
per-turn snapshots + a git-backed memory repo.

## Layout

| Path | What |
|---|---|
| `src/config.ts` | Env loading + validation; requires `ANTHROPIC_API_KEY`; **refuses to start if `OPENAI_API_KEY`/`CODEX_API_KEY` is set** (billing-flip guard). |
| `src/app.ts` | Composition root: wires memory + orchestrator + tool registry + loop + snapshots; `ingestTelegramUpdate`, `persist`/`restore`. |
| `src/manager/anthropic.ts` | `ManagerModel` seam + real `@anthropic-ai/sdk` wrapper (compaction + context-editing betas; compaction blocks round-trip verbatim). |
| `src/manager/manager.ts` | One serialized manager turn: event → tool loop → deliver. |
| `src/manager/prompt.ts` · `tools/` | System prompt (persona + rules + core memory) and the tool registry (memory, orchestration). |
| `src/memory/` | `memfs.ts` (the `memory_20250818` backend over `/memories`), `fts.ts` (sqlite FTS5), `git.ts` (commit-per-write changelog), `block.ts`. |
| `src/runtime/` | `eventQueue.ts`, `loop.ts` (one turn at a time), `snapshot.ts` (cold-wake), `hold.ts` (Sprite keep-alive). |
| `src/workers/` | `runner.ts` (`CodexRunner` over `@openai/codex-sdk`), `orchestrator.ts` (async `subagent_*`; steer = abort+resume), `registry.ts`, `summarize.ts`. |
| `src/transport/` | `telegram.ts` (chunked Bot API client), `webhook.ts` (`node:http`; secret-verified; enqueues). |
| `provision/` | `AGENTS.md` (worker rules: memory-bank + scope discipline), `memory-bank/` templates, `provision.sh`/`bootstrap.sh`. |
| `test/` | Per-subsystem tests + the fake-driven **headline e2e**. |

## Tests — the best e2e we can run without deploying

Every external boundary is injectable, so the **real** runtime loop runs against fakes while
memory runs for real:

- **Anthropic** → `test/fakes/fakeAnthropic.ts`, a scripted `ManagerModel` returning predetermined
  `tool_use` / text / `compaction` blocks. Records every request so we can assert the
  compaction round-trip.
- **Codex** → `test/fakes/fakeCodex.ts`, an in-process `CodexRunner` (async runs, `AbortSignal`,
  early thread-id, `WORKER_FAIL`/`WAIT_FOR_ABORT`/`LONG_OUTPUT` sentinels).
- **Telegram** → `test/fakes/fakeTelegram.ts`, an in-process HTTP server recording sends/edits.
- **Sprite** → a counting hold; off-Sprite the real hold no-ops.
- **Memory** → the **real** `node:sqlite` FTS + a **real** tmp git repo (high-fidelity).

`test/e2e.test.ts` is the headline (DESIGN §13): owner message → manager turn → `subagent_start`
×2 (parallel, prompt-scoped) → workers complete → `worker_event`s → manager narrates as plain
text → Telegram; it asserts memory-tool writes land in MemFS, compaction blocks round-trip, and a
simulated **cold wake** restores memory + transcript losslessly. Subsystem suites cover memory,
the manager loop, workers, durability, config, and transport.

```bash
npm install
npm run typecheck
npm test           # runs with --experimental-sqlite (node:sqlite)
```

> Note: the e2e never calls a real model — it validates *our* contracts (loop, memory, compaction
> round-trip, durability), not Anthropic's beta behavior. Live beta validation is a deploy-time
> follow-up.

## Real bring-up

1. Get a Codex subscription login (`codex login` → `CODEX_HOME/auth.json`) and an Anthropic API key.
2. Copy `.env.example` → `.env`; fill `TELEGRAM_BOT_TOKEN`, `ALLOWED_USER_IDS`,
   `TELEGRAM_WEBHOOK_SECRET`, `ANTHROPIC_API_KEY` (and `SPRITES_TOKEN` for provisioning).
3. Run `provision/provision.sh` — creates the Sprite, pushes the repo, runs `bootstrap.sh`
   (installs Codex + git, inits the workspace, places `provision/AGENTS.md` + `memory-bank/`
   templates, sets `CODEX_HOME`), and registers the bot as a Sprite **Service** with `PUBLIC_URL`
   set so it calls `setWebhook` on boot.
4. Message your bot. The manager delegates to workers and reports back; `/status` shows workers +
   state; `/new` clears the working transcript (long-term memory is kept).

> **The Sprites CLI is new** — command names in `provision.sh` are isolated in `sprite_*` shell
> functions; confirm against [docs.sprites.dev](https://docs.sprites.dev).

## Safety notes

- **Never set `OPENAI_API_KEY` or `CODEX_API_KEY`** on the Sprite — either flips Codex to metered
  API billing. The bot refuses to boot if either is present; the runner stays on the ChatGPT
  subscription via `CODEX_HOME`.
- The manager's capability boundary is its tool list — there is no bash/read/write/web tool on the
  raw Messages API unless we add one, so "no hands" is airtight by construction.
- Default worker sandbox is **`danger-full-access`** (with `approvalPolicy: "never"`): the Sprite
  is the isolation boundary. Switch via `CODEX_SANDBOX_MODE`.
- The webhook path embeds the secret and every POST is verified against the secret-token header.
