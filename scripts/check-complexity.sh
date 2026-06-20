#!/usr/bin/env bash
# Authoritative cyclomatic-complexity gate: fail if ANY function exceeds CCN 6. Uses lizard (a
# purpose-built, language-agnostic cyclomatic analyzer) — `pip install lizard`. Run from the crate
# root: `scripts/check-complexity.sh`.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v lizard >/dev/null 2>&1 && ! python3 -m lizard --version >/dev/null 2>&1; then
  echo "lizard not found — install with: pip install lizard" >&2
  exit 2
fi
LIZARD=(python3 -m lizard)

# -C 6: warn threshold = cyclomatic 6. -w + a non-zero exit on any warning is the gate.
if "${LIZARD[@]}" "$HERE/src" -l rust -C 6 -w | grep -qE '@.*\.rs'; then
  echo "❌ cyclomatic complexity > 6 in the functions above" >&2
  "${LIZARD[@]}" "$HERE/src" -l rust -C 6 -w >&2
  exit 1
fi
echo "✅ all functions are cyclomatic complexity ≤ 6"
