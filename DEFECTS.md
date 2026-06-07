# Defects found during live Sprite validation

Format: `[ID] (phase) severity — symptom → suspected fix location`. Status: OPEN / FIXED.

## Phase 1 — Provision

- **[D1] (1) HIGH — OPEN.** `provision.sh` calls `sprite push <local> <name:dest>`, but no
  `sprite push` command exists in the live CLI. File transfer is `sprite exec --file <src:dest>`
  (single file, repeatable). The whole repo-push strategy needs replacing (git clone, tar over
  exec, or repeated --file). → `provision/provision.sh` `sprite_push()`.
- **[D2] (1) MED — OPEN.** `sprite exec` / `sprite url` use a `-s <sprite>` flag, not the
  positional `<name> -- <cmd>` form the script assumes. → `provision/provision.sh`
  `sprite_run()`, `sprite_url()`.
- **[D3] (1) HIGH — OPEN.** No documented `sprite service create`. Services appear tied to
  `sprite create` ("services keep sprites alive and auto-restart on boot"). The Service
  registration block needs rework against the real mechanism. → `provision/provision.sh`
  `sprite_service()`.
- **[D4] (1) LOW — OPEN.** Script never runs `sprite auth setup --token "$SPRITES_TOKEN"`;
  assumes the CLI is already authed. → `provision/provision.sh` (add auth step).
- **[D5] (1) LOW — OPEN.** `sprite url` is deprecated (use `sprite info` to view /
  `sprite update --url-auth` to change). → `provision/provision.sh` `sprite_url()`.
- **[D6] (1) HIGH — OPEN.** New Sprite URL auth defaults to `sprite` (private) — Telegram
  cannot POST the webhook to it. Must set `sprite update --url-auth public` (or equivalent)
  before setWebhook. Not handled anywhere. → `provision/provision.sh`.
- **[D7] (1) LOW — OPEN.** README/.env example show URL like `https://my-sprite.fly.dev`;
  real host is `https://<name>-<rand>.sprites.app`. → `README.md`, `.env.example`.

## Phase 3 — Codex auth + execution

- **[D8] (3) MED — OPEN (blast radius reduced).** The Sprite image ships a stale `codex 0.118.0`
  at `~/.local/bin/codex` that SHADOWS PATH. 0.118.0's default `gpt-5.3-codex` is **sunset on
  ChatGPT subscriptions**. BUT the bot's workers run via `@openai/codex-sdk@0.137`, which drives
  its own vendored binary `@openai/codex-linux-x64/vendor/.../bin/codex` = **codex-cli 0.137.0**
  (verified) → default `gpt-5.5` works. So the bot is unaffected. Only bare-`codex` callers break:
  the boot-probe `loginStatus` shells out to bare `"codex"` (runner.ts:147) — version-agnostic for
  auth so it still works, but relies on `~/.local/bin` being on the service PATH. Low-risk; consider
  pinning. → `src/workers/runner.ts:147` (optional), provisioning probe (D9).
- **[D9] (3) MED — OPEN.** `provision.sh` Codex probe (line ~64) and `bootstrap.sh` call bare
  `codex`, which resolves to the shadowed 0.118.0 and pins a sunset model. Probe must use the
  0.137 binary and the DEFAULT model (no `-c model=`). → `provision/provision.sh`,
  `provision/bootstrap.sh`.
- **[D10] (3) LOW — OPEN.** Codex warns "could not find system bubblewrap on PATH; using
  vendored bubblewrap." Harmless under `danger-full-access` (no real sandbox), but
  `workspace-write` mode would depend on the vendored copy. Consider `apt-get install -y
  bubblewrap` in bootstrap. → `provision/bootstrap.sh`.
- **DECISION (user):** Let Codex use its DEFAULT model (currently `gpt-5.5`) — do not pin a
  model name. Validated working on 0.137.0.
- **Phase 3 PASS:** `codex login` (ChatGPT) OK; `auth.json` on persistent volume; default model
  `gpt-5.5` returns PROBE_OK under `danger-full-access` in the microVM.

## Phase 4 — Sprite keep-alive hold

- **Phase 4 PASS (no defect):** `hold.ts` matches the live Tasks API exactly. Socket
  `/.sprite/api.sock`; `PUT /v1/tasks/codex-turn {expire:"5m"}`→200 (upsert avoids POST's 409);
  `GET /v1/tasks` lists; `DELETE`→204. Docs note max task lifetime 1h (our 5m+60s heartbeat is
  fine). The off-Sprite no-op degrade path is also correct.

## Phase 5 — Service + webhook

- **[D3 RESOLVED in practice]** Services are managed on-box via `/.sprite/bin/sprite-env services
  create <name> --cmd <bin> --args <csv> --env <KEY=val,..> --dir <path> --http-port <port>`.
  NOT `sprite service create`. Validated: service `codex-bot` runs `bash -lc run.sh`, http-port
  8080, status running. provision.sh `sprite_service()` must be rewritten to this.
- **[D6 RESOLVED]** URL auth must be `public` for Telegram: `sprite url update --auth public`.
  Done; getWebhookInfo shows the URL registered, 0 pending, no last_error.
- **Phase 5 PASS:** boot clean (config guards OK, Codex auth via SDK OK, memory subsystem inits),
  `setWebhook` registered to `https://codex-bot-buaqy.sprites.app/tg/<secret>`, service persisted
  + auto-restart. Launch via `run.sh` sourcing `.env.runtime` avoids `--env` comma pitfalls.

## Phase 6 — Anthropic LIVE

- **Models/betas/types all current & accepted live:** `claude-opus-4-8`, `claude-haiku-4-5`,
  betas `compact-2026-01-12` + `context-management-2025-06-27`, edits `compact_20260112` +
  `clear_tool_uses_20250919`, memory tool `memory_20250818`. Opus call with the full bot param
  shape returned `stop_reason=tool_use` — no 400. Manager path validated.
- **[D11] (6) HIGH — OPEN.** The summarizer uses the **Haiku** utility model through the same
  `createAnthropicModel` wrapper, which **unconditionally** sets
  `context_management.edits=[compact_20260112, clear_tool_uses_20250919]`. Haiku 4.5 returns 400:
  "does not support the 'compact_20260112' context management strategy." Verified live: Haiku
  accepts `clear_tool_uses_20250919` and plain calls, but NOT `compact_20260112`. So any
  worker-output summarization (summarize.ts, triggered when a worker returns >2000 chars) fails.
  Fix: don't attach compaction to utility/Haiku calls — make `context_management`/`betas`
  per-request or model-tier-aware (compaction is Opus/Sonnet-only). The one-shot summarizer needs
  neither compaction nor context editing. → `src/manager/anthropic.ts:96-108`, `src/workers/summarize.ts`.

## Phase 7 — End-to-end single turn

- **Phase 7 PASS (real, verified on disk):** Telegram owner msg → manager turn (Opus, live) →
  Codex worker (gpt-5.5, `danger-full-access`) created `/workspace/project/hello.txt`="sprite
  works" AND committed it (`3edce0f Add hello marker file`); manager memory git repo +
  `memory.fts.sqlite` written (commit-per-write); per-turn snapshot `manager.json` (8.7KB) saved;
  user received the reply. Full spine works on the real Sprite.
- **[D12] (7) MED — OPEN (observability).** Default `LOG_LEVEL=info`, but the entire turn
  lifecycle — webhook ingest, manager tool loop, worker spawn/settle (`worker_event`),
  `notify_user`, keep-alive acquire/release — logs at `debug` (logger.ts:8 threshold). A complete
  successful turn produced **0 log lines** beyond startup. In prod a failed turn would leave no
  trace. Fix: promote key lifecycle events (turn start/end, worker start/settle, errors) to
  `info`, or document setting `LOG_LEVEL=debug`. → `src/**` log call sites / `logger.ts`.

## Phase 7 — follow-up (user-reported)

- **[D13] (7) HIGH — OPEN (user-facing duplicate messages).** The manager turn has TWO delivery
  paths to Telegram and both fire: (1) the `notify_user` tool (intended channel), and (2) the
  end-turn text fallback in `runManagerTurn` (`manager.ts:79-82`) which delivers any final text
  block via `deliver()`. Opus 4.8 narrates a closing summary after tool use by default, so the
  completion lands twice — verified in the snapshot transcript: `notify_user`="Done ✅ hello.txt
  created…" AND final text="Done — hello.txt is created…". The `deliver` comment calls itself a
  "fallback for when notify_user wasn't used," but it is unconditional. Fix: make `deliver` a true
  fallback — track whether any `notify_user` ran during the turn and only deliver end-turn text if
  none did (and/or add a prompt line: notify_user is your only user channel; don't also write a
  closing summary). → `src/manager/manager.ts` (track notify in the tool loop), `src/manager/prompt.ts`.

## Phase 8 — Concurrency

- **Phase 8 PASS:** two workers (w1,w2) spawned concurrently (14:06:17.16 / .21), keep-alive hold
  refcounted across both (one acquire/release pair), both files written (`a.txt`=A, `b.txt`=B).
  Prompt-only coordination worked — manager gave non-overlapping scopes, workers stayed in lane.
  Async/event-driven: manager waited for BOTH `worker_event`s before a single "Both done" notify.
- **[D13 reconfirmed]** Same duplicate on this turn: NOTIFY "Both done ✅…" + final text "Both
  files are created…".
- **OBSERVATION (low):** parallel workers did NOT commit a.txt/b.txt (hello.txt worker did).
  Worker commit behavior is gpt-5.5's discretion (objective didn't require it); the shared-tree
  no-conflict path wasn't stress-tested for a commit race. Not a bot defect; note for future
  lease/commit-coordination work.
- **OBSERVATION (low):** manager's `memory str_replace` on `/memories/system/workers.md` missed
  (old_str didn't match) and it fell back to `create`. Minor memory-bookkeeping friction.

## Phase 9 / 10 — Durability & memory (covered incidentally)

- **Phase 9 PASS (via service restart):** restarting the service triggered `app.restore()` →
  "Restored manager state from snapshot {messages:24, pending:0, workers:1}", and the conversation
  continued correctly into Phase 8 (manager retained context across the restart). Real cold-wake
  path proven.
- **Phase 10 PASS:** manager memory git repo (commit-per-write objects) + `memory.fts.sqlite`
  persisted on the volume across the restart; transcript snapshot `manager.json` grew 24→50 msgs.

### Phase 1 working recipe (validated)
- Auth: `sprite auth setup --token "$SPRITES_TOKEN"`
- Create: `sprite create codex-bot --skip-console`
- Push repo: tar locally (exclude node_modules/.git/dist/.env), then
  `sprite exec -s codex-bot --file repo.tar.gz:/tmp/repo.tar.gz -- bash -lc 'tar xzf ... -C /workspace/sprite-codex-bot'`
- Exec: `sprite exec -s <name> -- <cmd>`   ·   URL: `sprite info` / `sprite url`
- Sprite: Linux 6.12-fly microVM, x86_64, user `sprite`, home `/home/sprite`.
