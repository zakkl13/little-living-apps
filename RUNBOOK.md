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
agent, and the instance's IAM role grants it — it registers automatically a minute or two after boot.

**Learnings:**
- _(to fill in as we go: how long until the instance shows up in `aws ssm describe-instance-information`)_

---

## 3. Get the code onto the box
_(method TBD — GitHub clone vs. S3 presigned tarball; will record the chosen canonical path here.)_

---

## 4. Configure secrets (`.env`)
_(TBD: create `.env` from `.env.example` with the three tokens; how secrets are handled securely.)_

---

## 5. Bootstrap the manager
```bash
sudo bash bootstrap.sh
```
Installs mise (Ruby+Node), the Codex CLI, builds the manager, creates data dirs + workspace, installs
the `lila-manager` systemd unit. Idempotent.

_(TBD: record build time / memory pressure on t4g.small, any apt or mise surprises.)_

---

## 6. Authenticate Codex (interactive, one-time)
```bash
sudo -u <user> -H CODEX_HOME=/var/lib/lila/codex ~/.local/bin/mise exec -- codex login --device-auth
sudo systemctl start lila-manager
journalctl -u lila-manager -f
```
Device-auth prints a URL + code; open it in a browser and approve. `CODEX_HOME` persists on disk.

_(TBD: confirm the device-auth flow over SSM; capture exact command.)_

---

## 7. Build the app + publish it
```bash
lila-new-app                                  # scaffolds the minimal Rails 8 + PWA app, runs it (reload mode)
sudo apt-get install -y caddy
sudo LILA_DOMAIN=lillivinapps.zakk.io caddy run --config deploy/Caddyfile   # auto-HTTPS for the domain
```
The A-record already points at the EIP, so Caddy's Let's Encrypt challenge succeeds on first run.

_(TBD: confirm Caddy install path on Ubuntu 24.04; turn it into a systemd service.)_

---

## Running list of gotchas / learnings
1. **CDK `REVIEW_IN_PROGRESS` = unexecuted change set.** The security prompt wasn't answered `y`;
   re-run `cdk deploy`. (See step 1.)
