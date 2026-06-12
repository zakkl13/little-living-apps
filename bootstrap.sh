#!/usr/bin/env bash
# bootstrap.sh — run ONCE on a fresh Ubuntu 22.04+/Debian 12 host (e.g. an AWS EC2 instance):
#
#     git clone <this repo> && cd little-living-apps
#     cp .env.example .env && $EDITOR .env        # fill in tokens
#     sudo bash bootstrap.sh
#
# Installs the toolchain (mise -> Ruby + Node), the Codex CLI, builds the manager, creates the
# data dirs + workspace, and installs+enables the systemd service. Idempotent: safe to re-run.
#
# SECURITY MODEL: the disposable VM IS the boundary. Codex workers run with danger-full-access and
# the manager hands them full control of this box. Run this only on a host you would hand an agent.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

log() { printf '\033[1;34m[bootstrap]\033[0m %s\n' "$*"; }
die() { echo "ERROR: $*" >&2; exit 1; }

[[ $EUID -eq 0 ]] || die "Run with sudo (needs apt + systemd): sudo bash bootstrap.sh"

# The service runs as the human who invoked sudo (non-root), not as root. On EC2 that's 'ubuntu'.
SERVICE_USER="${SERVICE_USER:-${SUDO_USER:-}}"
[[ -n "$SERVICE_USER" && "$SERVICE_USER" != "root" ]] ||
  die "Could not determine a non-root service user. Re-run via 'sudo bash bootstrap.sh' from your normal account, or set SERVICE_USER=<name>."
USER_HOME="$(getent passwd "$SERVICE_USER" | cut -d: -f6)"
[[ -n "$USER_HOME" ]] || die "No home directory for user '$SERVICE_USER'."

# Run a command as the service user with a login shell (so HOME + PATH are correct for mise).
run_as() { sudo -u "$SERVICE_USER" -H bash -lc "$*"; }

# --- 1. Refuse the billing-flip footgun -------------------------------------------------------
# Either key silently flips Codex from the ChatGPT subscription to metered API billing.
for key in OPENAI_API_KEY CODEX_API_KEY; do
  [[ -z "${!key:-}" ]] || die "$key is set in the environment. Unset it (Codex must ride the ChatGPT subscription)."
  if [[ -f "$REPO_DIR/.env" ]] && grep -qE "^[[:space:]]*${key}=" "$REPO_DIR/.env"; then
    die "$key is set in .env. Remove it (Codex must ride the ChatGPT subscription)."
  fi
done

# --- 2. System packages (incl. Ruby build deps for mise's ruby-build) -------------------------
log "Installing system packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y \
  git curl ca-certificates build-essential \
  libssl-dev libyaml-dev libffi-dev zlib1g-dev libreadline-dev libgdbm-dev libncurses-dev

# --- 3. mise + pinned Ruby/Node (.mise.toml) --------------------------------------------------
MISE="$USER_HOME/.local/bin/mise"
if [[ ! -x "$MISE" ]]; then
  log "Installing mise for $SERVICE_USER"
  run_as "curl -fsSL https://mise.run | sh"
fi
log "mise: $("$MISE" --version 2>/dev/null || echo unknown)"
run_as "cd '$REPO_DIR' && '$MISE' trust && '$MISE' install"
# Also promote the pinned toolchain to the GLOBAL default. Without this, `mise exec -- npm/node`
# only resolves inside a dir that has a .mise.toml (i.e. the repo). The Playwright install (§4b, in
# /opt/lila/tooling) and Codex workers (which run in arbitrary app subdirs) have no local config, so
# they'd fail with "npm couldn't exec process". Keep these specs in sync with .mise.toml.
run_as "'$MISE' use -g ruby@3.3 node@22"

# --- 4. Codex CLI + build the manager ---------------------------------------------------------
log "Installing @openai/codex (under the mise-managed Node)"
run_as "cd '$REPO_DIR' && '$MISE' exec -- npm install -g @openai/codex"
log "Building the manager"
run_as "cd '$REPO_DIR' && '$MISE' exec -- npm ci && '$MISE' exec -- npm run build"

# --- 4b. Playwright + headless Chromium (workers self-validate: drive + screenshot the app) ----
# The `playwright` npm package goes into a FIXED, node-version-independent location
# ($PW_TOOLING/node_modules) — NOT `npm install -g`, whose prefix is tied to the active node
# version and silently orphans the install on the next node upgrade. The service unit exports
# NODE_PATH=$PW_TOOLING/node_modules, so workers (Codex children of the manager, inheriting its env)
# can `require("playwright")` in an interactive script AND `npx playwright …` resolves — from any
# cwd, with no network re-fetch. The browser binary lands once in the service user's
# ~/.cache/ms-playwright (shared host-wide). The OS shared-library deps need root (apt), so that step
# runs outside run_as and is non-fatal.
PW_TOOLING="/opt/lila/tooling"
log "Installing Playwright (-> $PW_TOOLING) + headless Chromium (worker self-validation)"
mkdir -p "$PW_TOOLING"
chown "$SERVICE_USER:$SERVICE_USER" "$PW_TOOLING"
run_as "cd '$PW_TOOLING' && { [ -f package.json ] || '$MISE' exec -- npm init -y >/dev/null; } && '$MISE' exec -- npm install playwright"
run_as "cd '$PW_TOOLING' && NODE_PATH='$PW_TOOLING/node_modules' '$MISE' exec -- npx playwright install chromium"
( cd "$PW_TOOLING" && HOME="$USER_HOME" NODE_PATH="$PW_TOOLING/node_modules" "$MISE" exec -- npx playwright install-deps chromium ) \
  || log "WARN: 'playwright install-deps' failed — chromium may need system libs; rerun bootstrap or run it by hand"
# Smoke-test the contract workers depend on: require("playwright") resolves purely via NODE_PATH.
run_as "NODE_PATH='$PW_TOOLING/node_modules' '$MISE' exec -- node -e 'require(\"playwright\"); console.log(\"playwright require() OK via NODE_PATH\")'" \
  || die "Playwright is not require()-resolvable via NODE_PATH=$PW_TOOLING/node_modules — worker self-validation would fail."

# --- 5. Data dirs + workspace (owned by the service user) -------------------------------------
WORKSPACE_DIR="${WORKSPACE_DIR:-/srv/app}"
MEMORY_DIR="${MEMORY_DIR:-/var/lib/lila/memory}"
STATE_DIR="${MANAGER_STATE_DIR:-/var/lib/lila/state}"
CODEX_HOME="${CODEX_HOME:-/var/lib/lila/codex}"
log "Creating $WORKSPACE_DIR, $MEMORY_DIR, $STATE_DIR, $CODEX_HOME"
mkdir -p "$WORKSPACE_DIR" "$MEMORY_DIR" "$STATE_DIR" "$CODEX_HOME"
chown -R "$SERVICE_USER:$SERVICE_USER" "$WORKSPACE_DIR" /var/lib/lila

# Init the workspace git repo (the agent's playground + rollback) and seed worker docs.
run_as "git -C '$WORKSPACE_DIR' rev-parse --git-dir >/dev/null 2>&1 || { git -C '$WORKSPACE_DIR' init -q && git -C '$WORKSPACE_DIR' config user.email 'lila@localhost' && git -C '$WORKSPACE_DIR' config user.name 'Little Living Apps'; }"
install -o "$SERVICE_USER" -g "$SERVICE_USER" -m 644 "$REPO_DIR/provision/AGENTS.md" "$WORKSPACE_DIR/AGENTS.md"
if [[ -d "$REPO_DIR/provision/memory-bank" && ! -d "$WORKSPACE_DIR/memory-bank" ]]; then
  cp -r "$REPO_DIR/provision/memory-bank" "$WORKSPACE_DIR/memory-bank"
  chown -R "$SERVICE_USER:$SERVICE_USER" "$WORKSPACE_DIR/memory-bank"
fi

# --- 6. Environment file for systemd ----------------------------------------------------------
mkdir -p /etc/lila
if [[ -f "$REPO_DIR/.env" ]]; then
  install -m 600 "$REPO_DIR/.env" /etc/lila/lila.env
  log "Installed /etc/lila/lila.env (from .env)"
else
  log "WARNING: no .env found — create /etc/lila/lila.env from .env.example before starting."
fi

# --- 7. systemd service -----------------------------------------------------------------------
log "Installing systemd unit"
sed -e "s|__USER__|$SERVICE_USER|g" -e "s|__REPO_DIR__|$REPO_DIR|g" -e "s|__MISE__|$MISE|g" \
  "$REPO_DIR/deploy/lila-manager.service" > /etc/systemd/system/lila-manager.service
systemctl daemon-reload
systemctl enable lila-manager.service

# Expose the app scaffolder on PATH so a worker (or you) can run `lila-new-app` to stand up the
# Rails 8 + PWA app the team builds. See bin/new-app.
cat > /usr/local/bin/lila-new-app <<EOF
#!/usr/bin/env bash
exec sudo bash "$REPO_DIR/bin/new-app" "\$@"
EOF
chmod +x /usr/local/bin/lila-new-app

# --- 8. Publish the app behind your domain (Caddy auto-HTTPS), if LILA_DOMAIN is set -----------
# Caddy is the TLS terminator that puts the app on your domain. It is NOT in the stock Ubuntu repos,
# so we add its official apt repo, then write /etc/caddy/Caddyfile with the domain baked in (the
# apt-managed caddy.service reads that file and has no env vars, so the domain must be literal).
LILA_DOMAIN="${LILA_DOMAIN:-$(grep -E '^LILA_DOMAIN=' "$REPO_DIR/.env" 2>/dev/null | cut -d= -f2- | tr -d '"'\''' )}"
if [[ -n "$LILA_DOMAIN" ]]; then
  log "Publishing the app on https://$LILA_DOMAIN (Caddy)"
  if ! command -v caddy >/dev/null 2>&1; then
    apt-get install -y debian-keyring debian-archive-keyring apt-transport-https curl gnupg
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list
    apt-get update -y && apt-get install -y caddy
  fi
  sed "s|{\$LILA_DOMAIN:localhost}|$LILA_DOMAIN|" "$REPO_DIR/deploy/Caddyfile" > /etc/caddy/Caddyfile
  systemctl enable caddy >/dev/null 2>&1 || true
  systemctl reload caddy 2>/dev/null || systemctl restart caddy
  log "Caddy serving https://$LILA_DOMAIN -> 127.0.0.1:3000 (cert issues on first request; needs 80/443 open + DNS at this host)."
else
  log "LILA_DOMAIN not set — skipping Caddy; the app stays private to the host (publish later by setting LILA_DOMAIN and re-running)."
fi

# --- 9. Codex auth (interactive, one-time) ----------------------------------------------------
if run_as "CODEX_HOME='$CODEX_HOME' '$MISE' exec -- codex login status" >/dev/null 2>&1; then
  log "Codex auth present — starting the service"
  systemctl restart lila-manager.service
  log "Done. Message your bot on Telegram to test the loop."
else
  cat <<EOF

────────────────────────────────────────────────────────────────────────────
Almost there. Authenticate Codex (ChatGPT subscription), then start the bot:

  sudo -u $SERVICE_USER -H CODEX_HOME=$CODEX_HOME $MISE exec -- codex login --device-auth
  sudo systemctl start lila-manager.service

Follow the logs with:  journalctl -u lila-manager -f
────────────────────────────────────────────────────────────────────────────
EOF
fi
