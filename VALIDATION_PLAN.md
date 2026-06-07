# Live Sprite Validation Plan

Bottom-up bring-up of the never-deployed v0.2 bot on a **real** Fly Sprite. One layer at a
time; a green layer is a precondition for the next. Defects are logged in `DEFECTS.md` and
reflected back into code/DESIGN.md/memory — not fixed inline mid-layer.

## Rules
- One variable at a time, bottom-up.
- Capture, don't fix: log each defect (layer, symptom, expected vs actual, suspected file).
  Batch-fix between layers, then re-run from the failed layer.
- Cheapest falsifying probe first (raw `sprite exec` shell checks before booting the Service).

## Phases
1. **Provision the bare Sprite** — install+auth CLI; `create`, push repo, `exec`, public URL.
   Validate the real CLI surface vs provision.sh's guessed shims.
2. **Runtime deps on the box** — `npm ci && build`; Node ≥22; `node --experimental-sqlite`
   actually loads `node:sqlite` on the Sprite's Node build.
3. **Codex auth + execution** — `codex login --device-auth`; raw probe under `danger-full-access`
   in the microVM (sandbox-free exec where Landlock/seccomp may not init).
4. **Sprite keep-alive hold** — exercise hold.ts against the real Tasks-API socket
   (`/.sprite/api.sock`): acquire / heartbeat / release; held Sprite keeps a long TCP stream.
5. **Boot as a Service + webhook** — config guards fire; `setWebhook(PUBLIC_URL)`; secret-token
   verification; inbound POST reaches the handler and wakes the box.
6. **Anthropic LIVE** — model ids resolve; each beta in isolation (compaction round-trip,
   memory tool CRUD → MemFS, context editing). Largest fakes-vs-reality gap.
7. **End-to-end single turn** — Telegram msg → manager → one Codex worker → worker_event →
   narrate → notify_user.
8. **Concurrency** — two parallel scoped workers; steer (abort+resume); cancel; overlap serialize.
9. **Durability / cold-wake** — hibernate mid-state; wake via webhook; restore() rehydrates
   transcript + queue + workers; memory survives.
10. **Memory persistence sweep** — commit-per-write + FTS survive a real wake cycle.

## Credentials status
- `SPRITES_TOKEN` ✅ (Phases 1–4)
- `CLAUDE_TOKEN` ⚠️ present but ≠ `ANTHROPIC_API_KEY` (config.ts requires an Anthropic API key)
- MISSING for Phase 5+: `TELEGRAM_BOT_TOKEN`, `ALLOWED_USER_IDS`, `TELEGRAM_WEBHOOK_SECRET`,
  `ANTHROPIC_API_KEY`
