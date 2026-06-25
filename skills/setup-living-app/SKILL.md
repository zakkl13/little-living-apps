---
name: setup-living-app
description: Stand up a Little Living Apps instance end to end — choose and provision a fresh VPS, get SSH in, create a Telegram bot, fill in .env, run bootstrap.sh, explicitly start the primary app with bin/new-app, do the one-time subscription login (Codex or Claude), optionally publish behind a domain with HTTPS, and verify the app and bot reply. Use when the user wants to set up, install, deploy, self-host, or get started with Little Living Apps, stand up the Telegram app-builder bot, or provision a box for it.
---

# Set up a Little Living App

You are walking the user through standing up one Little Living Apps instance on a host they own. The
end state: a Telegram bot, running under systemd on a fresh VPS, that the user can text to build and
maintain one app, plus the primary app service running under `lila-app@primary`.

**Hard rule — the host is the security boundary.** Workers run with `danger-full-access` and never
pause for approval. Provision a **fresh, disposable VPS** with nothing else on it. Never run this on
the user's laptop or a box with anyone else's data. State this up front and do not proceed onto a
shared machine.

You drive the machine steps over SSH yourself (via the Bash tool). Four steps are human-only —
**buying the VPS, creating the Telegram bot, the one-time subscription login, and DNS records** —
because they happen in the user's browser. Give exact instructions for those and wait for the user to
confirm before continuing.

## Quick start

The whole flow on the remote box, once you have SSH and the two Telegram values:

```bash
git clone https://github.com/zakkl13/little-living-apps.git && cd little-living-apps
cp .env.example .env
# edit .env: set TELEGRAM_BOT_TOKEN and ALLOWED_USER_IDS (optionally LILA_STACK, LILA_DOMAIN)
sudo bash bootstrap.sh
sudo LILA_INSTANCE=primary bash bin/new-app
# one-time login (default Codex backend / ChatGPT subscription) — run the exact command
# bootstrap.sh just printed; it fills in the service user. It looks like:
sudo -u <service-user> -H CODEX_HOME=/var/lib/lila/codex ~<service-user>/.local/bin/mise exec -- codex login --device-auth
sudo systemctl start lila-manager@primary
journalctl -u lila-manager@primary -f
```

## Workflow

Work through these in order. Check each off before moving on.

### 1. Provision a disposable VPS (human)
- Confirm the user understands the box will be fully agent-controlled and must be throwaway.
- Help them pick a provider and size. Any provider that gives a public-IP VM running **Ubuntu
  22.04 LTS or newer (or Debian 12)** works (DigitalOcean, Hetzner, Vultr, Linode, AWS EC2, …). See
  [REFERENCE.md](REFERENCE.md) for sizing — it must build Ruby + Node and install headless Chromium,
  so **≥ 2 GB RAM (4 GB comfortable), 2 vCPU, ≥ 25 GB disk**.
- Have them add their SSH public key during creation and note the public IP and login user (often
  `ubuntu` on EC2, `root` elsewhere — they should be able to `sudo`).

### 2. Get SSH access (you, then verify)
- Confirm you can reach the box: `ssh <user>@<ip> 'echo ok && lsb_release -ds'`. If it prints the
  Ubuntu/Debian version, you're in. From here you run the remote steps over SSH.

### 3. Create the Telegram bot + owner ID (human)
- **Bot token:** in Telegram, message **@BotFather** → `/newbot` → follow prompts → copy the
  `TELEGRAM_BOT_TOKEN` it gives. Each instance needs its **own** bot (a bot can't be long-polled
  twice).
- **Owner ID:** message **@userinfobot** → copy the numeric `id`. That is `ALLOWED_USER_IDS` (the
  only user the bot will answer; comma-separate for more).
- Collect both values from the user before continuing. Treat the token as a secret — it goes only
  into `.env` on the box.

### 4. Choose the backend (decide with the user)
- Default is **Codex** (ChatGPT subscription). The alternative is **Claude** (Claude Pro/Max),
  selected by `AGENT_BACKEND=claude` in `.env`.
- **Billing guard — do not trip it:** the matching pay-per-token key must be **unset** or bootstrap
  refuses to start. Codex → `OPENAI_API_KEY` / `CODEX_API_KEY`; Claude → `ANTHROPIC_API_KEY`. Make
  sure none is set in the environment or `.env`.

### 5. Clone, configure .env (you)
- On the box: `git clone https://github.com/zakkl13/little-living-apps.git && cd little-living-apps`
  then `cp .env.example .env`.
- Set `TELEGRAM_BOT_TOKEN` and `ALLOWED_USER_IDS` in `.env`. Set `AGENT_BACKEND=claude` only if the
  user chose Claude. For a public domain now, also set `LILA_DOMAIN` (see step 8); otherwise leave it
  unset and the app stays private to the box.
- **Pick the app stack (optional).** The *stack* decides the kind of app the agent builds. It ships
  with `rails-pwa` (Rails 8 + PWA — the default) and `node-react` (a zero-build Node + React PWA);
  more can be dropped in under `stacks/`. To use something other than the default, set
  `LILA_STACK=node-react` in `.env` before bootstrap. Leave it unset for Rails. See
  [REFERENCE.md](REFERENCE.md) for how stacks work and how to add your own.

### 6. Run bootstrap (you)
- `sudo bash bootstrap.sh`. It's idempotent. It installs mise → Ruby + Node, both agent CLIs,
  Playwright, and the Rust toolchain; **builds the `lila` binary** to `/opt/lila/bin` (a cargo
  release build — a few minutes; bootstrap adds swap on small boxes so it won't OOM); creates data
  dirs; installs the systemd unit; and (if `LILA_DOMAIN` is set) Caddy.
- Bootstrap prepares the host and manager service, but it does **not** scaffold or start the primary
  app. The app is a separate explicit step after the host is ready.
- If it dies on the billing-guard or a missing `.env` value, fix the value and re-run. Common
  failures (incl. an OOM during the cargo build) are in [REFERENCE.md](REFERENCE.md).

### 7. Start the primary app (you)
- Run `sudo LILA_INSTANCE=primary bash bin/new-app`. This scaffolds the selected stack into
  `/srv/primary`, installs the `lila-app@primary` systemd unit, and starts/restarts the app on the
  configured `APP_PORT` (default `3000`).
- This step is idempotent. Re-run it after stack scaffold changes or if `lila-app@primary` is missing.

### 8. One-time subscription login (human, then you start the manager)
- bootstrap prints the exact command. For the default Codex backend it's:
  `sudo -u <user> -H CODEX_HOME=/var/lib/lila/codex ~/.local/bin/mise exec -- codex login --device-auth`
  — run it and have the user complete the device-auth in their browser. For the Claude backend, log
  the box into the Claude subscription with `claude setup-token` (or export
  `CLAUDE_CODE_OAUTH_TOKEN`) instead.
- Then start it: `sudo systemctl start lila-manager@primary`.

### 9. (Optional) Publish behind a domain (human DNS + you)
- Point an **A record** for the domain at the box's public IP, and make sure ports **80 and 443** are
  open in the provider firewall / security group.
- Set `LILA_DOMAIN=app.example.com` in `.env` and re-run `sudo bash bootstrap.sh`. It installs Caddy
  and issues HTTPS on first request. Skip this to keep the app private to the box.

### 10. Verify (you + user)
- Confirm the app service is active: `systemctl status lila-app@primary`. If `LILA_DOMAIN` is set,
  `curl -I https://<domain>/up` should return `200`.
- Tail the logs: `journalctl -u lila-manager@primary -f` — you should see it boot and start
  long-polling.
- Have the user **text the bot** something like *"build me a reading log."* A reply (and worker
  activity in the logs) means the loop is live. `/status` in the chat shows workers + backend.
- If there's no reply: confirm the user's Telegram ID is in `ALLOWED_USER_IDS`, the token is right,
  and the service is `active` (`systemctl status lila-manager@primary`). More in
  [REFERENCE.md](REFERENCE.md).

## Done

The user now has a living app: text the bot to build, text again to change it. Long-term memory
survives restarts. To run a second app, repeat with a **new** bot token via `bin/new-instance` (see
the repo README).
