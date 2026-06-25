# setup-living-app — reference

Detailed guidance the main SKILL.md links to. Pull these in only when a step needs them.

## VPS sizing and provider notes

bootstrap.sh compiles Ruby via `ruby-build`, installs Node 22 + Playwright + headless Chromium
(workers screenshot the app to self-validate), and **builds the `lila` binary from source with
cargo** (Rust, thin-LTO — memory-heavy). That sets a real floor:

- **RAM:** 2 GB minimum, **4 GB comfortable**. 1 GB boxes OOM. On boxes under 3 GB, bootstrap adds a
  4 GB swapfile automatically so the cargo release build doesn't OOM.
- **vCPU:** 2 recommended (the Ruby build and the cargo release build are the slow parts — the cargo
  build alone takes a few minutes).
- **Disk:** ≥ 25 GB (toolchain + Chromium + the app's git history grow over time).
- **OS:** Ubuntu 22.04 LTS / 24.04 LTS or Debian 12. Not Alpine (glibc/Chromium deps), not anything
  older.

Any provider with a public-IP VM works — DigitalOcean, Hetzner Cloud, Vultr, Linode/Akamai, AWS EC2,
GCP, Azure. There is no provider lock-in; the box just needs apt + systemd + a public IP. Cheapest
viable tier is usually the ~$12–24/mo 2 vCPU / 4 GB instance.

Provisioning is something the **user** does in the provider's console (it costs money and needs their
payment method) — you cannot buy it for them. Give them the spec above and the exact OS image to
pick, then take over once SSH works.

## Choosing an app stack

The **stack** decides the *kind of app* the agent builds and maintains — its toolchain, scaffold,
serve command, and the prompts the workers run under. It's a data-driven plugin: a directory under
`stacks/<name>/` (a `stack.toml` plus a scaffold script and two prompt fragments), with no Rust code
or recompile needed to add one.

Ships with two:

- **`rails-pwa`** (default) — "Rails 8 + PWA". SQLite + the Solid stack + Hotwire + PWA stubs. The
  most batteries-included option; good when you want a full server-rendered app with auth.
- **`node-react`** — "Node + React (PWA)". A zero-build Node + React PWA. Lighter toolchain; good
  when the user wants a JS/React-flavored app.

Pick **per instance** with `LILA_STACK` (env var, or the line in `.env`; default `rails-pwa`):

```bash
# in .env, before bootstrap:
LILA_STACK=node-react
```

The choice is locked into that instance's env file at provision time, so the same value drives the
scaffold, the systemd serve unit, the app toolchain, the worker/manager prompts, and the eval
fixture. Different instances on the same box can run different stacks (`bin/new-instance` inherits the
primary's stack unless you override `LILA_STACK` for the new one).

**Adding your own:** copy an existing `stacks/<name>/` directory, edit its `stack.toml` + `scaffold.sh`
+ the two prompt fragments, and point `LILA_STACK` at the new name — no framework changes. (The design
system is framework-universal and independent of the stack: every app draws a design regardless of
which stack it runs.)

## Firewall / network

- The bot itself needs **no inbound ports** — it talks to Telegram by outbound long-poll. Default
  egress is enough; nothing inbound is required to just run the bot.
- Inbound **80 + 443** are needed **only** if publishing behind a domain (`LILA_DOMAIN`), so Caddy
  can solve the ACME challenge and serve HTTPS. Open them in the provider's firewall/security group,
  not just `ufw`.
- SSH (22) for you to drive setup.

## The billing guard (most common bootstrap failure)

bootstrap refuses to start if the active backend's pay-per-token key is set, because that silently
flips it from the subscription to metered API billing. This is intentional, not a bug.

- Codex backend (default): `OPENAI_API_KEY` and `CODEX_API_KEY` must be **unset** — in the shell
  env *and* absent from `.env`.
- Claude backend (`AGENT_BACKEND=claude`): `ANTHROPIC_API_KEY` must be **unset**.

If bootstrap dies with `<KEY> is set` — `unset` it in the shell, remove the line from `.env`, and
re-run. Check both: `env | grep -E 'OPENAI_API_KEY|CODEX_API_KEY|ANTHROPIC_API_KEY'`.

## Other bootstrap failures

- **`Run with sudo`** — invoke as `sudo bash bootstrap.sh` from the normal login user, not as root
  directly (it needs `$SUDO_USER` to pick the non-root service user). If you must, pass
  `SERVICE_USER=<name>`.
- **`playwright install-deps failed`** — Chromium's system libs didn't install; non-fatal but worker
  screenshots will fail. Re-run bootstrap, or run `npx playwright install-deps chromium` as the
  service user.
- **Ruby build is slow / hangs**, or **the cargo build is killed (OOM)** — usually under-provisioned
  RAM. bootstrap adds a 4 GB swapfile on boxes under 3 GB, but if it still OOMs, resize up (see
  sizing) and re-run; bootstrap is idempotent (it skips work already done).
- **No `.env`** — bootstrap warns and writes an empty `/etc/lila/primary.env`; create `.env` from
  `.env.example` first, then re-run.
- **Domain returns 502 / app is not listening** — bootstrap configures the host and proxy, but it
  does not scaffold or start the primary app. Run `sudo LILA_INSTANCE=primary bash bin/new-app`,
  then check `systemctl status lila-app@primary`.

## Auth notes

- **Codex (default):** the one-time `codex login --device-auth` stores auth under
  `CODEX_HOME=/var/lib/lila/codex` on the box, so it survives reboots. The user completes the device
  flow in their browser.
- **Claude:** run `claude setup-token`, complete the browser auth, then copy the printed one-year
  token into the instance env as `CLAUDE_CODE_OAUTH_TOKEN=<token>`. Subscription terms note: don't
  *offer* this to other people without Anthropic sign-off — single-owner personal use only.
- Bootstrap installs both agent CLIs (`codex` and `claude`) regardless of the initially selected
  backend, so later `/backend` swaps should not need a package install. If a legacy host says the
  Claude CLI is missing, pull this version and re-run `sudo bash bootstrap.sh`.
- A `/backend codex|claude` message in the chat hot-swaps the backend later; it starts a fresh
  manager thread but keeps long-term memory.

## The bot doesn't reply

1. `systemctl status lila-manager@primary` — is it `active (running)`? If it crash-loops, read
   `journalctl -u lila-manager@primary -e`.
2. Is the user's numeric Telegram ID exactly in `ALLOWED_USER_IDS`? Non-allowlisted users get
   silently refused — that's by design.
3. Is the token the one from *this* bot, and is no other process long-polling the same bot? A bot can
   only be long-polled from one place.
4. After editing `/etc/lila/primary.env` or `.env`, restart: `sudo systemctl restart
   lila-manager@primary`.

## Running more than one app

One manager → one app. For a second app, create a **new** bot via @BotFather and run, on the same
box:

```bash
sudo LILA_DOMAIN=cm.example.com APP_PORT=3001 INSPECTOR_PORT=9091 \
     TELEGRAM_BOT_TOKEN=<new-bot-token> \
     bash bin/new-instance cm
```

Each instance gets its own systemd unit (`lila-manager@<name>`), workspace, ports, and domain.
