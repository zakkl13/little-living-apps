#!/usr/bin/env bash
# bootstrap.sh — runs ON the Sprite. Installs Codex + git, prepares the persistent workspace
# git repo, places AGENTS.md, and points CODEX_HOME at the persistent volume (SPEC §10.3).
#
# Idempotent: safe to re-run on every provision or after a wipe.
set -euo pipefail

WORKSPACE_DIR="${WORKSPACE_DIR:-/workspace/project}"
CODEX_HOME="${CODEX_HOME:-/workspace/.codex}"
REPO_DIR="${REPO_DIR:-/workspace/sprite-codex-bot}" # where provision.sh pushed this repo

log() { printf '\033[1;34m[bootstrap]\033[0m %s\n' "$*"; }

# --- Refuse the billing-flip footgun (SPEC §4) -----------------------------------------------
if [[ -n "${OPENAI_API_KEY:-}" ]]; then
  echo "ERROR: OPENAI_API_KEY is set on the Sprite. This flips Codex to metered API billing." >&2
  echo "       Unset it; auth must ride the ChatGPT subscription (SPEC §4)." >&2
  exit 1
fi

# --- System deps ------------------------------------------------------------------------------
if ! command -v git >/dev/null 2>&1; then
  log "Installing git"
  if command -v apt-get >/dev/null 2>&1; then
    apt-get update -y && apt-get install -y git
  else
    echo "ERROR: git missing and no apt-get found; install git manually." >&2
    exit 1
  fi
fi

if ! command -v node >/dev/null 2>&1; then
  echo "ERROR: Node.js not found. Provision the Sprite image with Node >= 22 first." >&2
  exit 1
fi

# --- Codex CLI --------------------------------------------------------------------------------
if ! command -v codex >/dev/null 2>&1; then
  log "Installing @openai/codex"
  npm install -g @openai/codex
fi
log "codex version: $(codex --version 2>/dev/null || echo unknown)"

# --- Persistent CODEX_HOME (auth survives hibernation, SPEC §8) -------------------------------
mkdir -p "$CODEX_HOME"
log "CODEX_HOME=$CODEX_HOME"

# --- Workspace git repo (Codex playground + rollback, SPEC §8) --------------------------------
mkdir -p "$WORKSPACE_DIR"
if [[ ! -d "$WORKSPACE_DIR/.git" ]]; then
  log "Initializing git repo at $WORKSPACE_DIR"
  git -C "$WORKSPACE_DIR" init -q
  git -C "$WORKSPACE_DIR" config user.email "codex@sprite.local" || true
  git -C "$WORKSPACE_DIR" config user.name "Codex Sprite Bot" || true
fi

# --- Drop worker standing rules + memory-bank templates (DESIGN §6) ---------------------------
# Workers load AGENTS.md automatically; the memory-bank/ is their per-codebase durable memory.
if [[ -f "$REPO_DIR/provision/AGENTS.md" ]]; then
  cp "$REPO_DIR/provision/AGENTS.md" "$WORKSPACE_DIR/AGENTS.md"
  log "Placed worker AGENTS.md in $WORKSPACE_DIR"
elif [[ -f "$REPO_DIR/AGENTS.md" ]]; then
  cp "$REPO_DIR/AGENTS.md" "$WORKSPACE_DIR/AGENTS.md"
  log "Placed repo AGENTS.md in $WORKSPACE_DIR (worker rules not found)"
else
  log "WARNING: no AGENTS.md found; skipping (place it manually)."
fi

if [[ -d "$REPO_DIR/provision/memory-bank" && ! -d "$WORKSPACE_DIR/memory-bank" ]]; then
  cp -r "$REPO_DIR/provision/memory-bank" "$WORKSPACE_DIR/memory-bank"
  log "Seeded memory-bank/ templates in $WORKSPACE_DIR"
fi

# --- Manager memory + state dirs (persistent volume) ------------------------------------------
mkdir -p "${MEMORY_DIR:-/workspace/.manager/memory}" "${MANAGER_STATE_DIR:-/workspace/.manager/state}"
log "Manager memory dir: ${MEMORY_DIR:-/workspace/.manager/memory}"

# --- Auth assertion (SPEC §10.4) --------------------------------------------------------------
# Path A: run `CODEX_HOME=$CODEX_HOME codex login --device-auth` interactively before this.
# Path B: write $CODEX_AUTH_JSON to $CODEX_HOME/auth.json before this.
if [[ -n "${CODEX_AUTH_JSON:-}" && ! -f "$CODEX_HOME/auth.json" ]]; then
  log "Writing injected auth.json (Path B)"
  printf '%s' "$CODEX_AUTH_JSON" > "$CODEX_HOME/auth.json"
  chmod 600 "$CODEX_HOME/auth.json"
fi

if CODEX_HOME="$CODEX_HOME" codex login status >/dev/null 2>&1; then
  log "codex login status: OK (subscription auth present)"
else
  echo "WARNING: codex is not authenticated yet. Run:" >&2
  echo "         CODEX_HOME=$CODEX_HOME codex login --device-auth" >&2
fi

log "bootstrap complete"
