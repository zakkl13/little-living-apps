#!/usr/bin/env bash
# One-time setup for the Rails eval fixture: install the app's gems into its OWN vendor/bundle so the
# template is self-contained and per-trial copies need no network. The eval harness copies this app
# (APFS clone — instant) per trial, so the cost lands here, once, not per trial.
#
# Requires Ruby 3.2+ / Rails 8 on PATH (e.g. `export PATH=/opt/homebrew/opt/ruby/bin:$PATH`).
#   eval/fixtures/setup-rails.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP="$HERE/rails-app"

command -v ruby >/dev/null || { echo "ruby not on PATH (need 3.2+); try: export PATH=/opt/homebrew/opt/ruby/bin:\$PATH" >&2; exit 2; }
echo ">> ruby $(ruby --version)"
cd "$APP"
bundle config set --local path vendor/bundle
echo ">> bundle install (vendored)…"
bundle install
echo ">> sanity: bin/rails test"
bin/rails test
echo "✅ Rails fixture ready ($APP)"
