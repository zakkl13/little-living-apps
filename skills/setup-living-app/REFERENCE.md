# setup-living-app — reference

Detailed guidance the main `SKILL.md` links to. Pull this in only when a step needs it.

## VPS Sizing

The Docker image builds the Rust `lila` binary, includes Ruby/Node tooling, installs both agent CLIs,
and installs Playwright + Chromium for worker self-validation.

- **RAM:** 4 GB comfortable. 2 GB can work with swap but image builds may be slow.
- **vCPU:** 2 recommended.
- **Disk:** at least 25 GB.
- **OS:** any current Linux distribution that runs Docker Engine and the Compose plugin.

## Stacks

The stack decides the kind of app the agent builds and maintains: toolchain, scaffold, serve command,
and prompt fragments. Pick per instance with `LILA_STACK` in `.env` before running `bin/new-instance`.

- `rails-pwa` is the default Rails 8 + PWA stack.
- `node-react` is a zero-build Node + React PWA.

To add a stack, copy an existing `stacks/<name>/`, edit `stack.toml`, `scaffold.sh`, `worker.md`,
and `manager.md`, then set `LILA_STACK=<name>`.

## Network

- The bot itself needs no inbound port; it talks to Telegram by outbound long-poll.
- The app is published by Compose port binding, defaulting to `127.0.0.1:$APP_PORT`.
- Open 80/443 only when using a reverse proxy or the Caddy Compose profile for a domain.

## Billing Guard

The manager refuses to start if the active backend's pay-per-token key is set, because that would
move the backend off subscription auth and onto metered billing.

- Codex backend: unset `OPENAI_API_KEY` and `CODEX_API_KEY`.
- Claude backend: unset `ANTHROPIC_API_KEY`.

## Auth

- Codex auth lives in the `codex-home` named volume via `CODEX_HOME=/var/lib/lila/codex`.
- Claude auth is stored as `CLAUDE_CODE_OAUTH_TOKEN` in the instance env file.
- Both CLI binaries are installed in the Docker image.

## Troubleshooting

- Check containers: `docker compose --env-file .docker/<name>.env ps`.
- Tail logs: `docker compose --env-file .docker/<name>.env logs -f manager app`.
- Restart manager after env changes: `docker compose --env-file .docker/<name>.env restart manager`.
- Restart app after structural changes: `docker restart <name>-app`.
- If the bot does not reply, confirm `ALLOWED_USER_IDS`, bot token, backend auth, and that no other
  process is long-polling the same bot token.
