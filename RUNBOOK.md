# RUNBOOK — provision a host and bootstrap a little living app

A reproducible, end-to-end recipe for standing up the manager + one app on a fresh AWS host. Written
so future-you (or anyone) can do it again for another app without rediscovering the gotchas. Each
step has a **Why** and a **Learnings** note; the running list of gotchas is at the bottom.

> Reference deployment: `LilaHostStack` in `~/code/zakk-projects/_infra`, host `lillivinapps.zakk.io`,
> account `831092627739`, region `us-east-1`.

---

## 0. Prerequisites (on your laptop)
- AWS CLI v2 configured with creds for the target account (`aws sts get-caller-identity` works).
- The **Session Manager plugin** for the AWS CLI (for interactive shells):
  `aws ssm start-session` requires it — install per AWS docs if `start-session` errors.
- Node + CDK in the infra repo (`npx cdk --version`).
- Your secrets ready: `TELEGRAM_BOT_TOKEN` (@BotFather), `ALLOWED_USER_IDS` (@userinfobot),
  `ANTHROPIC_API_KEY`. **Never** an `OPENAI_API_KEY` / `CODEX_API_KEY`.

---

## 1. Provision the host (CDK)

```bash
cd ~/code/zakk-projects/_infra
npx cdk diff LilaHostStack
npx cdk deploy LilaHostStack       # answer 'y' to the security prompt (see Learnings)
```

The stack creates: a VPC (1 public subnet, **no NAT**), a `t4g.small` Ubuntu 24.04 arm64 instance
(16 GB gp3, 4 GB swap via user-data), a security group (**only 80/443 in**; admin via SSM, no SSH),
an IAM role (`AmazonSSMManagedInstanceCore`), an **Elastic IP**, and a Route53 A-record
`lillivinapps.zakk.io → EIP`.

Grab the outputs (you'll use `InstanceId` everywhere):
```bash
aws cloudformation describe-stacks --stack-name LilaHostStack --region us-east-1 \
  --query "Stacks[0].Outputs" --output table
```

**Why:** one always-on box is the whole substrate. The EIP keeps DNS/admin stable across stop/start;
no NAT keeps cost down (the instance egresses directly via the public subnet + IGW).

**Learnings:**
- **The deploy needs an interactive `y`.** Because the stack adds an IAM role + 80/443 ingress, CDK
  prints a security-confirmation prompt. If you don't answer `y`, the change set is created but
  **not executed**, and the stack sits in **`REVIEW_IN_PROGRESS`** with zero resources — looking
  "deployed" but doing nothing. Fix: just run `cdk deploy` again and confirm (or
  `--require-approval never` in CI). Verify it actually finished with the `describe-stacks` status
  = `CREATE_COMPLETE`.

---

## 2. Connect to the box (SSM, no SSH)

Interactive shell:
```bash
aws ssm start-session --target <InstanceId> --region us-east-1
```
Drive it non-interactively (what we use to script bootstrap) with `aws ssm send-command` against the
`AWS-RunShellScript` document; poll `aws ssm get-command-invocation` for output.

**Why:** SSM means no key pair to manage and no open port 22. The Ubuntu 24.04 AMI ships the SSM
agent, and the instance's IAM role grants it — it registers automatically.

Confirm it's manageable, then you can drive it non-interactively (no interactive shell needed):
```bash
aws ssm describe-instance-information --region us-east-1 \
  --filters "Key=InstanceIds,Values=<InstanceId>" \
  --query "InstanceInformationList[].PingStatus" --output text   # -> Online
```

To script remote steps, we use a tiny helper that runs a script (from stdin) on the box via
`AWS-RunShellScript` and prints status+stdout+stderr — see `~/.lila/ssm-run.sh` (base64-pipes the
script so multi-line/quoting is safe). Usage: `echo 'whoami' | ~/.lila/ssm-run.sh`.

**Learnings:**
- The instance was `Online` in SSM within ~1 min of `CREATE_COMPLETE` — no extra agent install
  needed on Ubuntu 24.04.
- **`send-command` runs as `root` with no `SUDO_USER`.** `bootstrap.sh` derives the service user
  from `$SUDO_USER` and refuses to run as root, so when driving it over SSM you must pass it
  explicitly: `SERVICE_USER=ubuntu bash bootstrap.sh`. (Interactive `sudo bash bootstrap.sh` works
  without this.) On the Ubuntu AMI the default human user is `ubuntu` (uid 1000).
- Host facts to expect: `aarch64` (Graviton), default user `ubuntu`, the 4 GB swap file from
  user-data is active at boot (`swapon --show`).

---

## 3. Get the code onto the box (public GitHub clone)

The framework lives at **github.com/zakkl13/little-living-apps** (public), so the box clones with no
auth:
```bash
echo 'sudo -u ubuntu git clone --depth=1 https://github.com/zakkl13/little-living-apps.git /home/ubuntu/little-living-apps' | ~/.lila/ssm-run.sh
```
Clone **as `ubuntu`** (not root) so the repo is owned by the service user. Re-running? `git -C <dir>
pull --ff-only` instead.

**Why:** a public clone is the simplest reproducible delivery and doubles as the OSS publish. For a
private repo you'd use a read-only deploy key or an S3 presigned tarball instead.

---

## 4. Bootstrap the manager

Long step (mise **compiles Ruby** from source on ARM), so launch it **detached** and tail the log
rather than blocking an SSM call:
```bash
cat <<'SH' | ~/.lila/ssm-run.sh
REPO=/home/ubuntu/little-living-apps
[ -f "$REPO/.env" ] || sudo -u ubuntu cp "$REPO/.env.example" "$REPO/.env"   # template; fill secrets later
: > /var/log/lila-bootstrap.log; chmod 666 /var/log/lila-bootstrap.log
setsid bash -c "SERVICE_USER=ubuntu bash $REPO/bootstrap.sh" > /var/log/lila-bootstrap.log 2>&1 < /dev/null &
echo "launched pid=$!"
SH
# then poll: echo 'tail -5 /var/log/lila-bootstrap.log; pgrep -f bootstrap.sh >/dev/null && echo RUNNING || echo DONE' | ~/.lila/ssm-run.sh
```
Installs mise (Ruby+Node), the Codex CLI, builds the manager, creates data dirs + workspace, installs
the `lila-manager` systemd unit (enabled, not started until Codex auth exists), and drops
`lila-new-app` on PATH. Idempotent.

_(build time / memory notes: filled in after the run completes.)_

---

## 5. Secrets + Codex auth + start (interactive, one-time)

This is the hands-on phase — secrets and the device-auth flow shouldn't go through scripted SSM
(they'd land in command logs). Open an interactive shell: `aws ssm start-session --target
<InstanceId> --region us-east-1`, then:

```bash
# 1) fill the three secrets (bootstrap installed a template from .env.example)
sudo nano /etc/lila/lila.env       # TELEGRAM_BOT_TOKEN, ALLOWED_USER_IDS, ANTHROPIC_API_KEY

# 2) authenticate Codex on the ChatGPT subscription (NOT an API key)
sudo -u ubuntu -H CODEX_HOME=/var/lib/lila/codex ~ubuntu/.local/bin/mise exec -- codex login --device-auth
#    -> open the printed URL, enter the code, approve in your browser

# 3) start the manager
sudo systemctl restart lila-manager
journalctl -u lila-manager -f      # expect "little-living-apps (v0.2 manager) ready"
```
Then message your bot on Telegram. `CODEX_HOME` persists on the VM disk, so this auth survives
reboots.

_(TBD: confirm the exact device-auth UX over an SSM session.)_

---

## 6. Build the app
```bash
lila-new-app          # scaffolds the minimal Rails 8 + PWA app, runs it under systemd (reload mode)
```
The app runs in reload mode bound to `127.0.0.1:3000` — private to the box until Caddy fronts it.

## 7. Publish it on the domain (Caddy, auto-HTTPS) — part of bootstrap

Caddy is the TLS terminator that puts the app on your domain (single binary, auto-renewing Let's
Encrypt cert). **`bootstrap.sh` now installs + configures it automatically when `LILA_DOMAIN` is
set** — set it in `.env` (or `/etc/lila/lila.env`) and bootstrap does the rest:
```bash
echo 'LILA_DOMAIN=lillivinapps.zakk.io' >> /home/ubuntu/little-living-apps/.env
sudo LILA_DOMAIN=lillivinapps.zakk.io bash /home/ubuntu/little-living-apps/bootstrap.sh   # idempotent
```
It adds Caddy's official apt repo (it's **not** in the stock Ubuntu repos), writes
`/etc/caddy/Caddyfile` with the domain baked in (the apt-managed `caddy.service` reads that file and
has no env vars, so the domain must be literal — not `{$LILA_DOMAIN}`), and reloads Caddy. The
A-record already points at the EIP and the security group allows 80/443, so the cert issues on the
first request and `https://lillivinapps.zakk.io` serves the app (gated by Rails' built-in auth).

**Learnings:**
- `apt-get install caddy` alone fails — Caddy ships via its own Cloudsmith apt repo.
- The Caddyfile env-var form `{$LILA_DOMAIN:localhost}` resolves to `localhost` under the apt
  `caddy.service` (no env), so bootstrap substitutes the real domain into `/etc/caddy/Caddyfile`.

---

## Running list of gotchas / learnings
1. **CDK `REVIEW_IN_PROGRESS` = unexecuted change set.** The security prompt wasn't answered `y`;
   re-run `cdk deploy`. (See step 1.)
