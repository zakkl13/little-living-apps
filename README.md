# little-living-apps — Rust agent (`lila`)

A Telegram-driven **manager agent** that delegates to ephemeral **workers**, keeps durable
git-backed **memory**, and rides a **Codex *or* Claude subscription** (never metered API billing).
One self-contained binary per host instance. This is the Rust reimplementation of the original
TypeScript agent — same architecture, idiomatic async Rust. (The original TypeScript implementation
is preserved on the `legacy-typescript` branch.)

## Architecture

A single async task owns all mutable manager state; producers feed it over channels, so **turns are
serialized** (the core invariant that keeps memory + transcript coherent without locks).

```
Telegram long-poll ─┐
                    ├─► [serialized loop] ─► ManagerDriver ─► backend (Codex│Claude CLI)
worker completions ─┘         │                                   │ MCP (HTTP+bearer)
                              ▼                                    ▼
                        snapshot (crash-safe)              Lila MCP server  ── memory_* tools
                                                           (rmcp + axum)    └─ subagent_start ─► Orchestrator ─► ephemeral workers
```

- **Manager** has "no hands": its only tools are the loopback **Lila MCP server** (`memory_*` +
  `subagent_start`). Shell/web off, read-only sandbox.
- **Workers** are single-shot: born for one objective, report back once as an event, then gone.
- **Memory** is a `/memories` git repo of markdown + a derived SQLite FTS5 index.
- **Lossless restart**: backend session id + queue + usage are snapshotted after every turn.
- **Billing guard**: the bot refuses to start if the active backend's API key is set (it would flip
  off the subscription onto metered billing); those keys are stripped from every spawned CLI's env.

| Concern | Crate |
|---|---|
| async runtime | `tokio` |
| Codex backend | `codex-client-sdk` |
| Claude backend | `claude-agent-sdk-rust` |
| MCP server | `rmcp` + `axum` |
| memory | `rusqlite` (FTS5) + shell-out `git` |
| transport | `reqwest` |
| logging | `tracing` |

## CLI

The binary is CLI-first (`lila <command>`): `run` is the daemon; the rest are host stand-up / day-2
ops that double as the integration-test surface.

```
lila run             # the long-lived manager daemon (Telegram long-poll + serialized loop)
lila config-check    # validate env + billing guard (exits non-zero on error)
lila doctor          # config + backend CLI availability
lila status          # persisted runtime state from the snapshot
lila backend [codex|claude]   # show/persist the active backend
lila memory view <path> | search <query>   # inspect/repair memory
lila mcp serve       # run the Lila MCP server standalone (debugging)
```

## Build & run

Requires the `codex` and/or `claude` CLI on `PATH`, authenticated to your subscription.

```sh
cargo build --release
TELEGRAM_BOT_TOKEN=… ALLOWED_USER_IDS=123456 ./target/release/lila run
```

Key env vars: `AGENT_BACKEND` (`codex`|`claude`), `TELEGRAM_BOT_TOKEN`, `ALLOWED_USER_IDS`,
`WORKSPACE_DIR`, `MEMORY_DIR`, `MANAGER_STATE_DIR`, `MANAGER_REASONING_EFFORT`, `LILA_DOMAIN`,
`LOG_LEVEL`. See `src/config.rs` for the full set and defaults.

## Testing

Integration coverage is the headline metric — tests drive the **compiled binary** through its CLI
against a hermetic fake Telegram server + a scripted fake backend (no subscription needed).

```sh
cargo test                 # unit + binary-driven e2e + MCP integration (all hermetic)
cargo test --test live_codex -- --ignored --nocapture   # LIVE: real Codex model + MCP attach
```

The fake backend is inert in production — it only activates when `LILA_FAKE_BACKEND` is set.

## Quality gates (CI)

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings` (`#![forbid(unsafe_code)]`; no `unwrap`/`expect` in
  non-test code)
- `scripts/check-complexity.sh` — **cyclomatic complexity ≤ 6** for every function (via `lizard`)
- `cargo-deny` + `cargo-audit` (advisories / licenses / supply chain)

## Deploy

`deploy/deploy-rs.sh` builds a static musl binary, ships it via S3, and installs the
`deploy/lila-rs@.service` systemd template unit over SSM — standing the Rust instance up
side-by-side with the live TS instances for UAT. See the script header for prerequisites.

## Principles

No `unsafe`. No global mutable state (owned values + channels; cross-task sharing is explicit
`Arc`/channels). Obsess over integration tests against the real binary.
