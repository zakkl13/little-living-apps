#!/usr/bin/env bash
# provision.sh — runs on your LAPTOP. Creates a Sprite, pushes this repo, runs bootstrap.sh on
# the Sprite, and registers the bot as a Sprite Service (SPEC §10).
#
# ⚠️  The Sprites product is new (Jan 2026). The CLI command names below are best-effort and
#     MUST be confirmed against docs.sprites.dev. They are isolated in the `sprite_*` helper
#     functions so you only have to fix them in one place.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPRITE_NAME="${SPRITE_NAME:-codex-bot}"
REMOTE_REPO_DIR="/workspace/sprite-codex-bot"

# Load local .env (bot token, allowlist, secret, sandbox mode, etc.)
if [[ -f "$REPO_ROOT/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$REPO_ROOT/.env"
  set +a
fi

log() { printf '\033[1;32m[provision]\033[0m %s\n' "$*"; }
die() { echo "ERROR: $*" >&2; exit 1; }

: "${SPRITES_TOKEN:?Set SPRITES_TOKEN (Fly Sprites API token) in .env}"
: "${TELEGRAM_BOT_TOKEN:?Set TELEGRAM_BOT_TOKEN in .env}"
: "${ALLOWED_USER_IDS:?Set ALLOWED_USER_IDS in .env}"
: "${TELEGRAM_WEBHOOK_SECRET:?Set TELEGRAM_WEBHOOK_SECRET in .env}"

command -v sprite >/dev/null 2>&1 || die "The 'sprite' CLI is not installed. See docs.sprites.dev."

# --- Sprites CLI shims (CONFIRM these against docs.sprites.dev) -------------------------------
sprite_create()  { sprite create "$SPRITE_NAME" || true; }                     # idempotent-ish
sprite_push()    { sprite push "$REPO_ROOT" "$SPRITE_NAME:$REMOTE_REPO_DIR"; } # copy repo onto volume
sprite_run()     { sprite exec "$SPRITE_NAME" -- "$@"; }                        # run a command on the Sprite
sprite_url()     { sprite url "$SPRITE_NAME"; }                                 # routed HTTPS URL
sprite_service() {                                                             # register a long-running Service
  sprite service create "$SPRITE_NAME" \
    --name codex-bot \
    --workdir "$REMOTE_REPO_DIR" \
    --command "node dist/index.js" \
    "$@"
}

log "1/6 Creating Sprite '$SPRITE_NAME'"
sprite_create

log "2/6 Pushing repo to $SPRITE_NAME:$REMOTE_REPO_DIR"
sprite_push

log "3/6 Installing deps + building on the Sprite"
sprite_run bash -lc "cd '$REMOTE_REPO_DIR' && npm ci && npm run build"

log "4/6 Bootstrapping Codex + workspace repo"
sprite_run bash -lc "cd '$REMOTE_REPO_DIR' && WORKSPACE_DIR='${WORKSPACE_DIR:-/workspace/project}' CODEX_HOME='${CODEX_HOME:-/workspace/.codex}' REPO_DIR='$REMOTE_REPO_DIR' bash provision/bootstrap.sh"

SPRITE_PUBLIC_URL="$(sprite_url)"
log "Sprite URL: $SPRITE_PUBLIC_URL"

log "5/6 Probing Codex (should return on the subscription path)"
# Raw CLI smoke check that the box can run Codex at all under full access. The bot itself drives
# Codex via @openai/codex-sdk, but this verifies auth + sandbox-free execution in the microVM.
sprite_run bash -lc "CODEX_HOME='${CODEX_HOME:-/workspace/.codex}' codex exec --skip-git-repo-check -C '${WORKSPACE_DIR:-/workspace/project}' --sandbox danger-full-access 'respond with OK'" \
  || log "WARNING: Codex probe failed — finish 'codex login --device-auth' on the Sprite first."

log "6/6 Registering the bot as a Sprite Service"
# Inject env vars into the Service. CONFIRM the exact secret/env syntax in docs.sprites.dev.
sprite_service \
  --env "TELEGRAM_BOT_TOKEN=$TELEGRAM_BOT_TOKEN" \
  --env "ALLOWED_USER_IDS=$ALLOWED_USER_IDS" \
  --env "TELEGRAM_WEBHOOK_SECRET=$TELEGRAM_WEBHOOK_SECRET" \
  --env "PUBLIC_URL=$SPRITE_PUBLIC_URL" \
  --env "PORT=${PORT:-8080}" \
  --env "WORKSPACE_DIR=${WORKSPACE_DIR:-/workspace/project}" \
  --env "SESSION_STORE_PATH=${SESSION_STORE_PATH:-/workspace/.sessions.json}" \
  --env "CODEX_HOME=${CODEX_HOME:-/workspace/.codex}" \
  --env "CODEX_SANDBOX_MODE=${CODEX_SANDBOX_MODE:-danger-full-access}"

log "Done. The bot Service will start and call setWebhook($SPRITE_PUBLIC_URL) on boot."
log "Message your bot on Telegram to test the loop."
