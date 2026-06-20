#!/usr/bin/env bash
# READ-ONLY host discovery for the cutover. Prints exactly what we need to write the Rust manager's
# env + unit and to plan the swap, and changes nothing. Pipe it through the TS tree's SSM runner:
#
#   /Users/zakk/lilapps/lila-rs/deploy/discover.sh | /Users/zakk/lilapps/sprite-codex-bot/dogfooding/ssm.sh
#
# (the runner reads IID/REGION from dogfooding/host.env). Run this first, once `aws` is re-authed.
set -euo pipefail
cat <<'REMOTE'
set -uo pipefail
echo "== arch / mem =="; uname -m; free -m | head -2
echo; echo "== current manager service =="; systemctl is-active lila-manager 2>/dev/null; systemctl cat lila-manager 2>/dev/null | sed -n '1,30p'
echo; echo "== manager env (/etc/lila/lila.env), tokens masked =="
sudo sed -E 's/(TOKEN|KEY)=.*/\1=<redacted>/' /etc/lila/lila.env 2>/dev/null
echo; echo "== workspace / app dir (where the Rails app the manager maintains lives) =="
echo "WORKSPACE_DIR from env:"; sudo grep -E '^WORKSPACE_DIR=' /etc/lila/lila.env 2>/dev/null || echo "  (not set — using default)"
for d in /var/lib/lila/workspace /home/ubuntu/little-living-apps/workspace /var/lib/lila; do
  [ -d "$d" ] && { echo "  $d:"; ls -1 "$d" 2>/dev/null | head; }
done
echo; echo "== app (puma) service(s) =="; systemctl list-units --type=service --all 2>/dev/null | grep -iE 'puma|rails|lila-app|app@' | head
echo; echo "== memory + state dirs (will be RESET on cutover — clean slate) =="
for d in /var/lib/lila/memory /var/lib/lila/state; do echo "  $d:"; sudo ls -1 "$d" 2>/dev/null | head; done
echo; echo "== Caddyfile (the /_inspect + app proxy we must preserve) =="; sudo sed -n '1,60p' /etc/caddy/Caddyfile 2>/dev/null
echo; echo "== rust toolchain present? =="; sudo -u ubuntu bash -lc 'command -v cargo && cargo --version' 2>/dev/null || echo "  (no cargo — will install rustup on the box)"
echo; echo "== codex/claude auth homes (workers ride these) =="; sudo ls -1 /var/lib/lila/codex 2>/dev/null | head -3; sudo ls -1d /home/ubuntu/.claude 2>/dev/null
REMOTE
