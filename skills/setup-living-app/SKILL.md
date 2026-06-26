---
name: setup-living-app
description: Stand up a Little Living Apps instance end to end with Docker Compose — choose and provision a fresh VPS, get SSH in, install Docker, create a Telegram bot, fill in .env, run bin/new-instance, do the one-time subscription login, optionally publish behind a domain, and verify the app and bot reply. Use when the user wants to set up, install, deploy, self-host, or get started with Little Living Apps.
---

# Set up a Little Living App

You are walking the user through standing up one Little Living Apps instance on a host they own. The
end state: one Docker Compose project with a Telegram manager container, an app container, persistent
volumes, and a bot the user can text to build and maintain one app.

**Hard rule — the host is the security boundary.** Workers run with `danger-full-access` and never
pause for approval. Provision a **fresh, disposable VPS** with nothing else on it. Never run this on
the user's laptop or a box with anyone else's data.

Four steps are human-only: buying the VPS, creating the Telegram bot, the one-time subscription
login, and DNS records.

## Quick Start

```bash
git clone https://github.com/zakkl13/little-living-apps.git && cd little-living-apps
cp .env.example .env
# edit .env: set TELEGRAM_BOT_TOKEN and ALLOWED_USER_IDS
bin/new-instance primary

docker compose --env-file .docker/primary.env exec manager codex login --device-auth
docker compose --env-file .docker/primary.env restart manager
docker compose --env-file .docker/primary.env logs -f manager
```

## Workflow

### 1. Provision a disposable VPS
- Confirm the user understands the box will be fully agent-controlled and must be throwaway.
- Pick a Linux VPS with Docker support, 2 vCPU, 4 GB RAM, and at least 25 GB disk.
- Have them add their SSH public key during creation and note the public IP and login user.

### 2. Get SSH access and Docker ready
- Confirm access: `ssh <user>@<ip> 'echo ok && uname -a'`.
- Install Docker Engine and the Compose plugin if needed.

### 3. Create the Telegram bot + owner ID
- Bot token: in Telegram, message **@BotFather** -> `/newbot` -> copy `TELEGRAM_BOT_TOKEN`.
- Owner ID: message **@userinfobot** -> copy the numeric `id` into `ALLOWED_USER_IDS`.
- Each instance needs its own bot because a bot cannot be long-polled twice.

### 4. Configure `.env`
- Clone the repo, copy `.env.example` to `.env`, and set `TELEGRAM_BOT_TOKEN` and
  `ALLOWED_USER_IDS`.
- Leave `AGENT_BACKEND` unset for Codex, or set `AGENT_BACKEND=claude` for Claude.
- Make sure pay-per-token API keys are absent: Codex refuses `OPENAI_API_KEY` / `CODEX_API_KEY`;
  Claude refuses `ANTHROPIC_API_KEY`.
- Optional: set `LILA_STACK=node-react` or another stack name. Default is `rails-pwa`.
- Optional: set `LILA_DOMAIN=app.example.com` after DNS points at the box.

### 5. Start the instance
- Run `bin/new-instance primary`.
- This writes `.docker/primary.env`, creates named volumes, builds the Docker image, and starts the
  `manager` and `app` services.
- The app service runs `lila-new-app` before serving, so an empty workspace volume is scaffolded
  automatically.

### 6. One-time subscription login
- Codex: `docker compose --env-file .docker/primary.env exec manager codex login --device-auth`.
- Claude: `docker compose --env-file .docker/primary.env exec manager claude setup-token`, then put
  the printed token into `.docker/primary.env` as `CLAUDE_CODE_OAUTH_TOKEN=<token>`.
- Restart the manager: `docker compose --env-file .docker/primary.env restart manager`.

### 7. Verify
- Tail logs: `docker compose --env-file .docker/primary.env logs -f manager`.
- Check the app: `curl -I http://127.0.0.1:3000/`.
- Have the user text the bot something like "build me a reading log." `/status` shows workers,
  backend, and memory state.

### 8. More instances

```bash
APP_PORT=3001 INSPECTOR_PORT=9091 TELEGRAM_BOT_TOKEN=<new-token> bin/new-instance cm
```

Each instance is a separate Compose project with its own manager, app, workspace, memory, state,
Codex home, ports, and bot.
