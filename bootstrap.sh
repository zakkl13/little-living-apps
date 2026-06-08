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

# --- 4. Codex CLI + build the manager ---------------------------------------------------------
log "Installing @openai/codex (under the mise-managed Node)"
run_as "cd '$REPO_DIR' && '$MISE' exec -- npm install -g @openai/codex"
log "Building the manager"
run_as "cd '$REPO_DIR' && '$MISE' exec -- npm ci && '$MISE' exec -- npm run build"

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

# --- 8. Codex auth (interactive, one-time) ----------------------------------------------------
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
