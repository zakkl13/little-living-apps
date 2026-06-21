#!/usr/bin/env bash
# Build the `lila` release binary NATIVELY on the live Graviton (aarch64) host — no cross-compile.
# Idempotent: ensures rustup + build deps + swap, clones/updates the repo, and (re)builds the binary.
# Pipe through the SSM runner (it has the host facts):
#
#   BRANCH=main /Users/zakk/lilapps/lila-rs/deploy/build-on-box.sh | lila-rs/dogfooding/ssm.sh
#
# Defaults to the `main` branch (post-promotion). Pass BRANCH=rust-rewrite during the transition.
set -euo pipefail
BRANCH="${BRANCH:-main}"
REPO_URL="${REPO_URL:-https://github.com/zakkl13/little-living-apps.git}"
cat <<REMOTE
set -uo pipefail
BRANCH=${BRANCH}
REPO_URL=${REPO_URL}

echo "== swap (release LTO build is memory-heavy on 2GB) =="
if ! swapon --show | grep -q .; then
  fallocate -l 4G /swapfile && chmod 600 /swapfile && mkswap /swapfile >/dev/null && swapon /swapfile && echo "added 4G swap"
else echo "swap present"; fi

echo "== build deps (gcc/cmake for bundled sqlite + aws-lc-rs) =="
apt-get update -qq && apt-get install -y -qq build-essential cmake pkg-config >/dev/null 2>&1 && echo "deps ok"

echo "== rustup for ubuntu =="
sudo -u ubuntu -H bash -c '
  set -e
  [ -x "\$HOME/.cargo/bin/cargo" ] || curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null 2>&1
  "\$HOME/.cargo/bin/rustc" --version
'

echo "== clone/update \$BRANCH =="
sudo -u ubuntu -H bash -c '
  set -e
  cd "\$HOME"
  if [ -d lila-rs/.git ]; then
    git -C lila-rs fetch -q origin "'"\$BRANCH"'" && git -C lila-rs checkout -q "'"\$BRANCH"'" && git -C lila-rs reset -q --hard "origin/'"\$BRANCH"'"
  else
    git clone -q --branch "'"\$BRANCH"'" --depth 1 "'"\$REPO_URL"'" lila-rs
  fi
  git -C lila-rs log --oneline -1
'

echo "== release build (foreground; ~7 min) =="
sudo -u ubuntu -H bash -c 'cd "\$HOME/lila-rs" && CARGO_BUILD_JOBS=2 "\$HOME/.cargo/bin/cargo" build --release --bin lila'
ls -la /home/ubuntu/lila-rs/target/release/lila
REMOTE
