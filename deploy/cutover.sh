#!/usr/bin/env bash
# CUTOVER: replace the live TS `lila-manager` with the Rust binary on the same host/domain/bot.
# Two phases, so the destructive flip is explicit:
#
#   STAGE (default, non-destructive): install the freshly-built binary + the Rust env (fresh
#     memory/state dirs — clean slate) + the unit, and run `lila config-check`. Does NOT touch the
#     running TS manager.
#   FLIP  (CONFIRM=1): stop+disable lila-manager (TS), enable+start lila-rs-manager (Rust), verify.
#
# Run over the TS tree's SSM runner (it has the host facts):
#   /Users/zakk/lilapps/lila-rs/deploy/cutover.sh          | sprite-codex-bot/dogfooding/ssm.sh   # stage
#   CONFIRM=1 /Users/zakk/lilapps/lila-rs/deploy/cutover.sh | sprite-codex-bot/dogfooding/ssm.sh   # flip
set -euo pipefail
CONFIRM="${CONFIRM:-0}"
cat <<REMOTE
set -euo pipefail
CONFIRM=${CONFIRM}
SRC=/home/ubuntu/lila-rs/target/release/lila
BIN=/opt/lila/bin/lila
ENVF=/etc/lila/lila-rs.env

test -x "\$SRC" || { echo "ERROR: built binary missing at \$SRC — build first"; exit 1; }

echo "== install binary =="
install -d -m 0755 /opt/lila/bin
install -m 0755 "\$SRC" "\$BIN"
"\$BIN" --version || true

echo "== write Rust env (reuse TS values; FRESH memory+state dirs = clean slate) =="
# codex/claude are mise-managed (node-backed); the native unit gets no mise shell hook, so put the
# mise node bin dir (holds both \`codex\` and the \`node\` it shebangs to) on PATH explicitly.
NODEBIN=\$(sudo -u ubuntu -H bash -lc 'dirname "\$(mise which codex)"')
test -x "\$NODEBIN/codex" || { echo "ERROR: codex not found via mise (NODEBIN=\$NODEBIN)"; exit 1; }
grep -vE '^(MEMORY_DIR|MANAGER_STATE_DIR|AGENT_BACKEND|LILA_APP_SERVICE|PATH)=' /etc/lila/lila.env > "\$ENVF"
cat >> "\$ENVF" <<EOF
MEMORY_DIR=/var/lib/lila-rs/memory
MANAGER_STATE_DIR=/var/lib/lila-rs/state
AGENT_BACKEND=codex
LILA_APP_SERVICE=lila-app
PATH=\$NODEBIN:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
EOF
chmod 600 "\$ENVF"
install -d -o ubuntu -g ubuntu -m 0755 /var/lib/lila-rs /var/lib/lila-rs/memory /var/lib/lila-rs/state

echo "== install unit =="
cp /home/ubuntu/lila-rs/deploy/lila-rs-manager.service /etc/systemd/system/lila-rs-manager.service
systemctl daemon-reload

echo "== config-check (billing guard + env sanity) =="
sudo -u ubuntu -H env \$(grep -vE '^#|^$' "\$ENVF" | xargs) "\$BIN" config-check

if [ "\$CONFIRM" != "1" ]; then
  echo; echo ">> STAGED OK. Re-run with CONFIRM=1 to flip the service."
  exit 0
fi

echo "== FLIP: stop TS manager, start Rust manager =="
systemctl disable --now lila-manager
systemctl enable --now lila-rs-manager
sleep 3
echo -n "lila-rs-manager: "; systemctl is-active lila-rs-manager
echo -n "lila-app (website): "; systemctl is-active lila-app
echo "== recent manager logs =="; journalctl -u lila-rs-manager -n 15 --no-pager || true
echo "== inspector localhost =="
TOKEN=\$(grep -E '^INSPECTOR_TOKEN=' "\$ENVF" | cut -d= -f2 || true)
curl -s -o /dev/null -w "  no-token (expect 401): %{http_code}\n" "http://127.0.0.1:9090/api/overview" || true
[ -n "\$TOKEN" ] && curl -s -o /dev/null -w "  with-token (expect 200): %{http_code}\n" "http://127.0.0.1:9090/api/overview?t=\$TOKEN" || true
REMOTE
