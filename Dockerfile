FROM rust:1-bookworm AS builder

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY vendor ./vendor
COPY src ./src
RUN cargo build --release --bin lila

FROM ruby:3.3-bookworm

ENV DEBIAN_FRONTEND=noninteractive \
    NODE_PATH=/opt/lila/tooling/node_modules \
    PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PATH=/opt/lila/bin:/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        cmake \
        curl \
        git \
        gnupg \
        build-essential \
        pkg-config \
        libffi-dev \
        libssl-dev \
        libyaml-dev \
        nodejs \
        npm \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g @openai/codex @anthropic-ai/claude-code \
    && mkdir -p /opt/lila/tooling \
    && cd /opt/lila/tooling \
    && npm init -y >/dev/null \
    && npm install playwright \
    && npx playwright install chromium

COPY --from=builder /src/target/release/lila /opt/lila/bin/lila
COPY stacks /opt/lila/stacks
COPY design /opt/lila/design

WORKDIR /workspace
CMD ["lila", "--help"]
