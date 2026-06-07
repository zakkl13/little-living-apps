# Spec — Sprite-Hosted Codex Telegram Bot (v0.1)

> **Goal of this iteration:** A single git repo that can (1) provision a Fly Sprite, (2) install OpenAI Codex CLI on it, (3) run a small server that lets a Telegram bot drive a Codex session **on my behalf** using my ChatGPT subscription.
>
> This is the thinnest possible proof of the "agent lives on a Sprite, I talk to it from my phone" loop. Multi-app orchestration, the owned PWA, sharing, and self-healing are explicitly **out of scope** here.

> **Why Codex, not Claude Code:** Claude Code's subscription path is moving `claude -p` / Agent SDK usage onto a separate metered "Agent SDK credit" (starting June 15, 2026), which makes the always-on-bot use case a cost liability. Codex `exec` runs on the ChatGPT subscription with **no separate API billing** — it draws from normal plan usage limits, which OpenAI explicitly supports for scripted/automated use. That's the whole reason for the switch. (Verify plan limits below — "included" still means "rate-limited by plan," not "infinite.")

---

## 1. Scope

**In scope (v0.1)**
- A provisioning script that creates a Sprite and bootstraps it (Node/binary, Codex, the bot service, secrets).
- A long-running bot process on the Sprite bridging Telegram ⇄ headless Codex (`codex exec`).
- One Codex session per Telegram chat, persisted to disk so it survives hibernation.
- Creator-only access via a Telegram user-ID allowlist.
- A single persistent working directory (a git repo) that Codex operates in.
- A baked-in **AGENTS.md** that teaches the agent how to build durable apps on a Sprite (see §11).

**Out of scope (later iterations)**
- The owned mobile/desktop chat UI (Telegram is the stand-in client for now).
- Multiple apps / per-app Sprites / a deploy seam.
- Self-healing, liveness monitoring, cron/scheduling beyond the minimal heartbeat.
- Response streaming, rich diffs, approval UX.
- Any multi-user / sharing model.

---

## 2. Architecture

```
[ Telegram app on phone ]
          │  (Bot API)
          ▼
[ Telegram bot server ]  ──────────────  runs as a Sprite "Service"
   - auth: user-ID allowlist            (auto-restarts on wake; survives hibernation)
   - session store (SQLite/JSON on disk)
   - spawns:  codex exec [resume <id>] --json ...
          │
          ▼
[ Codex CLI (headless) ]  ── authenticated via ChatGPT subscription (device-code or injected auth.json)
   - cwd = /workspace/project  (persistent volume, a git repo, contains AGENTS.md)
   - reads/writes files, runs commands, etc.
```

Everything lives on **one Sprite**. The Sprite's 100 GB persistent filesystem is Codex's working directory, the home of the session store, and where Codex's auth (`$CODEX_HOME`) lives. The Sprite hibernates when idle; how it gets woken is the central design decision in §6.

---

## 3. Prerequisites

| Thing | Notes |
|---|---|
| Fly Sprites account + API token | `SPRITES_TOKEN`. Sprites API/CLI at `docs.sprites.dev` (new product — confirm exact CLI command names there). |
| Telegram bot token | Create via `@BotFather`, gives `TELEGRAM_BOT_TOKEN`. |
| My Telegram user ID | For the allowlist. From `@userinfobot` or the first `getUpdates` payload. |
| ChatGPT plan (Plus/Pro/etc.) | Codex is included across Free/Go/Plus/Pro/Business/Edu/Enterprise; usage limits vary by plan. |
| Codex CLI | `npm install -g @openai/codex` (or the native binary / Homebrew). |
| Node.js (for installer) + git | Installed during provisioning. |

---

## 4. Authentication design (the part that changed most)

**Authenticate Codex to my ChatGPT subscription on the Sprite — no API key.** Two viable headless paths; browser OAuth alone won't work on a headless box (the localhost callback fails), same as before.

**Path A (recommended): device-code login.**
1. First enable it: ChatGPT → Settings → Security → allow **device code login** (personal account). Workspace accounts need an admin to enable it under Workspace Permissions. It's opt-in because device-code flows carry more social-engineering risk.
2. On the Sprite, run `codex login --device-auth`. It prints a URL (`auth.openai.com/device`) and a short code; enter the code in a browser on my phone/laptop. Credentials persist under `$CODEX_HOME` (default `~/.codex`).

**Path B (alternative): inject `auth.json`.**
1. `codex login` on my laptop (browser works there) → writes auth to `~/.codex/auth.json`.
2. Inject into the Sprite via the `CODEX_AUTH_JSON` env var (or mount the file as a secret). **Never bake it into an image.** This mirrors the token-injection approach and is handy for re-provisioning.

**Verify auth:** `codex login status` exits 0 when credentials are present — use it as the boot probe.

**Gotchas to bake into the design:**
- **`OPENAI_API_KEY` switches you to API billing.** If it's set (or you use `--with-api-key`), Codex bills the API instead of your subscription. **Do not set `OPENAI_API_KEY`** on the Sprite; keep auth on the ChatGPT path. Confirm with `codex login status`.
- **Subscription means rate-limited, not unlimited.** Usage limits vary by plan and are shared with your normal ChatGPT/Codex usage. The bot's throughput is bounded by the plan — but it is *not* a separate metered credit, which is the win over Claude Code. (Pro tiers get a faster, lower-quota model variant; check current details at `chatgpt.com/pricing`.)
- **Auth lives on disk** under `$CODEX_HOME`, so it persists across hibernation — but tokens can still expire; surface re-auth needs clearly rather than failing silently.

---

## 5. Repo structure

```
sprite-codex-bot/
├── README.md
├── SPEC.md                  # this file
├── .env.example             # documents required env vars (no secrets committed)
├── AGENTS.md                # default agent-manager instructions (see §11) — deployed to /workspace/project
├── provision/
│   ├── provision.sh         # create Sprite, push code, install deps, register service
│   └── bootstrap.sh         # runs ON the Sprite: install Codex + git, set cwd repo, place AGENTS.md
├── src/
│   ├── bot.(ts|py)          # Telegram bridge: auth, routing, session store, codex spawn
│   ├── codex.(ts|py)        # thin wrapper around `codex exec ... --json`
│   ├── sessions.(ts|py)     # disk-backed chat_id -> codex session_id store
│   └── config.(ts|py)       # env var loading + validation
└── workspace/               # (created on the Sprite) the git repo Codex operates in
```

Language: TypeScript or Python. The Sprites client has Go/TS today (Python "coming soon" as of launch), so **TypeScript** keeps one runtime across provisioning + bot. Codex itself is language-agnostic (spawned as a subprocess), so pick by Telegram-library comfort.

---

## 6. Central design decision: how does the Sprite get woken?

| | Long-polling (`getUpdates`) | Webhook (Telegram → Sprite URL) |
|---|---|---|
| Build effort | Lowest | Moderate |
| Hibernation | **Defeats it** — the poll loop keeps the Sprite awake ~24/7 | **Preserves it** — an incoming update hits the Sprite's public URL and wakes it on demand |
| Cost | Always-on-ish | Pay-per-message |
| Security | Outbound only | Public URL exposed — must verify Telegram's `X-Telegram-Bot-Api-Secret-Token` header and use an unguessable path |
| Cold start | None | First message after sleep pays wake latency (fine for a personal bot) |

**Recommendation:** Ship **long-polling** for v0.1 to get the loop working fastest, then switch to **webhook** to reclaim hibernation. Either way, register the bot as a Sprite **Service** (not a TTY/console process) so it auto-restarts on wake.

---

## 7. Bot behavior (interfaces, not implementation)

**On each inbound Telegram message:**
1. **Authorize.** Reject any `from.id` not in `ALLOWED_USER_IDS` (v0.1 = just my ID).
2. **Resolve session.** Look up `session_id` for this `chat_id` in the disk-backed store. None → first turn.
3. **Acknowledge.** Send a "working…" message immediately (Codex runs can be long).
4. **Run Codex** in `/workspace/project`:
   - First turn: `codex exec --json "<text>"`
   - Subsequent: `codex exec resume "<session_id>" --json "<text>"`
     (`codex exec resume --last "<text>"` continues the most recent session in the cwd, but prefer the explicit session ID to stay safe under multiple chats.)
   - Codex streams JSONL events to stdout (`--json`) and the final agent message to stdout (or to a file via `-o/--output-last-message`). Parse the **session id** from the event stream on the first turn and store it; capture the final message as the reply. *(Confirm the exact session-id field name in the current `--json` event schema.)*
5. **Persist** the `session_id` back to the store.
6. **Reply.** Send the final message back to Telegram, **chunked to ≤4096 chars**.

**Commands:**
- `/new` — drop the stored session for this chat (start fresh).
- `/status` — report active auth (`codex login status`), current `session_id`, and rough usage notes.

**Sandbox / approval posture (Codex specifics):**
- In `exec` mode there's no TTY, so `on-request` approvals are auto-downgraded to `never`. You choose the sandbox level explicitly:
  - `--sandbox workspace-write` — edit within the working dir (prefer this; `--full-auto` is a deprecated alias).
  - `--sandbox danger-full-access` — also allows networked commands (needed if the agent must hit the internet, e.g. installs, scraping).
  - `--dangerously-bypass-approvals-and-sandbox` / `--yolo` — no approvals, no sandbox.
- **Sprite-specific note:** Codex's Linux sandbox uses Landlock/seccomp, which may be unavailable inside a microVM and can cause sandbox-init errors. Since the **Sprite is itself the isolation boundary**, running with `--sandbox danger-full-access` (or `--yolo`) and treating the Sprite as the blast-radius container is defensible here — but **test which sandbox modes actually initialize on a Sprite** during bring-up.
- Codex refuses to run outside a git repo by default; pass `--skip-git-repo-check` only if the working dir isn't a repo (it should be — see §8).

---

## 8. State & persistence rules (Sprite-specific)

- **RAM does not survive hibernation.** Session store, working repo, Codex's `$CODEX_HOME` auth, and any usage log **must** be on the Sprite's disk. Never hold session state only in memory.
- **Working dir is a real git repo.** `cd /workspace/project && git init` during bootstrap — both Codex's playground and your free rollback (`git revert`). Codex also writes per-session rollout logs under `$CODEX_HOME/sessions/…` if you ever need the full trajectory.
- **Optional safety net:** snapshot the Sprite filesystem before risky runs to roll back the whole environment, not just the repo.

---

## 9. Configuration (`.env`)

| Var | Purpose |
|---|---|
| `CODEX_AUTH_JSON` *(Path B only)* | Injected ChatGPT auth blob; or rely on on-Sprite `codex login --device-auth` (Path A). |
| `CODEX_HOME` | Default `~/.codex` — keep it on the persistent volume. |
| `TELEGRAM_BOT_TOKEN` | From `@BotFather`. |
| `ALLOWED_USER_IDS` | Comma-separated Telegram user IDs permitted to use the bot. |
| `WORKSPACE_DIR` | Default `/workspace/project`. |
| `SESSION_STORE_PATH` | Default `/workspace/.sessions.sqlite` (on disk!). |
| `CODEX_SANDBOX_MODE` | `workspace-write` \| `danger-full-access` (chosen per §7). |
| `TELEGRAM_MODE` | `polling` (v0.1) or `webhook`. |
| `TELEGRAM_WEBHOOK_SECRET` | Required if `webhook`; verify against inbound header. |
| `SPRITES_TOKEN` | Provisioning only (used by `provision.sh`, **not** the bot). |

Explicitly **must not** be set on the Sprite: `OPENAI_API_KEY` (would flip Codex to API billing — see §4).

---

## 10. Bring-up sequence (the "get started" path)

1. Enable device-code login in ChatGPT settings (Path A) **or** run `codex login` on laptop and grab `auth.json` (Path B).
2. Fill `.env` locally (bot token, my user ID, sandbox mode).
3. `provision/provision.sh`:
   1. Create a Sprite via the Sprites API/CLI; capture name/URL.
   2. Copy the repo onto the Sprite (`sprite` file push / git clone), including `AGENTS.md`.
   3. Run `bootstrap.sh`: install git + `@openai/codex`, `mkdir -p /workspace/project && git init`, drop `AGENTS.md` at the repo root, set `CODEX_HOME` on the volume.
   4. Authenticate: `codex login --device-auth` (Path A) or inject `CODEX_AUTH_JSON` (Path B); assert `codex login status` exits 0.
   5. Probe: `codex exec --json "respond with OK"` and confirm it returns on the **subscription** (not API) path.
   6. Register the bot as a Sprite **Service** with env vars injected.
4. Message the bot from Telegram → expect a reply produced by Codex in `/workspace/project`.
5. Send a follow-up → confirm `codex exec resume` continues the same session.
6. Close everything, wait for hibernation, message again → confirm the Service wakes and the session store + auth survived.

---

## 11. Default agent-manager instructions (`AGENTS.md`)

This ships at `/workspace/project/AGENTS.md` — Codex's project-instructions file, loaded automatically into every session. It encodes the Sprite-correct patterns so every app the agent builds is durable by default. *(Confirm AGENTS.md precedence/merging — global `~/.codex` vs project — in current Codex docs; a global copy can hold the platform rules and the project copy the per-app rules.)*

> ### AGENTS.md (canonical contents)
>
> **Your runtime environment**
> - You run on a Fly **Sprite**: a persistent Linux microVM that **hibernates after ~30s idle** and **wakes on demand** (on an HTTP request to its URL, or an external ping). Treat sleep/wake as constant and normal.
> - You have a **100 GB persistent filesystem** and outbound internet. CPU/RAM exist only while awake.
>
> **The non-negotiable rules**
> 1. **RAM is wiped on hibernation. The filesystem is not.** Never store anything that must survive in memory. Persist all durable state to disk under `/workspace` (SQLite or files). Assume the process can be killed and restarted at any moment.
> 2. **Any long-running process MUST be a Sprite Service.** Web servers, workers, and listeners must be registered as Services so they auto-restart on wake. **Never** leave a server running only inside a console/TTY session — it dies the moment the Sprite sleeps. *(Use the Sprite Services mechanism; verify exact command in docs.sprites.dev.)*
> 3. **Web servers must listen on the Sprite's routed HTTP port** (e.g. 8080) so an incoming request wakes the Sprite and the Service answers it.
> 4. **There is no built-in cron.** For anything scheduled (periodic scraping, digests, cleanups), do **not** rely on `cron` or `systemd` timers firing while asleep. Expose a job endpoint and rely on the external heartbeat scheduler to wake the Sprite and trigger it. Make every scheduled job **idempotent** — it may fire late, twice, or after a long sleep.
> 5. **Write for restart-tolerance.** Re-entrant init, reconnect logic, resume-from-disk. The box sleeps and wakes constantly; code that assumes continuous uptime will break.
>
> **Working discipline**
> 6. **Always work inside the git repo** at `/workspace/project`. Commit in small, logical units with clear messages — this is the rollback mechanism. Before destructive or risky changes, prefer creating a checkpoint (and note that a full-filesystem snapshot is available as a coarser undo).
> 7. **Leave breadcrumbs.** When you finish a task, update a short `STATUS.md` (what exists, what's running as a Service, what each scheduled job does, known issues). After hibernation there is no memory of this session except what's on disk — write for the next cold start.
>
> **Security & safety**
> 8. **Treat the Sprite as the isolation boundary.** Stay within `/workspace`. Don't attempt to exfiltrate credentials or escape the box. Minimize destructive operations.
> 9. **Never hardcode or commit secrets.** Read credentials from environment variables / secret files only. Don't echo secrets into logs or `STATUS.md`.
> 10. **Default to private.** Only make a port or URL public when the task explicitly requires it (e.g. a webhook). For any public endpoint, require a shared secret and verify it on every request.
>
> **Resource awareness**
> 11. You share one Sprite (100 GB disk; CPU/RAM only while awake). Keep dependencies lean, clean up build artifacts, and avoid unbounded disk growth (rotate logs, cap caches).

---

## 12. Acceptance criteria

- [ ] A non-allowlisted Telegram user gets rejected.
- [ ] First message creates a Codex session in `/workspace/project` and replies with its output.
- [ ] A follow-up resumes the **same** session via `codex exec resume` (verified by stable session id).
- [ ] `/new` starts a fresh session.
- [ ] Session continuity + auth survive a full hibernate→wake cycle (state was on disk).
- [ ] `codex login status` confirms **subscription** auth, not an API key.
- [ ] The chosen `--sandbox` mode actually initializes on the Sprite.
- [ ] Responses >4096 chars are chunked, not truncated.
- [ ] An app the agent builds follows AGENTS.md (runs as a Service, persists to disk, survives a sleep/wake cycle).

---

## 13. Key risks & gotchas (consolidated)

1. **Auth billing flip:** a stray `OPENAI_API_KEY` silently moves you to API pricing. Probe with `codex login status`.
2. **Plan rate limits:** subscription usage is shared/limited per plan — fine for personal use, but bound the bot accordingly. (Not a separate metered credit — the reason we left Claude Code.)
3. **Device-code is opt-in:** must be enabled in ChatGPT settings before `codex login --device-auth` works.
4. **Sandbox-in-microVM:** Landlock/seccomp may not initialize on a Sprite; test sandbox modes, fall back to `danger-full-access` treating the Sprite as the boundary.
5. **Sprite Services, not TTY:** console processes die on sleep. Register Services.
6. **RAM is ephemeral across hibernation:** all state on disk, including `$CODEX_HOME`.
7. **No native cron:** scheduled work needs the external heartbeat; jobs must be idempotent.
8. **Public URL exposure (webhook mode):** verify Telegram's secret-token header; unguessable path.
9. **New product churn:** Sprites (Jan 2026) and Codex `exec`/device-auth are evolving — confirm CLI/API/flag names against `docs.sprites.dev` and `developers.openai.com/codex`.

---

## 14. Open decisions to make before coding

- **Language/runtime:** TypeScript (shares runtime with the TS Sprites client) vs Python. Recommend TS.
- **Auth path:** device-code (A) vs injected `auth.json` (B). A is cleaner for a long-lived Sprite; B is better for repeatable provisioning.
- **Polling vs webhook for v0.1:** polling first, webhook immediately after.
- **Sandbox posture:** `workspace-write` vs `danger-full-access` (the content machine will likely need network → probably the latter, pending the sandbox-init test).
- **One Sprite forever vs recreate per provision:** for v0.1, one long-lived Sprite is simplest.
