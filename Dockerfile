FROM rust:1-bookworm AS builder

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY vendor ./vendor
COPY src ./src
RUN cargo build --release --bin lila

FROM docker:29-cli AS docker-cli

FROM ruby:3.3-bookworm

ENV DEBIAN_FRONTEND=noninteractive \
    LILA_ASSETS_DIR=/opt/lila \
    NODE_PATH=/opt/lila/tooling/node_modules \
    PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PATH=/opt/lila/bin:/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
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
    && npx playwright install chromium \
    && npx playwright install-deps chromium \
    && rm -rf /var/lib/apt/lists/*

RUN gem install rails -v '~> 8.0' --no-document

COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=builder /src/target/release/lila /opt/lila/bin/lila
COPY bin /opt/lila/bin
COPY stacks /opt/lila/stacks
COPY design /opt/lila/design

WORKDIR /workspace
CMD ["lila", "--help"]
