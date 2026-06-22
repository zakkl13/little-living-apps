mod helpers;

use std::collections::HashMap;

use codex::{
    ApprovalMode, CodexOptions, Input, ModelReasoningEffort, SandboxMode, ThreadItem,
    ThreadOptions, TurnOptions, UserInput, WebSearchMode,
};
use serde_json::{Value, json};

use crate::helpers::{MockCodexHarness, collect_config_values, expect_pair};

fn success_events(thread_id: Option<&str>, text: &str, item_id: &str) -> Vec<Value> {
    let mut events = Vec::new();
    if let Some(thread_id) = thread_id {
        events.push(json!({
            "type": "thread.started",
            "thread_id": thread_id
        }));
    }
    events.push(json!({ "type": "turn.started" }));
    events.push(json!({
        "type": "item.completed",
        "item": {
            "id": item_id,
            "type": "agent_message",
            "text": text
        }
    }));
    events.push(json!({
        "type": "turn.completed",
        "usage": {
            "input_tokens": 42,
            "cached_input_tokens": 12,
            "output_tokens": 5
        }
    }));
    events
}

#[tokio::test]
async fn returns_thread_events() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "Hi!", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let thread = codex.start_thread(None);
    let result = thread.run("Hello, world!", None).await.expect("run");

    assert_eq!(result.final_response, "Hi!");
    assert_eq!(result.items.len(), 1);
    assert_eq!(
        result.usage.expect("usage"),
        codex::Usage {
            input_tokens: 42,
            cached_input_tokens: 12,
            output_tokens: 5
        }
    );
    assert_eq!(thread.id().as_deref(), Some("thread_1"));

    let logs = harness.logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].stdin, "Hello, world!");
}

#[tokio::test]
async fn sends_resume_when_run_is_called_twice() {
    let harness = MockCodexHarness::new(vec![
        success_events(Some("thread_1"), "First response", "item_1"),
        success_events(None, "Second response", "item_2"),
    ]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let thread = codex.start_thread(None);
    thread.run("first input", None).await.expect("first run");
    let second = thread.run("second input", None).await.expect("second run");

    assert_eq!(second.final_response, "Second response");
    let logs = harness.logs();
    assert_eq!(logs.len(), 2);

    let second_args = &logs[1].args;
    expect_pair(second_args, ("resume", "thread_1"));
}

#[tokio::test]
async fn resumes_thread_by_id() {
    let harness = MockCodexHarness::new(vec![
        success_events(Some("thread_1"), "First response", "item_1"),
        success_events(None, "Second response", "item_2"),
    ]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let original_thread = codex.start_thread(None);
    original_thread
        .run("first input", None)
        .await
        .expect("first run");

    let resumed = codex.resume_thread(original_thread.id().expect("id"), None);
    let result = resumed.run("second input", None).await.expect("second run");

    assert_eq!(result.final_response, "Second response");
    assert_eq!(resumed.id(), original_thread.id());

    let logs = harness.logs();
    expect_pair(&logs[1].args, ("resume", "thread_1"));
}

#[tokio::test]
async fn passes_turn_options_to_exec() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let thread = codex.start_thread(Some(ThreadOptions {
        model: Some("gpt-test-1".to_string()),
        sandbox_mode: Some(SandboxMode::WorkspaceWrite),
        ..Default::default()
    }));
    thread.run("apply options", None).await.expect("run");

    let logs = harness.logs();
    let args = &logs[0].args;
    expect_pair(args, ("--model", "gpt-test-1"));
    expect_pair(args, ("--sandbox", "workspace-write"));
}

#[tokio::test]
async fn passes_model_reasoning_effort_to_exec() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let thread = codex.start_thread(Some(ThreadOptions {
        model_reasoning_effort: Some(ModelReasoningEffort::High),
        ..Default::default()
    }));
    thread.run("reasoning effort", None).await.expect("run");

    let args = &harness.logs()[0].args;
    expect_pair(args, ("--config", "model_reasoning_effort=\"high\""));
}

#[tokio::test]
async fn passes_network_access_enabled_to_exec() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let thread = codex.start_thread(Some(ThreadOptions {
        network_access_enabled: Some(true),
        ..Default::default()
    }));
    thread.run("network access", None).await.expect("run");

    let args = &harness.logs()[0].args;
    expect_pair(
        args,
        ("--config", "sandbox_workspace_write.network_access=true"),
    );
}

#[tokio::test]
async fn passes_web_search_enabled_to_exec() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let thread = codex.start_thread(Some(ThreadOptions {
        web_search_enabled: Some(true),
        ..Default::default()
    }));
    thread.run("web search", None).await.expect("run");

    let args = &harness.logs()[0].args;
    expect_pair(args, ("--config", "web_search=\"live\""));
}

#[tokio::test]
async fn passes_web_search_mode_to_exec() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let thread = codex.start_thread(Some(ThreadOptions {
        web_search_mode: Some(WebSearchMode::Cached),
        ..Default::default()
    }));
    thread.run("web search mode", None).await.expect("run");

    let args = &harness.logs()[0].args;
    expect_pair(args, ("--config", "web_search=\"cached\""));
}

#[tokio::test]
async fn prefers_web_search_mode_over_web_search_enabled_when_both_are_set() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let thread = codex.start_thread(Some(ThreadOptions {
        web_search_mode: Some(WebSearchMode::Cached),
        web_search_enabled: Some(true),
        ..Default::default()
    }));
    thread
        .run("web search precedence", None)
        .await
        .expect("run");

    let args = &harness.logs()[0].args;
    let values = collect_config_values(args, "web_search");
    assert_eq!(values, vec!["web_search=\"cached\"".to_string()]);
}

#[tokio::test]
async fn passes_web_search_disabled_to_exec() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let thread = codex.start_thread(Some(ThreadOptions {
        web_search_enabled: Some(false),
        ..Default::default()
    }));
    thread.run("web search off", None).await.expect("run");

    let args = &harness.logs()[0].args;
    expect_pair(args, ("--config", "web_search=\"disabled\""));
}

#[tokio::test]
async fn passes_approval_policy_to_exec() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let thread = codex.start_thread(Some(ThreadOptions {
        approval_policy: Some(ApprovalMode::OnRequest),
        ..Default::default()
    }));
    thread.run("approval", None).await.expect("run");

    let args = &harness.logs()[0].args;
    expect_pair(args, ("--config", "approval_policy=\"on-request\""));
}

#[tokio::test]
async fn passes_codex_config_overrides_as_toml_flags() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let mut options = CodexOptions::default();
    options.config = Some(
        json!({
            "approval_policy": "never",
            "sandbox_workspace_write": { "network_access": true },
            "retry_budget": 3,
            "tool_rules": { "allow": ["git status", "git diff"] }
        })
        .as_object()
        .expect("config object")
        .clone(),
    );
    let codex = harness.codex(options).expect("codex");

    let thread = codex.start_thread(None);
    thread.run("config overrides", None).await.expect("run");

    let args = &harness.logs()[0].args;
    expect_pair(args, ("--config", "approval_policy=\"never\""));
    expect_pair(
        args,
        ("--config", "sandbox_workspace_write.network_access=true"),
    );
    expect_pair(args, ("--config", "retry_budget=3"));
    expect_pair(
        args,
        (
            "--config",
            "tool_rules.allow=[\"git status\", \"git diff\"]",
        ),
    );
}

#[tokio::test]
async fn errors_on_null_config_override_values() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let mut options = CodexOptions::default();
    options.config = Some(
        json!({
            "approval_policy": null
        })
        .as_object()
        .expect("config object")
        .clone(),
    );
    let codex = harness.codex(options).expect("codex");
    let thread = codex.start_thread(None);

    let error = thread
        .run("invalid config", None)
        .await
        .expect_err("must fail");
    assert!(error.to_string().contains("cannot be null"));
}

#[tokio::test]
async fn serializes_non_bare_inline_table_keys_with_quotes() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let mut options = CodexOptions::default();
    options.config = Some(
        json!({
            "tool_rules": {
                "allow list": ["git status"]
            }
        })
        .as_object()
        .expect("config object")
        .clone(),
    );
    let codex = harness.codex(options).expect("codex");
    let thread = codex.start_thread(None);
    thread.run("quoted key", None).await.expect("run");

    let args = &harness.logs()[0].args;
    expect_pair(
        args,
        ("--config", "tool_rules.\"allow list\"=[\"git status\"]"),
    );
}

#[tokio::test]
async fn thread_options_override_global_config() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let mut options = CodexOptions::default();
    options.config = Some(
        json!({
            "approval_policy": "never"
        })
        .as_object()
        .expect("config object")
        .clone(),
    );
    let codex = harness.codex(options).expect("codex");

    let thread = codex.start_thread(Some(ThreadOptions {
        approval_policy: Some(ApprovalMode::OnRequest),
        ..Default::default()
    }));
    thread.run("override", None).await.expect("run");

    let args = &harness.logs()[0].args;
    let values = collect_config_values(args, "approval_policy");
    assert_eq!(
        values,
        vec![
            "approval_policy=\"never\"".to_string(),
            "approval_policy=\"on-request\"".to_string()
        ]
    );
}

#[tokio::test]
async fn allows_overriding_env_passed_to_codex_cli() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);

    let mut options = CodexOptions::default();
    let mut env = HashMap::new();
    env.insert("CUSTOM_ENV".to_string(), "custom".to_string());
    options.env = Some(env);

    let codex = harness.codex(options).expect("codex");
    let thread = codex.start_thread(None);
    thread.run("custom env", None).await.expect("run");

    let logs = harness.logs();
    let env = &logs[0].env;
    assert_eq!(env.get("CUSTOM_ENV").map(String::as_str), Some("custom"));
    assert_eq!(env.get("CODEX_ENV_SHOULD_NOT_LEAK"), None);
    assert_eq!(
        env.get("CODEX_INTERNAL_ORIGINATOR_OVERRIDE")
            .map(String::as_str),
        Some("codex_sdk_rust")
    );
}

#[tokio::test]
async fn passes_additional_directories_as_repeated_flags() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let thread = codex.start_thread(Some(ThreadOptions {
        additional_directories: Some(vec!["../backend".to_string(), "/tmp/shared".to_string()]),
        ..Default::default()
    }));
    thread.run("dirs", None).await.expect("run");

    let args = &harness.logs()[0].args;
    let mut values = Vec::new();
    for window in args.windows(2) {
        if window[0] == "--add-dir" {
            values.push(window[1].to_string());
        }
    }
    assert_eq!(values, vec!["../backend", "/tmp/shared"]);
}

#[tokio::test]
async fn writes_output_schema_temp_file_and_cleans_up() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    let schema = json!({
        "type": "object",
        "properties": { "answer": { "type": "string" } },
        "required": ["answer"],
        "additionalProperties": false
    });
    thread
        .run(
            "structured",
            Some(TurnOptions {
                output_schema: Some(schema.clone()),
                ..Default::default()
            }),
        )
        .await
        .expect("run");

    let logs = harness.logs();
    assert!(logs[0].output_schema_exists);
    assert_eq!(logs[0].output_schema, Some(schema));
    let schema_path = logs[0].output_schema_path.clone().expect("schema path");
    assert!(!harness.path_exists(&schema_path));
}

#[tokio::test]
async fn rejects_non_object_output_schema() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    let error = thread
        .run(
            "structured",
            Some(TurnOptions {
                output_schema: Some(json!(["not", "an", "object"])),
                ..Default::default()
            }),
        )
        .await
        .expect_err("must fail");
    assert!(
        error
            .to_string()
            .contains("output_schema must be a plain JSON object")
    );
}

#[tokio::test]
async fn combines_structured_text_input_segments() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    let input = Input::Entries(vec![
        UserInput::Text {
            text: "Describe file changes".to_string(),
        },
        UserInput::Text {
            text: "Focus on impacted tests".to_string(),
        },
    ]);
    thread.run(input, None).await.expect("run");

    let logs = harness.logs();
    assert_eq!(
        logs[0].stdin,
        "Describe file changes\n\nFocus on impacted tests"
    );
}

#[tokio::test]
async fn forwards_images_to_exec() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    let image1 = "/tmp/mock_image_1.png";
    let image2 = "/tmp/mock_image_2.jpg";
    let input = Input::Entries(vec![
        UserInput::Text {
            text: "describe".to_string(),
        },
        UserInput::LocalImage {
            path: image1.into(),
        },
        UserInput::LocalImage {
            path: image2.into(),
        },
    ]);
    thread.run(input, None).await.expect("run");

    let args = &harness.logs()[0].args;
    let mut images = Vec::new();
    for window in args.windows(2) {
        if window[0] == "--image" {
            images.push(window[1].to_string());
        }
    }
    assert_eq!(images, vec![image1, image2]);
}

#[tokio::test]
async fn runs_in_provided_working_directory() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(Some(ThreadOptions {
        working_directory: Some("/tmp".to_string()),
        skip_git_repo_check: Some(true),
        ..Default::default()
    }));

    thread.run("cwd", None).await.expect("run");
    let args = &harness.logs()[0].args;
    expect_pair(args, ("--cd", "/tmp"));
    assert!(args.iter().any(|arg| arg == "--skip-git-repo-check"));
}

#[tokio::test]
async fn throws_when_working_directory_not_git_and_skip_not_set() {
    let harness = MockCodexHarness::new(vec![Vec::new()]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(Some(ThreadOptions {
        working_directory: Some("/tmp".to_string()),
        ..Default::default()
    }));

    let error = thread.run("cwd", None).await.expect_err("must fail");
    assert!(error.to_string().contains("Not inside a trusted directory"));
}

#[tokio::test]
async fn sets_the_codex_sdk_originator_env() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "ok", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    thread.run("originator", None).await.expect("run");
    let logs = harness.logs();
    assert_eq!(
        logs[0]
            .env
            .get("CODEX_INTERNAL_ORIGINATOR_OVERRIDE")
            .map(String::as_str),
        Some("codex_sdk_rust")
    );
}

#[tokio::test]
async fn throws_thread_run_error_on_turn_failures() {
    let harness = MockCodexHarness::new(vec![vec![
        json!({ "type": "thread.started", "thread_id": "thread_1" }),
        json!({ "type": "turn.started" }),
        json!({
            "type": "turn.failed",
            "error": { "message": "rate limit exceeded" }
        }),
    ]]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    let error = thread.run("fail", None).await.expect_err("must fail");
    assert!(error.to_string().contains("rate limit exceeded"));
}

#[tokio::test]
async fn throws_thread_run_error_on_stream_error_event() {
    let harness = MockCodexHarness::new(vec![vec![
        json!({ "type": "thread.started", "thread_id": "thread_1" }),
        json!({ "type": "turn.started" }),
        json!({
            "type": "error",
            "message": "stream disconnected before completion"
        }),
    ]]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    let error = thread.run("fail", None).await.expect_err("must fail");
    assert!(
        error
            .to_string()
            .contains("stream disconnected before completion")
    );
}

#[tokio::test]
async fn returns_agent_message_items_in_run_result() {
    let harness = MockCodexHarness::new(vec![success_events(Some("thread_1"), "Hello", "item_1")]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    let result = thread.run("hello", None).await.expect("run");
    assert!(matches!(
        result.items.first(),
        Some(ThreadItem::AgentMessage(_))
    ));
}
