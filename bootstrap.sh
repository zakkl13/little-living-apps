#!/usr/bin/env bash
# bootstrap.sh — run ONCE on a fresh Ubuntu 22.04+/Debian 12 host (e.g. an AWS EC2 instance):
#
#     git clone <this repo> && cd little-living-apps
#     cp .env.example .env && $EDITOR .env        # fill in tokens
#     sudo bash bootstrap.sh
#
# Installs the toolchain (mise -> Node + the active stack's app language, both agent CLIs,
# Playwright), the Rust toolchain, BUILDS
# the self-contained `lila` binary to /opt/lila/bin, creates the data dirs + workspace, and
# installs+enables the systemd service. Idempotent: safe to re-run.
#
# SECURITY MODEL: the disposable VM IS the boundary. Workers run with danger-full-access and the
# manager hands them full control of this box. Run this only on a host you would hand an agent.
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

# Run a command as the service user with a login shell (so HOME + PATH are correct for mise/cargo).
run_as() { sudo -u "$SERVICE_USER" -H bash -lc "$*"; }

# --- 1. Refuse the billing-flip footgun -------------------------------------------------------
# The active backend's billing-flip key silently flips it from its subscription to metered API
# billing. The backend is AGENT_BACKEND (codex default; claude = the Claude CLI), read from the
# environment or .env. (Codex rides the ChatGPT subscription via `codex login`; Claude rides the
# Claude Pro/Max subscription via `claude setup-token` / CLAUDE_CODE_OAUTH_TOKEN.)
backend="${AGENT_BACKEND:-}"
if [[ -z "$backend" && -f "$REPO_DIR/.env" ]]; then
  backend="$(grep -E '^[[:space:]]*AGENT_BACKEND=' "$REPO_DIR/.env" | tail -1 | sed -E 's/^[[:space:]]*AGENT_BACKEND=//; s/^"//; s/"$//' | tr -d '[:space:]')"
fi
backend="${backend:-codex}"
case "$backend" in
  claude)
    guard_keys=(ANTHROPIC_API_KEY); subscription="Claude Pro/Max subscription"
    ;;
  codex)
    guard_keys=(OPENAI_API_KEY CODEX_API_KEY); subscription="ChatGPT subscription"
    ;;
  *)
    die "AGENT_BACKEND must be codex or claude (got '$backend')."
    ;;
esac
for key in "${guard_keys[@]}"; do
  [[ -z "${!key:-}" ]] || die "$key is set in the environment. Unset it (the $backend backend must ride the $subscription)."
  if [[ -f "$REPO_DIR/.env" ]] && grep -qE "^[[:space:]]*${key}=" "$REPO_DIR/.env"; then
    die "$key is set in .env. Remove it (the $backend backend must ride the $subscription)."
  fi
done

# --- 2. System packages (Ruby build deps + the C toolchain the Rust build needs) --------------
log "Installing system packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y \
  git curl ca-certificates build-essential cmake pkg-config \
  libssl-dev libyaml-dev libffi-dev zlib1g-dev libreadline-dev libgdbm-dev libncurses-dev

# --- 3. mise + pinned Ruby/Node (.mise.toml) --------------------------------------------------
MISE="$USER_HOME/.local/bin/mise"
if [[ ! -x "$MISE" ]]; then
  log "Installing mise for $SERVICE_USER"
  run_as "curl -fsSL https://mise.run | sh"
fi
log "mise: $("$MISE" --version 2>/dev/null || echo unknown)"
run_as "cd '$REPO_DIR' && '$MISE' trust && '$MISE' install"
# Promote Node to the GLOBAL default so `mise exec -- node/<cli>` resolves from any dir (the Playwright
# install and workers run in arbitrary subdirs with no local .mise.toml). The APP language toolchain
# is NOT installed here — it follows the active stack and is added after the lila binary is built (so
# `lila stack` can read it). Node is always needed (agent CLI + validation), the app stack is not.
run_as "'$MISE' use -g node@22"

# --- 4. Agent CLIs + Rust toolchain + BUILD the lila binary -----------------------------------
log "Installing Codex + Claude CLIs under the mise-managed Node"
run_as "cd '$REPO_DIR' && '$MISE' exec -- npm install -g @openai/codex @anthropic-ai/claude-code"

log "Installing the Rust toolchain (rustup) for $SERVICE_USER"
run_as '[ -x "$HOME/.cargo/bin/cargo" ] || curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal'
run_as '"$HOME/.cargo/bin/rustc" --version'

# The release build (thin-LTO) is memory-heavy; add swap on small boxes so it doesn't OOM.
TOTAL_MB="$(free -m | awk '/^Mem:/{print $2}')"
if [[ "${TOTAL_MB:-0}" -lt 3000 ]] && ! swapon --show | grep -q .; then
  log "Low RAM (${TOTAL_MB}MB) — adding a 4G swapfile for the build"
  fallocate -l 4G /swapfile && chmod 600 /swapfile && mkswap /swapfile >/dev/null && swapon /swapfile
  grep -q '^/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

log "Building the lila binary (cargo build --release; this takes a few minutes)"
run_as "cd '$REPO_DIR' && CARGO_BUILD_JOBS=2 \"\$HOME/.cargo/bin/cargo\" build --release --bin lila"
install -d -m 0755 /opt/lila/bin
install -m 0755 "$REPO_DIR/target/release/lila" /opt/lila/bin/lila
log "Installed /opt/lila/bin/lila ($(/opt/lila/bin/lila --version 2>/dev/null || echo lila))"

# --- 4a. App toolchain from the ACTIVE STACK (now that `lila stack` can read the contract) -------
# The kind of app the team builds is a stack plugin (stacks/<name>/); its language toolchain (ruby@3.3
# for rails-pwa, nothing extra for node-react) is installed globally here. Node is already in place.
LILA_STACK="${LILA_STACK:-rails-pwa}"
eval "$(cd "$REPO_DIR" && /opt/lila/bin/lila stack "$LILA_STACK")" ||
  die "Unknown LILA_STACK '$LILA_STACK' (expected a directory under $REPO_DIR/stacks/)."
log "Active app stack: $LILA_STACK_DISPLAY"
if [[ -n "$LILA_STACK_TOOLCHAIN" ]]; then
  log "Installing the $LILA_STACK app toolchain: $LILA_STACK_TOOLCHAIN"
  run_as "'$MISE' use -g $LILA_STACK_TOOLCHAIN"
fi

# The mise node bin dir holds BOTH agent CLIs and the `node` they shebang to; the native systemd
# unit puts it on PATH (it gets no mise shell hook). Resolve it now for the unit substitution below.
CODEX_CLI="$(run_as "'$MISE' which codex" 2>/dev/null || true)"
CLAUDE_CLI="$(run_as "'$MISE' which claude" 2>/dev/null || true)"
NODEBIN="$(dirname -- "$CODEX_CLI" 2>/dev/null || true)"
CLAUDE_NODEBIN="$(dirname -- "$CLAUDE_CLI" 2>/dev/null || true)"
[[ -n "$CODEX_CLI" && -x "$CODEX_CLI" ]] || die "Could not resolve the codex CLI via mise."
[[ -n "$CLAUDE_CLI" && -x "$CLAUDE_CLI" ]] || die "Could not resolve the claude CLI via mise."
[[ "$NODEBIN" == "$CLAUDE_NODEBIN" ]] ||
  die "Codex and Claude CLIs resolved to different bin dirs ($NODEBIN vs $CLAUDE_NODEBIN); systemd needs both in one mise Node bin dir."

# --- 4b. Playwright + headless Chromium (workers self-validate: drive + screenshot the app) ----
# The `playwright` npm package goes into a FIXED, node-version-independent location
# ($PW_TOOLING/node_modules) — NOT `npm install -g`, whose prefix is tied to the active node version
# and silently orphans on the next node upgrade. The service unit exports
# NODE_PATH=$PW_TOOLING/node_modules so workers can `require("playwright")` / `npx playwright …` from
# any cwd. The browser binary lands once in the service user's ~/.cache/ms-playwright.
PW_TOOLING="/opt/lila/tooling"
log "Installing Playwright (-> $PW_TOOLING) + headless Chromium (worker self-validation)"
mkdir -p "$PW_TOOLING"
chown "$SERVICE_USER:$SERVICE_USER" "$PW_TOOLING"
run_as "cd '$PW_TOOLING' && { [ -f package.json ] || '$MISE' exec -- npm init -y >/dev/null; } && '$MISE' exec -- npm install playwright"
run_as "cd '$PW_TOOLING' && NODE_PATH='$PW_TOOLING/node_modules' '$MISE' exec -- npx playwright install chromium"
( cd "$PW_TOOLING" && HOME="$USER_HOME" NODE_PATH="$PW_TOOLING/node_modules" "$MISE" exec -- npx playwright install-deps chromium ) \
  || log "WARN: 'playwright install-deps' failed — chromium may need system libs; rerun bootstrap or run it by hand"
run_as "NODE_PATH='$PW_TOOLING/node_modules' '$MISE' exec -- node -e 'require(\"playwright\"); console.log(\"playwright require() OK via NODE_PATH\")'" \
  || die "Playwright is not require()-resolvable via NODE_PATH=$PW_TOOLING/node_modules — worker self-validation would fail."

# --- 4c. Fetch the full Open Design catalog -----------------------------------------------------
# Only the safety-net subset (default-pool neutrals + stripe) is vendored in git; the rest of the
# 150-system catalog is pulled here at standup so an explicit LILA_DESIGN=<brand> pin can reach any
# system. Non-fatal: a blind `random` draw is bounded to the committed default pool, so the app still
# gets a design even if this can't reach GitHub — re-run bin/fetch-design-catalog later to backfill.
log "Fetching the Open Design catalog (pinned commit; bin/fetch-design-catalog)"
run_as "cd '$REPO_DIR' && bash bin/fetch-design-catalog" \
  || log "WARN: design catalog fetch failed — blind draws still work from the committed default pool; rerun 'bin/fetch-design-catalog' to enable <brand> pins"

# --- 5. Data dirs + workspace (owned by the service user) -------------------------------------
# The host's first (primary) app is just instance "primary": it runs under the SAME systemd template
# units as every additional bin/new-instance app (lila-{manager,app}@primary), serving /srv/primary
# and reading /etc/lila/primary.env. n=1 is the n>1 path.
INSTANCE="${LILA_INSTANCE:-primary}"
WORKSPACE_DIR="${WORKSPACE_DIR:-/srv/$INSTANCE}"
MEMORY_DIR="${MEMORY_DIR:-/var/lib/lila/memory}"
STATE_DIR="${MANAGER_STATE_DIR:-/var/lib/lila/state}"
CODEX_HOME="${CODEX_HOME:-/var/lib/lila/codex}"
log "Creating $WORKSPACE_DIR, $MEMORY_DIR, $STATE_DIR, $CODEX_HOME"
mkdir -p "$WORKSPACE_DIR" "$MEMORY_DIR" "$STATE_DIR" "$CODEX_HOME"
chown -R "$SERVICE_USER:$SERVICE_USER" "$WORKSPACE_DIR" /var/lib/lila

# Init the workspace git repo (the agent's playground + rollback). The worker AGENTS.md/CLAUDE.md
# standing rules are written into the workspace by the manager at startup — no static file to seed.
run_as "git -C '$WORKSPACE_DIR' rev-parse --git-dir >/dev/null 2>&1 || { git -C '$WORKSPACE_DIR' init -q && git -C '$WORKSPACE_DIR' config user.email 'lila@localhost' && git -C '$WORKSPACE_DIR' config user.name 'Little Living Apps'; }"

# --- 6. Environment file for systemd ----------------------------------------------------------
# The primary instance reads /etc/lila/$INSTANCE.env (the template unit's EnvironmentFile=%i.env).
ENV_FILE="/etc/lila/$INSTANCE.env"
mkdir -p /etc/lila
if [[ -f "$REPO_DIR/.env" ]]; then
  install -m 600 "$REPO_DIR/.env" "$ENV_FILE"
  log "Installed $ENV_FILE (from .env)"
else
  log "WARNING: no .env found — create $ENV_FILE from .env.example before starting."
  install -m 600 /dev/null "$ENV_FILE"
fi
# Pin the vars the template units couple to the instance, so a stale .env can't desync them.
ensure_env() { # ensure_env KEY VALUE — replace KEY in $ENV_FILE if present, else append it
  local key="$1" val="$2"
  if grep -qE "^${key}=" "$ENV_FILE"; then
    sed -i "s|^${key}=.*|${key}=${val}|" "$ENV_FILE"
  else
    printf '%s=%s\n' "$key" "$val" >> "$ENV_FILE"
  fi
}
ensure_env WORKSPACE_DIR "$WORKSPACE_DIR"
ensure_env APP_PORT "${APP_PORT:-3000}"
ensure_env INSPECTOR_PORT "${INSPECTOR_PORT:-9090}"
ensure_env LILA_APP_SERVICE "lila-app@$INSTANCE"
ensure_env CODEX_BIN "$CODEX_CLI"
ensure_env CLAUDE_BIN "$CLAUDE_CLI"

# --- 7. systemd manager unit (template) -------------------------------------------------------
log "Installing systemd template unit lila-manager@.service; enabling lila-manager@$INSTANCE"
sed -e "s|__USER__|$SERVICE_USER|g" -e "s|__NODEBIN__|$NODEBIN|g" \
  "$REPO_DIR/deploy/lila-manager@.service" > /etc/systemd/system/lila-manager@.service
systemctl daemon-reload
systemctl enable "lila-manager@$INSTANCE"

# Expose the app scaffolder on PATH so a worker (or you) can run `lila-new-app` to stand up the
# Rails 8 + PWA app the team builds. It forwards LILA_INSTANCE — set per-manager by the template unit
# (Environment=LILA_INSTANCE=%i) and inherited by workers — so a worker always (re)scaffolds ITS app.
cat > /usr/local/bin/lila-new-app <<EOF
#!/usr/bin/env bash
exec sudo LILA_INSTANCE="\${LILA_INSTANCE:-$INSTANCE}" bash "$REPO_DIR/bin/new-app" "\$@"
EOF
chmod +x /usr/local/bin/lila-new-app

# --- 8. Publish the primary app behind your domain (Caddy auto-HTTPS), if LILA_DOMAIN is set -----
source "$REPO_DIR/bin/lib-caddy.sh"
LILA_DOMAIN="${LILA_DOMAIN:-$(grep -E '^LILA_DOMAIN=' "$REPO_DIR/.env" 2>/dev/null | cut -d= -f2- | tr -d '"'\''' )}"
if [[ -n "$LILA_DOMAIN" ]]; then
  log "Publishing the app on https://$LILA_DOMAIN (Caddy)"
  if ! command -v caddy >/dev/null 2>&1; then
    apt-get install -y debian-keyring debian-archive-keyring apt-transport-https curl gnupg
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list
    apt-get update -y && apt-get install -y caddy
  fi
  install -m 644 "$REPO_DIR/deploy/Caddyfile" /etc/caddy/Caddyfile
  lila_write_caddy_site "$INSTANCE" "$LILA_DOMAIN" "${APP_PORT:-3000}" "${INSPECTOR_PORT:-9090}" ||
    die "Caddy publish failed for the primary instance '$INSTANCE' — check /etc/caddy/sites/$INSTANCE.caddy"
  systemctl enable caddy >/dev/null 2>&1 || true
  log "Caddy serving https://$LILA_DOMAIN -> 127.0.0.1:${APP_PORT:-3000} (cert issues on first request; needs 80/443 open + DNS at this host)."
else
  log "LILA_DOMAIN not set — skipping Caddy; the app stays private to the host (publish later by setting LILA_DOMAIN and re-running)."
fi

# --- 9. Subscription auth (interactive, one-time) ---------------------------------------------
if [[ "$backend" == "codex" ]] && run_as "CODEX_HOME='$CODEX_HOME' '$MISE' exec -- codex login status" >/dev/null 2>&1; then
  log "Codex auth present — starting the service"
  systemctl restart "lila-manager@$INSTANCE"
  log "Done. Message your bot on Telegram to test the loop."
elif [[ "$backend" == "codex" ]]; then
  cat <<EOF

────────────────────────────────────────────────────────────────────────────
Almost there. Authenticate Codex (ChatGPT subscription), then start the bot:

  sudo -u $SERVICE_USER -H CODEX_HOME=$CODEX_HOME $MISE exec -- codex login --device-auth
  sudo systemctl start lila-manager@$INSTANCE

Follow the logs with:  journalctl -u lila-manager@$INSTANCE -f
────────────────────────────────────────────────────────────────────────────
EOF
else
  cat <<EOF

────────────────────────────────────────────────────────────────────────────
Almost there. Authenticate Claude (Claude Pro/Max subscription), then start the bot:

  sudo -u $SERVICE_USER -H $MISE exec -- claude setup-token
  # Copy the printed token into $ENV_FILE as:
  #   CLAUDE_CODE_OAUTH_TOKEN=<token>
  sudo systemctl start lila-manager@$INSTANCE

Follow the logs with:  journalctl -u lila-manager@$INSTANCE -f
────────────────────────────────────────────────────────────────────────────
EOF
fi
