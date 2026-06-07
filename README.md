# sprite-codex-bot

A Telegram bot that drives a headless **OpenAI Codex** session running on a **Fly Sprite**, on
your behalf, using your **ChatGPT subscription** (no API key, no separate metered billing).

This is the v0.1 from [`SPEC.md`](./SPEC.md): the thinnest proof of "an agent lives on a Sprite,
I talk to it from my phone." One Codex session per Telegram chat, persisted to disk so it
survives Sprite hibernation. Webhook transport. Creator-only via a user-ID allowlist.

```
Telegram ──webhook POST──▶ bot (Sprite Service, :8080)
                              │ authorize · resolve thread · post "Working…"
                              │ @openai/codex-sdk: resumeThread(id) | startThread()
                              │ thread.runStreamed(prompt) → live progress edits
                              ▼
                           final reply (chunked ≤4096) ──▶ Telegram sendMessage
```

## Layout

| Path | What |
|---|---|
| `src/config.ts` | Env loading + validation; **refuses to start if `OPENAI_API_KEY`/`CODEX_API_KEY` is set** (billing-flip guard). |
| `src/codex.ts` | `CodexRunner` over **`@openai/codex-sdk`**: `startThread`/`resumeThread` + `runStreamed`, mapping events to live progress. |
| `src/sessions.ts` | Disk-backed `chat_id → thread_id` JSON store with atomic writes. |
| `src/telegram.ts` | Tiny fetch-based Bot API client; `sendMessage` (chunked ≤4096) + `editMessageText` for the live status. |
| `src/webhook.ts` | `node:http` server; verifies secret path + `X-Telegram-Bot-Api-Secret-Token`. |
| `src/handler.ts` | Per-update logic: authorize → resolve thread → stream progress → reply; `/new`, `/status`. |
| `src/index.ts` | Entrypoint wiring it together; runs as a Sprite Service. |
| `provision/` | `provision.sh` (laptop) + `bootstrap.sh` (on-Sprite). |
| `AGENTS.md` | Default agent-manager instructions deployed to `/workspace/project`. |
| `test/` | Unit tests + the fake-driven **end-to-end** test. |

## The end-to-end test (no real Sprite / Telegram / Codex)

The whole point of the harness is that every external boundary is injectable, so the **real**
bot loop runs against fakes:

- **Telegram** → `test/fakes/fakeTelegram.ts`, an in-process HTTP server that records every
  `sendMessage` and `editMessageText`. The bot points at it via `TELEGRAM_API_BASE_URL`.
- **Codex** → `test/fakes/fakeCodex.ts`, an in-process implementation of the `CodexRunner`
  interface (no subprocess). It emits progress notes via `onProgress` and echoes the resumed
  thread id so we can prove continuity. Prompt sentinels (`LONG_OUTPUT`, `AUTH_FAILURE`) drive
  the edge cases. The real runner wraps `@openai/codex-sdk`; tests inject the fake at that seam.
- **Sprite** → not needed; the bot is just a local process. Hibernate→wake is simulated by
  killing the bot and re-reading the on-disk thread store.

`test/e2e.test.ts` posts fake Telegram updates at the webhook endpoint and asserts the full
loop, covering most of the SPEC §12 acceptance criteria: auth rejection, first-turn thread
creation, **streamed progress edits**, stable-id resume, `/new`, `/status`, >4096-char chunking,
auth-failure surfacing, webhook secret rejection, and survival across a restart.

```bash
npm install
npm run typecheck
npm test
```

## Real bring-up (SPEC §10)

1. **Enable device-code login** in ChatGPT → Settings → Security (Path A), _or_ run
   `codex login` on your laptop and grab `~/.codex/auth.json` (Path B).
2. Copy `.env.example` → `.env`; fill `TELEGRAM_BOT_TOKEN`, `ALLOWED_USER_IDS`,
   `TELEGRAM_WEBHOOK_SECRET`, and `SPRITES_TOKEN`.
3. Run `provision/provision.sh`. It creates the Sprite, pushes the repo, runs `bootstrap.sh`
   (installs Codex + git, inits `/workspace/project`, places `AGENTS.md`, sets `CODEX_HOME`),
   probes auth, and registers the bot as a Sprite **Service** with `PUBLIC_URL` set so it calls
   `setWebhook` on boot.
4. Finish auth on the Sprite if needed: `CODEX_HOME=/workspace/.codex codex login --device-auth`.
   The SDK rides this same cached login (it reads `CODEX_HOME/auth.json`).
5. Message your bot. Follow-ups resume the same thread; `/new` starts fresh; `/status` shows
   auth + thread + sandbox.

> **The Sprites CLI is new** — the command names in `provision.sh` are isolated in `sprite_*`
> shell functions and must be confirmed against [docs.sprites.dev](https://docs.sprites.dev).
> Note the SDK may ship its own bundled `codex` binary; both it and the CLI read `CODEX_HOME`,
> so subscription auth works either way. Set `CODEX_BIN` to pin a specific binary.

## Safety notes

- **Never set `OPENAI_API_KEY` or `CODEX_API_KEY`** on the Sprite — either flips Codex to
  metered API billing. The bot refuses to boot if either is present, and the runner omits
  `apiKey` and strips both from the CLI env so the SDK stays on the ChatGPT subscription.
- Default sandbox is **`danger-full-access`** (paired with `approvalPolicy: "never"`): the Sprite
  is the isolation boundary, and Landlock/seccomp may not initialize in a microVM, so no sandbox
  is initialized at all. Switch via `CODEX_SANDBOX_MODE` for `workspace-write` / `read-only`.
- The webhook path embeds the secret and every POST is verified against the secret-token header.
