#!/usr/bin/env bash
# Deploy the Rust manager to a host over SSM (no SSH). Ships a prebuilt static musl binary via S3
# (a multi-MB binary can't go inline through SSM), and stands the Rust instance up SIDE-BY-SIDE with
# the live TS instances (its own bot token / env / domain) so UAT never touches them.
#
# Prereqs (export, or put in a sourced deploy/host.env):
#   IID       EC2 instance id            REGION    AWS region
#   INSTANCE  instance name (e.g. rs)    BUCKET    an S3 bucket the box can read
#   TARGET    {x86_64,aarch64}-unknown-linux-musl  (match the box's arch)
# And /etc/lila/$INSTANCE.env must already exist on the box (bot token, ALLOWED_USER_IDS, dirs, …).
#
#   deploy/deploy-rs.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
[ -f "$HERE/host.env" ] && source "$HERE/host.env"
: "${IID:?set IID}"; : "${REGION:?set REGION}"; : "${BUCKET:?set BUCKET}"
: "${INSTANCE:=rs}"; : "${TARGET:=aarch64-unknown-linux-musl}"
SERVICE="lila-rs-manager@${INSTANCE}"
KEY="lila/${INSTANCE}/lila"

echo ">> Building static binary ($TARGET)…"
rustup target add "$TARGET" >/dev/null 2>&1 || true
( cd "$ROOT" && cargo build --release --target "$TARGET" )
BIN="$ROOT/target/$TARGET/release/lila"

echo ">> Uploading binary to s3://$BUCKET/$KEY…"
aws s3 cp --region "$REGION" "$BIN" "s3://$BUCKET/$KEY"

UNIT_B64="$(base64 < "$HERE/lila-rs@.service" | tr -d '\n')"
SCRIPT="$(cat <<REMOTE
set -euo pipefail
sudo install -d -m 0755 /opt/lila/bin
aws s3 cp --region ${REGION} s3://${BUCKET}/${KEY} /opt/lila/bin/lila
sudo chmod 0755 /opt/lila/bin/lila
printf '%s' '${UNIT_B64}' | base64 -d | sudo tee /etc/systemd/system/lila-rs-manager@.service >/dev/null
sudo systemctl daemon-reload
# Sanity: the binary validates its own config (incl. the billing guard) before we (re)start it.
sudo -E env \$(grep -v '^#' /etc/lila/${INSTANCE}.env | xargs) /opt/lila/bin/lila config-check
sudo systemctl enable --now ${SERVICE}
sudo systemctl restart ${SERVICE}
sleep 3
echo -n "status: "; systemctl is-active ${SERVICE}
journalctl -u ${SERVICE} -n 12 --no-pager || true
REMOTE
)"

echo ">> Running install on $IID over SSM…"
CMD_ID="$(aws ssm send-command --region "$REGION" --instance-ids "$IID" \
  --document-name AWS-RunShellScript \
  --parameters commands="$(printf '%s' "$SCRIPT" | python3 -c 'import json,sys; print(json.dumps([sys.stdin.read()]))')" \
  --query 'Command.CommandId' --output text)"
echo ">> SSM command: $CMD_ID"
echo "   poll: aws ssm get-command-invocation --region $REGION --instance-id $IID --command-id $CMD_ID --query StandardOutputContent --output text"
