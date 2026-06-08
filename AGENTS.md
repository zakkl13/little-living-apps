# AGENTS.md — working on this repo

This is the **factory**: the manager runtime that builds and maintains little living apps. It is
human-grade, reviewed code. (The apps it *builds* are agent-owned and never-read — different rules,
see `provision/AGENTS.md`.)

## Working rules
- **No backwards compatibility.** When changing direction, RIP OUT the prior code entirely — no
  compat shims, no dead branches, no legacy naming kept "just in case."
- **Work directly on `main`.** Do not create feature branches for this project — commit straight to
  main, in small logical units with clear messages.
- **Keep the seam discipline.** Every external boundary (Anthropic, Codex, Telegram) is injected, so
  the real runtime runs against fakes in tests. Don't reach around a seam; add/extend one.
- **Tests must pass.** `npm run typecheck && npm test` (the suite runs with `--experimental-sqlite`).

## Non-negotiable guards (do not weaken)
- The bot **refuses to start if `OPENAI_API_KEY` or `CODEX_API_KEY` is set** — either flips Codex to
  metered API billing instead of the ChatGPT subscription. Keep this guard in `config.ts` and
  `bootstrap.sh`.
- **The host is the security boundary.** Codex runs `danger-full-access` + `approvalPolicy: "never"`
  on a disposable VM; the manager is single-owner (Telegram allowlist); the app is private until
  deliberately exposed. Don't add an inbound port for the bot (transport is outbound long-poll).

## Orientation
- `README.md` — what this is + the security model + quickstart.
- `DESIGN.md` — architecture (manager loop, memory, workers, durability).
- `MIGRATION.md` — the host-native plan and what was deleted; the Rails 8 runtime is the next phase.
- `DEFECTS.md` — open/resolved defects.
