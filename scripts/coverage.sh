#!/usr/bin/env bash
# Core line-coverage measurement, gate, and badge generator — the number behind the README
# "coverage" badge. "Core" is the deterministic logic the `cargo test` suite is meant to cover; it
# EXCLUDES the live-only seams (the real network runners, transport I/O) and the CLI entrypoints,
# which only run against a real subscription (the #[ignore]d `live_*` tests) and so are unreachable
# from `cargo test`. Excluding them keeps the figure honest about what the suite actually exercises.
#
# Usage:
#   scripts/coverage.sh            # print the core line-coverage % (e.g. "73.2")
#   scripts/coverage.sh badge      # emit shields.io endpoint JSON (for the `badges` branch)
#   scripts/coverage.sh check 70   # exit non-zero if core coverage < 70 (CI regression gate)
#
# Needs cargo-llvm-cov + the llvm-tools-preview component:
#   rustup component add llvm-tools-preview && cargo install cargo-llvm-cov
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"

# Real-subscription seams — the manager/worker backends that shell out to the live `codex`/`claude`
# CLIs and the `lila eval` harness entrypoint. Only reachable from the #[ignore]d live_* tests or by
# hand, so `cargo test` can never cover them. Everything else — including the `lila run` daemon
# (manager/app.rs, commands/run.rs, the pollers/deliver) — IS exercised by the binary-driven
# integration tests, whose spawned daemon now forwards LLVM_PROFILE_FILE and exits on SIGTERM so its
# profile is captured. Keep this in lock-step with the README's coverage-scope note.
IGNORE='(bin/|manager/(claude|codex)\.rs|workers/(claude_runner|codex_runner|real|runner)\.rs|eval/(report|run)\.rs)'

measure() {
  # Two steps so the figure is robust to a flaky test: a non-zero `cargo llvm-cov` (one e2e test
  # races under parallel load) makes the all-in-one form skip report generation. So RUN with
  # --no-report (tolerating any failure), then build the summary from the collected profile with the
  # `report` subcommand, which ignores test exit codes entirely.
  #
  # rails_fixture_planted_realities self-skips without the vendored Rails gems (the CI case) but
  # PANICS on a half-set-up local toolchain (aborting the run) — skip it by name so the profile is
  # always collected, identically on CI and dev boxes.
  cargo llvm-cov --no-report --no-fail-fast \
    -- --skip rails_fixture_planted_realities >/dev/null 2>&1 || true
  local out
  out="$(cargo llvm-cov report --summary-only --ignore-filename-regex "$IGNORE" 2>/dev/null)"
  # TOTAL row: Regions Missed R% Functions Missed F% Lines Missed L% ... — the line % is the 3rd %.
  echo "$out" | awk '/^TOTAL/{ n=0; for(i=1;i<=NF;i++) if($i ~ /%$/){ n++; if(n==3){ sub(/%/,"",$i); print $i } } }'
}

color_for() { # integer pct -> shields color
  local p=$1
  if   [ "$p" -ge 90 ]; then echo brightgreen
  elif [ "$p" -ge 75 ]; then echo green
  elif [ "$p" -ge 60 ]; then echo yellowgreen
  elif [ "$p" -ge 50 ]; then echo yellow
  else echo orange
  fi
}

PCT="$(measure)"
if [ -z "${PCT:-}" ]; then
  echo "coverage.sh: could not parse a TOTAL line coverage from cargo-llvm-cov" >&2
  exit 2
fi
ROUNDED="$(printf '%.0f' "$PCT")"

case "${1:-print}" in
  print)
    echo "$PCT"
    ;;
  badge)
    printf '{"schemaVersion":1,"label":"coverage","message":"%s%%","color":"%s"}\n' \
      "$ROUNDED" "$(color_for "$ROUNDED")"
    ;;
  check)
    floor="${2:?usage: scripts/coverage.sh check <floor>}"
    echo "core line coverage: ${PCT}% (floor ${floor}%)"
    if [ "$ROUNDED" -lt "$floor" ]; then
      echo "❌ coverage ${ROUNDED}% is below the ${floor}% floor" >&2
      exit 1
    fi
    echo "✅ coverage ${ROUNDED}% ≥ ${floor}%"
    ;;
  *)
    echo "usage: scripts/coverage.sh [print|badge|check <floor>]" >&2
    exit 2
    ;;
esac
