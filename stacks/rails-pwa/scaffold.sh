#!/usr/bin/env bash
# stacks/rails-pwa/scaffold.sh — scaffold ONE instance's app as a minimal Rails 8 + PWA project.
# Lifted verbatim from the old bin/new-app (steps 1, 2, 2b, 3); the generic bin/new-app now owns the
# framework contract (instance dir, env file, service user, systemd) and delegates the app body here.
#
# Invoked by bin/new-app as the SERVICE USER, with the cwd already at the app dir and these vars in
# the environment: LILA_INSTANCE, APP_DIR, APP_PORT, LILA_DOMAIN, SKIP_AUTH, SERVICE_USER, MISE.
# Deliberately thin: it leans on Rails 8's own defaults (SQLite, the Solid stack, Hotwire, PWA stubs)
# and generators rather than vendoring a template. Idempotent: re-running on a scaffolded app is a
# no-op for the app body (the service install/start lives back in bin/new-app).
set -euo pipefail

log() { printf '\033[1;35m[scaffold:rails-pwa]\033[0m %s\n' "$*"; }

# --- 1. Rails (install the gem once, then scaffold if not already a Rails app) -----------------
if ! "$MISE" exec -- gem list -i '^rails$' >/dev/null 2>&1; then
  log "Installing Rails 8"
  "$MISE" exec -- gem install rails -v '~> 8.0' --no-document
fi

if [[ -f "$APP_DIR/config/application.rb" ]]; then
  log "Rails app already present at $APP_DIR — skipping scaffold"
else
  log "Scaffolding a Rails 8 app at $APP_DIR (SQLite + Solid + Hotwire + PWA defaults)"
  # --skip-git: bootstrap already inited the repo. Keep Rails 8 defaults otherwise (that IS the
  # opinionated stack); the agent adds everything else.
  "$MISE" exec -- rails new . --skip-git

  if [[ -z "${SKIP_AUTH:-}" ]]; then
    log "Generating Rails' built-in authentication (for private access behind your domain)"
    "$MISE" exec -- bin/rails generate authentication
  fi
fi

# --- 2. PWA: enable the routes Rails 8 ships (commented by default) + link the manifest ---------
ROUTES="$APP_DIR/config/routes.rb"
if [[ -f "$ROUTES" ]]; then
  sed -i 's|^[[:space:]]*#[[:space:]]*\(get "service-worker".*\)|  \1|; s|^[[:space:]]*#[[:space:]]*\(get "manifest".*\)|  \1|' "$ROUTES"
  # Reserve /_agent/* for an (opt-in) in-app agent surface so a worker never collides with it.
  if ! grep -q "_agent" "$ROUTES"; then
    awk 'NR==1{print; print "  # Reserved: /_agent/* is for an optional in-app agent surface — do not route app paths here."; next} {print}' "$ROUTES" > "$ROUTES.tmp" && mv "$ROUTES.tmp" "$ROUTES"
  fi
fi

LAYOUT="$APP_DIR/app/views/layouts/application.html.erb"
if [[ -f "$LAYOUT" ]] && ! grep -q 'rel="manifest"' "$LAYOUT"; then
  log "Linking the PWA manifest in the app layout"
  # Matches the route Rails 8 ships (get "manifest" => "rails/pwa#manifest") served at /manifest.
  sed -i 's|</head>|    <link rel="manifest" href="/manifest">\n  </head>|' "$LAYOUT"
fi

# --- 2b. Allow the published host (reload mode = development, where Rails host-auth blocks all but
# localhost). Caddy forwards the original Host, so the app must permit the domain it's served on. ---
DEV_ENV="$APP_DIR/config/environments/development.rb"
if [[ -n "${LILA_DOMAIN:-}" && -f "$DEV_ENV" ]] && ! grep -q "lila published host" "$DEV_ENV"; then
  log "Allowing published host $LILA_DOMAIN in development host-authorization"
  # If this is an already-checkpointed Rails app, checkpoint this platform-managed host auth line
  # immediately. During the initial scaffold the app tree is still untracked; the worker's scaffold
  # commit will include this line with the rest of the generated app.
  can_checkpoint_host=0
  if git ls-files --error-unmatch config/environments/development.rb >/dev/null 2>&1 && git diff --quiet -- config/environments/development.rb; then
    can_checkpoint_host=1
  fi
  sed -i "s|^Rails.application.configure do|Rails.application.configure do\n  config.hosts << \"$LILA_DOMAIN\" # lila published host|" "$DEV_ENV"
  if [[ "$can_checkpoint_host" -eq 1 ]]; then
    git add config/environments/development.rb && git commit -m 'Allow published host in development' >/dev/null
    log "Committed published-host allowance in the app repo"
  fi
fi

# --- 3. Prepare the databases (SQLite + Solid Queue/Cache/Cable) -------------------------------
log "Preparing databases"
RAILS_ENV=development "$MISE" exec -- bin/rails db:prepare

# --- 4. Install the locked design system's curated baseline ------------------------------------
# Copies the drawn Open Design system's CURATED assets (LILA_DESIGN_DIR) into the app: upstream's
# machine-readable tokens.css becomes the app's token sink, and the rest of the package (DESIGN.md,
# USAGE.md, components.html + manifest, design-tokens.json) is carried into .lila/ as the worker's
# reference — the agent adapts those reference components to ERB per the system's own USAGE.md. We do
# NOT generate tokens or ship a hand-written component layer. Writes design.lock (the active system +
# the selection-flow state). Idempotent and STABLE: if design.lock already exists we DO NOT re-copy or
# reroll — the look is locked for life; only the design skill (a user-driven selection) rewrites it.
# No-ops if the stack didn't opt in or no system was drawn.
render_design() {
  local tokens layout pkg
  if [[ -z "${LILA_DESIGN_DIR:-}" || -z "${LILA_STACK_DESIGN_TOKENS:-}" ]]; then
    log "No design system drawn — skipping the design baseline"; return 0
  fi
  if [[ -f "$APP_DIR/design.lock" ]]; then
    log "design.lock present — keeping the locked look (no reroll)"; return 0
  fi

  log "Installing curated design system '$LILA_DESIGN_BRAND' (Open Design package)"
  tokens="$APP_DIR/$LILA_STACK_DESIGN_TOKENS"
  mkdir -p "$(dirname "$tokens")" "$APP_DIR/.lila"

  # Upstream's curated, machine-readable tokens.css is the app's token sink (agents paste its :root
  # block and reference var(--name)). Copied verbatim — not generated.
  cp "$LILA_DESIGN_DIR/tokens.css" "$tokens"
  # Carry the rest of the curated package into .lila/ as the worker + design skill's reference (its
  # visual intent + anti-patterns, the reference components, the structured tokens).
  for f in DESIGN.md USAGE.md components.html components.manifest.json design-tokens.json; do
    [[ -f "$LILA_DESIGN_DIR/$f" ]] && cp "$LILA_DESIGN_DIR/$f" "$APP_DIR/.lila/$f"
  done

  # Link the token sheet (Propshaft resolves the logical name to its fingerprinted file). The app's
  # own component CSS is built by the agent on top of these tokens, per .lila/USAGE.md.
  layout="$APP_DIR/app/views/layouts/application.html.erb"
  if [[ -f "$layout" ]] && ! grep -q 'stylesheet_link_tag "tokens"' "$layout"; then
    sed -i 's|</head>|    <%= stylesheet_link_tag "tokens" %>\n  </head>|' "$layout"
  fi

  # The lock: the active system + the selection-flow state machine (source=default for a blind draw,
  # pinned for an explicit LILA_DESIGN=<brand>). The design skill is the only thing that rewrites it.
  cat > "$APP_DIR/design.lock" <<LOCK
brand  = "${LILA_DESIGN_BRAND}"
pool   = "${LILA_DESIGN_POOL}"
source = "${LILA_DESIGN_SOURCE}"
seed   = ${LILA_DESIGN_SEED:-0}
commit = "${LILA_DESIGN_COMMIT}"
LOCK

  # Commit the baseline if this is already a checkpointed repo; during the initial scaffold the tree is
  # still untracked and the worker's scaffold commit will include these files.
  if git -C "$APP_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1 && [[ -n "$(git -C "$APP_DIR" log -1 2>/dev/null)" ]]; then
    git -C "$APP_DIR" add design.lock .lila "$LILA_STACK_DESIGN_TOKENS" \
      app/views/layouts/application.html.erb 2>/dev/null || true
    git -C "$APP_DIR" commit -m "Install '$LILA_DESIGN_BRAND' design baseline (curated tokens + reference)" >/dev/null 2>&1 || true
  fi
}
render_design
