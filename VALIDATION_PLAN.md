# Host-Native Bring-Up / Smoke Plan

Bottom-up validation of the bot on a **real** disposable Linux host (trial target: a small AWS EC2
Ubuntu instance). One layer at a time; a green layer is a precondition for the next. Defects go in
`DEFECTS.md`.

> The original plan validated the Fly Sprite substrate (Sprite CLI, single-port routing, keep-alive
> Tasks API, webhook). That substrate was removed — see `MIGRATION.md`. This plan replaces it.

## Rules
- One variable at a time, bottom-up. Cheapest falsifying probe first.
- Capture, don't fix: log each defect (layer, symptom, expected vs actual, suspected file). Batch-fix
  between layers, then re-run from the failed layer.

## Phases
1. **Bootstrap a bare host** — `sudo bash bootstrap.sh` on fresh Ubuntu 22.04+/Debian 12: billing
   guard fires; apt deps install; mise brings up pinned Ruby+Node (`.mise.toml`); the Codex CLI
   installs; `npm ci && npm run build` succeeds; data dirs + workspace git repo created; worker
   `AGENTS.md`/`memory-bank` seeded; systemd unit installed + enabled.
2. **Runtime deps** — `node --experimental-sqlite` loads `node:sqlite` on the host's Node;
   `ruby --version` is the pinned Ruby under mise.
3. **Codex auth** — `codex login --device-auth` on the ChatGPT subscription; `codex login status` OK;
   `CODEX_HOME` persists on the VM disk. Raw probe under `danger-full-access`.
4. **Service up (long-poll)** — `systemctl start lila-manager`; config guards pass; the manager
   `deleteWebhook`s then long-polls `getUpdates`; **no inbound port is opened** (verify with `ss
   -tlnp`). `journalctl -u lila-manager -f` is clean.
5. **Anthropic LIVE** — model ids resolve; betas in isolation (compaction round-trip, memory tool
   CRUD → MemFS, context editing). Largest fakes-vs-reality gap.
6. **End-to-end single turn** — Telegram owner msg → manager turn → one Codex worker scaffolds a
   trivial app under `$WORKSPACE_DIR` → `worker_event` → manager narrates → reply lands once.
7. **Concurrency** — two parallel scoped workers; steer (abort+resume); cancel; overlap serializes.
8. **Durability / cold restart** — `systemctl restart lila-manager` mid-state; `restore()` rehydrates
   transcript + queue + workers; git memory + FTS survive.

## Credentials needed
- `TELEGRAM_BOT_TOKEN`, `ALLOWED_USER_IDS`, `ANTHROPIC_API_KEY` (in `.env` → `/etc/lila/lila.env`).
- Codex: interactive `codex login --device-auth` (ChatGPT subscription). **No** `OPENAI_API_KEY`.
