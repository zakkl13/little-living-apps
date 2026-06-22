# Codex Rust SDK

[English](README.md) | [中文](README_zh.md)

通过 `codex` CLI（`codex exec --experimental-json`）的 JSONL 通道，将 Codex agent 以 Rust SDK 方式集成到应用中。

## 目录

- [概览](#概览)
- [状态](#状态)
- [安装](#安装)
- [认证与环境配置](#认证与环境配置)
- [快速开始](#快速开始)
- [API 选型指南](#api-选型指南)
- [核心 API](#核心-api)
- [关键实现点](#关键实现点)
- [与官方 TypeScript SDK 的特性对比](#与官方-typescript-sdk-的特性对比)
- [兼容性矩阵](#兼容性矩阵)
- [已知限制](#已知限制)
- [测试与验证](#测试与验证)
- [并发模型](#并发模型)
- [开发](#开发)
- [贡献](#贡献)
- [许可证](#许可证)

## 概览

该 crate 是一个以能力对齐为目标的 Rust 实现，语义上与官方 Codex TypeScript SDK 保持一致。

支持能力：

- 基于线程的多轮会话（`start_thread`、`resume_thread`）
- 缓冲与流式两种执行方式（`run`、`run_streamed`）
- 基于 JSON Schema 的结构化输出（`--output-schema`）
- 多模态输入（文本 + 本地图片）
- 取消机制与线程恢复
- CLI 配置与环境透传（`--config`、API 地址/密钥、sandbox/approval/web-search）

## 状态

- 版本：`0.107.0`（`codex-client-sdk`）
- 范围：覆盖 Codex 核心工作流的对齐实现
- 验证：测试通过（`cargo test -p codex-client-sdk`）
- 文档：公开 API 已补齐 rustdoc，并可通过 `missing_docs` 检查

## 安装

当前仓库采用 workspace / 本地路径依赖方式。

```toml
[dependencies]
codex = { package = "codex-client-sdk", path = "../../crates/codex" }
```

运行前提：

- Rust 1.85+（edition 2024）
- 已安装并可访问 Codex CLI（`codex`，通常来自 `@openai/codex`）

## 认证与环境配置

该 SDK 本质上是调用外部 Codex CLI。认证配置可通过环境变量或 `CodexOptions` 提供。

### 方式 A：环境变量

```bash
export CODEX_API_KEY="<your_api_key>"
# 可选：覆盖接口地址
export OPENAI_BASE_URL="https://api.openai.com/v1"
```

### 方式 B：代码中覆盖

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

安全提示：不要将密钥硬编码或提交到代码仓库。

## 快速开始

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

### 在同一线程继续对话

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

### 流式消费事件

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

### 结构化输出

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

## API 选型指南

| 场景 | 推荐 API | 原因 |
| --- | --- | --- |
| 只关心最终结果 | `Thread::run` | 直接返回 `Turn`，调用简单 |
| 需要实时进度/工具/文件变化 | `Thread::run_streamed` | 流式返回类型化事件 |
| 按线程 ID 继续对话 | `Codex::resume_thread` + `run`/`run_streamed` | 恢复上下文 |
| 一次输入文本+图片 | `Input::Entries(Vec<UserInput>)` | 与 CLI `--image` 语义一致 |

## 核心 API

- `Codex`
  - `new`
  - `start_thread`
  - `resume_thread`
- `Thread`
  - `id`
  - `run`
  - `run_streamed`
- 输入类型
  - `Input`（`Text`、`Entries`）
  - `UserInput`（`Text`、`LocalImage`）
- 配置
  - `CodexOptions`
  - `ThreadOptions`
  - `TurnOptions`
- 事件与条目模型
  - `ThreadEvent` 及其类型化 payload
  - `ThreadItem` 及其类型化 payload
- 底层执行
  - `CodexExec`
  - `CodexExecArgs`

## 关键实现点

- CLI 路径发现较健壮（PATH、本地 `node_modules`、vendor、常见全局目录）
- JSONL 事件/条目均采用强类型反序列化
- `--output-schema` 临时文件生命周期自动管理
- `config` 对象可展开为重复的 TOML 兼容 `--config` 参数
- 对重叠选项有明确优先级（例如 `web_search_mode` 高于 `web_search_enabled`）

## 与官方 TypeScript SDK 的特性对比

| 特性 | 官方 TypeScript SDK | 本 Rust SDK | 说明 |
| --- | --- | --- | --- |
| 新建/恢复线程 | ✅ | ✅ | `startThread` / `resumeThread` 对应 `start_thread` / `resume_thread` |
| 缓冲式回合 API | ✅（`run`） | ✅（`run`） | 高层语义一致 |
| 流式回合 API | ✅（`runStreamed`） | ✅（`run_streamed`） | Rust 返回 `futures::Stream` |
| 结构化输出 | ✅ | ✅ | Rust 侧传入 `serde_json::Value` schema |
| 多模态输入（文本+本地图） | ✅ | ✅ | `Input::Entries` + `UserInput::LocalImage` |
| 取消机制 | ✅（`AbortSignal`） | ✅（`CancellationToken`） | Rust 风格原语 |
| 环境变量覆盖 | ✅ | ✅ | `CodexOptions.env` |
| `--config` 展平透传 | ✅ | ✅ | TOML 兼容序列化 |
| 全部事件类型（thread/turn/item） | ✅ | ✅ | 与 `exec_events.rs` 完整对齐 |
| 全部条目类型（message/reasoning 等） | ✅ | ✅ | 与 CLI 输出完整对齐 |
| Schema 辅助集成 | ✅（Zod 生态） | ✅（通过 `serde_json::Value`） | Rust 直接传 JSON Schema；可结合 `schemars` 生成 |
| 核心 SDK 工作流 | ✅ | ✅ | 核心用例已实现完整对齐 |

> **说明**：该 Rust SDK 与官方 TypeScript SDK 已实现核心能力完整对齐。主要差异仅在生态层：
> - TypeScript 侧常见 Zod 集成（`zodToJsonSchema`），Rust 侧直接通过 `serde_json::Value` 传 JSON Schema
> - Rust 若需 derive 式 schema 生成，可使用 [`schemars`](https://crates.io/crates/schemars)

## 兼容性矩阵

| 组件 | 要求 / 说明 |
| --- | --- |
| Rust | `1.85+` |
| Edition | `2024` |
| Codex CLI | 必需（通常通过 `@openai/codex` 安装） |
| Runtime | Tokio 异步运行时 |
| 操作系统 | 依赖 Codex CLI 支持矩阵 |

## 已知限制

- SDK 依赖外部 CLI，最终行为会受安装的 CLI 版本影响。
- 不提供内建 JSON Schema 构建器；请直接传入 `serde_json::Value`。
- 当前测试以协议/模拟链路为主，不包含完整 live-model 矩阵测试。

## 测试与验证

参考对齐映射（TypeScript -> Rust）：

- `tests/run.test.ts` -> `tests/run_tests.rs`
- `tests/runStreamed.test.ts` -> `tests/run_streamed_tests.rs`
- `tests/exec.test.ts` -> `tests/exec_tests.rs`
- `tests/abort.test.ts` -> `tests/abort_tests.rs`

验证命令：

```bash
RUSTDOCFLAGS='-Dwarnings -Dmissing_docs' cargo doc -p codex-client-sdk --no-deps
cargo test -p codex-client-sdk
```

## 并发模型

- `run_streamed()` 返回 `Send` 的 `ThreadEvent` 流
- `run()` 通过消费事件流构建最终 `Turn`
- 取消机制基于 `tokio_util::sync::CancellationToken`

## 开发

```bash
cargo test -p codex-client-sdk
cargo clippy -p codex-client-sdk --all-targets --all-features -- -D warnings
```

## 贡献

欢迎提交 PR。提交前请执行：

```bash
cargo fmt
cargo clippy -p codex-client-sdk --all-targets --all-features -- -D warnings
cargo test -p codex-client-sdk
```

## 许可证

本项目采用 [Apache License, Version 2.0](../../LICENSE) 许可。
