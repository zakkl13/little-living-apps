# Codex Rust SDK

[English](README.md) | [中文](README_zh.md)

Rust SDK for embedding the Codex agent in applications by driving the `codex` CLI over JSONL (`codex exec --experimental-json`).

## Table of Contents

- [Overview](#overview)
- [Status](#status)
- [Installation](#installation)
- [Authentication and Environment Setup](#authentication-and-environment-setup)
- [Quickstart](#quickstart)
- [API Selection Guide](#api-selection-guide)
- [Core API Surface](#core-api-surface)
- [Feature Highlights](#feature-highlights)
- [Feature Comparison with Official TypeScript SDK](#feature-comparison-with-official-typescript-sdk)
- [Compatibility Matrix](#compatibility-matrix)
- [Known Limitations](#known-limitations)
- [Testing and Validation](#testing-and-validation)
- [Concurrency Model](#concurrency-model)
- [Development](#development)
- [Contributing](#contributing)
- [License](#license)

## Overview

This crate is a parity-focused Rust implementation aligned with the official Codex TypeScript SDK semantics.

It supports:

- Thread-based multi-turn workflows (`start_thread`, `resume_thread`)
- Buffered and streamed turn execution (`run`, `run_streamed`)
- Structured output via JSON Schema (`--output-schema`)
- Multimodal input (text + local images)
- Cancellation and thread resume
- CLI config/env forwarding (`--config`, API endpoint/key, sandbox/approval/web-search settings)

## Status

- Package version: `0.107.0` (`codex-client-sdk`)
- Scope: parity-focused SDK implementation for core Codex workflows
- Validation: crate tests pass (`cargo test -p codex-client-sdk`)
- Rust docs: public API is documented and checked with `missing_docs`

## Installation

This repository currently uses a workspace/local package layout.

```toml
[dependencies]
codex = { package = "codex-client-sdk", path = "../../crates/codex" }
```

Runtime prerequisites:

- Rust 1.85+ (edition 2024)
- Codex CLI installed and available (`codex`), typically from `@openai/codex`

## Authentication and Environment Setup

The SDK invokes the external Codex CLI process. Authentication/config can be supplied either by environment variables or by `CodexOptions`.

### Option A: environment variables

```bash
export CODEX_API_KEY="<your_api_key>"
# Optional: override endpoint
export OPENAI_BASE_URL="https://api.openai.com/v1"
```

### Option B: programmatic overrides

```rust,no_run
use codex::{Codex, CodexOptions};

# fn example() -> codex::Result<()> {
let codex = Codex::new(Some(CodexOptions {
    api_key: Some("<your_api_key>".to_string()),
    base_url: Some("https://api.openai.com/v1".to_string()),
    ..Default::default()
}))?;
# let _ = codex;
# Ok(())
# }
```

Security note: do not hard-code or commit secrets to source control.

## Quickstart

```rust,no_run
use codex::Codex;

# async fn example() -> codex::Result<()> {
let codex = Codex::new(None)?;
let thread = codex.start_thread(None);

let turn = thread
    .run("Diagnose the test failure and propose a fix", None)
    .await?;

println!("final response: {}", turn.final_response);
println!("items: {}", turn.items.len());
# Ok(())
# }
```

### Continue the same conversation

```rust,no_run
# use codex::Codex;
# async fn example() -> codex::Result<()> {
# let codex = Codex::new(None)?;
# let thread = codex.start_thread(None);
let _first = thread.run("Diagnose failure", None).await?;
let second = thread.run("Implement the fix", None).await?;
println!("{}", second.final_response);
# Ok(())
# }
```

### Stream events during a turn

```rust,no_run
use codex::{Codex, ThreadEvent};
use futures::StreamExt;

# async fn example() -> codex::Result<()> {
let codex = Codex::new(None)?;
let thread = codex.start_thread(None);
let streamed = thread.run_streamed("Diagnose the failure", None).await?;

let mut events = streamed.events;
while let Some(event) = events.next().await {
    match event? {
        ThreadEvent::ItemCompleted { item } => println!("item: {:?}", item),
        ThreadEvent::TurnCompleted { usage } => println!("usage: {:?}", usage),
        _ => {}
    }
}
# Ok(())
# }
```

### Structured output

```rust,no_run
use codex::{Codex, TurnOptions};
use serde_json::json;

# async fn example() -> codex::Result<()> {
let codex = Codex::new(None)?;
let thread = codex.start_thread(None);

let schema = json!({
    "type": "object",
    "properties": {
        "summary": { "type": "string" },
        "status": { "type": "string", "enum": ["ok", "action_required"] }
    },
    "required": ["summary", "status"],
    "additionalProperties": false
});

let turn = thread
    .run(
        "Summarize repository status",
        Some(TurnOptions {
            output_schema: Some(schema),
            ..Default::default()
        }),
    )
    .await?;

println!("{}", turn.final_response);
# Ok(())
# }
```

## API Selection Guide

| Use case | Recommended API | Why |
| --- | --- | --- |
| Need only final answer/items | `Thread::run` | Simple call, returns `Turn` directly |
| Need progress events/tool/file updates | `Thread::run_streamed` | Streamed typed events (`ThreadEvent`) |
| Continue prior thread by ID | `Codex::resume_thread` + `run`/`run_streamed` | Restores conversation context |
| Need text + images in one turn | `Input::Entries(Vec<UserInput>)` | Matches CLI `--image` flow |

## Core API Surface

- `Codex`
  - `new`
  - `start_thread`
  - `resume_thread`
- `Thread`
  - `id`
  - `run`
  - `run_streamed`
- Input types
  - `Input` (`Text`, `Entries`)
  - `UserInput` (`Text`, `LocalImage`)
- Options
  - `CodexOptions`
  - `ThreadOptions`
  - `TurnOptions`
- Event/item models
  - `ThreadEvent` + typed event payloads
  - `ThreadItem` + typed item payloads
- Low-level execution
  - `CodexExec`
  - `CodexExecArgs`

## Feature Highlights

- Robust CLI binary discovery (PATH, local `node_modules`, vendor, common globals)
- Typed event/item deserialization from JSONL stream
- Output schema temp-file lifecycle managed automatically
- Config object flattening to repeated TOML-compatible `--config` flags
- Explicit precedence behavior for overlapping options (for example `web_search_mode` over `web_search_enabled`)

## Feature Comparison with Official TypeScript SDK

| Feature | Official TypeScript SDK | This Rust SDK | Notes |
| --- | --- | --- | --- |
| Start/resume threads | ✅ | ✅ | `startThread` / `resumeThread` vs `start_thread` / `resume_thread` |
| Buffered turn API | ✅ (`run`) | ✅ (`run`) | Equivalent high-level behavior |
| Streamed turn API | ✅ (`runStreamed`) | ✅ (`run_streamed`) | Rust returns `futures::Stream` |
| Structured output schema | ✅ | ✅ | Rust accepts `serde_json::Value` schema |
| Multimodal input (text + local images) | ✅ | ✅ | `Input::Entries` + `UserInput::LocalImage` |
| Cancellation | ✅ (`AbortSignal`) | ✅ (`CancellationToken`) | Rust-idiomatic token |
| CLI env override | ✅ | ✅ | `CodexOptions.env` |
| Config flattening to `--config` flags | ✅ | ✅ | TOML-compatible serialization |
| All event types (thread/turn/item) | ✅ | ✅ | Full alignment with `exec_events.rs` |
| All item types (message/reasoning/etc) | ✅ | ✅ | Full alignment with CLI output |
| Schema helper integration | ✅ (Zod ecosystem) | ✅ (via `serde_json::Value`) | Rust users pass JSON Schema directly; consider `schemars` crate for derive-based schemas |
| Core SDK workflow | ✅ | ✅ | Full parity for all core use cases |

> **Note**: This Rust SDK achieves full core parity with the official TypeScript SDK. The only ecosystem differences are:
> - TypeScript has Zod integration (`zodToJsonSchema`) while Rust users pass JSON Schema directly via `serde_json::Value`
> - For Rust schema generation, consider using the [`schemars`](https://crates.io/crates/schemars) crate for derive-based JSON Schema generation

## Compatibility Matrix

| Component | Requirement / Notes |
| --- | --- |
| Rust | `1.85+` |
| Edition | `2024` |
| Codex CLI | Required; install `codex` (typically via `@openai/codex`) |
| Runtime | Tokio async runtime |
| OS support | Linux/macOS/Windows expected via CLI support |

## Known Limitations

- The SDK wraps an external CLI process; behavior also depends on installed CLI version.
- No built-in JSON-schema builder helper is provided; pass schema as `serde_json::Value`.
- Test suite is comprehensive at protocol/mock level; real CLI/live-model matrix testing is not part of this crate.

## Testing and Validation

Reference alignment coverage (TypeScript -> Rust):

- `tests/run.test.ts` -> `tests/run_tests.rs`
- `tests/runStreamed.test.ts` -> `tests/run_streamed_tests.rs`
- `tests/exec.test.ts` -> `tests/exec_tests.rs`
- `tests/abort.test.ts` -> `tests/abort_tests.rs`

Validation commands:

```bash
RUSTDOCFLAGS='-Dwarnings -Dmissing_docs' cargo doc -p codex-client-sdk --no-deps
cargo test -p codex-client-sdk
```

## Concurrency Model

- `run_streamed()` returns a `Send` stream of `ThreadEvent`
- `run()` materializes a final `Turn` by consuming streamed events
- Turn cancellation uses `tokio_util::sync::CancellationToken`

## Development

```bash
cargo test -p codex-client-sdk
cargo clippy -p codex-client-sdk --all-targets --all-features -- -D warnings
```

## Contributing

Pull requests are welcome. Before submitting, run:

```bash
cargo fmt
cargo clippy -p codex-client-sdk --all-targets --all-features -- -D warnings
cargo test -p codex-client-sdk
```

## License

Licensed under the [Apache License, Version 2.0](../../LICENSE).
