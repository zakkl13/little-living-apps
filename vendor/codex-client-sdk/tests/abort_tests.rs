mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use codex::{CodexOptions, Error, TurnOptions};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::helpers::MockCodexHarness;

fn success_events() -> Vec<Value> {
    vec![
        json!({ "type": "thread.started", "thread_id": "thread_1" }),
        json!({ "type": "turn.started" }),
        json!({
            "type": "item.completed",
            "item": { "id": "item_1", "type": "agent_message", "text": "Hi!" }
        }),
        json!({
            "type": "turn.completed",
            "usage": { "input_tokens": 42, "cached_input_tokens": 12, "output_tokens": 5 }
        }),
    ]
}

fn infinite_mode_options() -> CodexOptions {
    let mut options = CodexOptions::default();
    let mut env = HashMap::new();
    env.insert("CODEX_MOCK_INFINITE".to_string(), "1".to_string());
    env.insert("CODEX_MOCK_STREAM_DELAY_MS".to_string(), "20".to_string());
    options.env = Some(env);
    options
}

#[tokio::test]
async fn aborts_run_when_token_is_already_cancelled() {
    let harness = MockCodexHarness::new(vec![success_events()]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    let token = CancellationToken::new();
    token.cancel();
    let error = thread
        .run(
            "Hello, world!",
            Some(TurnOptions {
                cancellation_token: Some(token),
                ..Default::default()
            }),
        )
        .await
        .expect_err("must fail");

    assert!(matches!(error, Error::Cancelled));
}

#[tokio::test]
async fn aborts_run_streamed_when_token_is_already_cancelled() {
    let harness = MockCodexHarness::new(vec![success_events()]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    let token = CancellationToken::new();
    token.cancel();
    let error = match thread
        .run_streamed(
            "Hello, world!",
            Some(TurnOptions {
                cancellation_token: Some(token),
                ..Default::default()
            }),
        )
        .await
    {
        Ok(_) => panic!("expected cancellation error"),
        Err(error) => error,
    };

    assert!(matches!(error, Error::Cancelled));
}

#[tokio::test]
async fn aborts_run_when_token_is_cancelled_during_execution() {
    let harness = MockCodexHarness::new(vec![Vec::new()]);
    let codex = harness.codex(infinite_mode_options()).expect("codex");
    let thread = codex.start_thread(None);

    let token = CancellationToken::new();
    let token_for_task = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        token_for_task.cancel();
    });

    let error = thread
        .run(
            "Hello, world!",
            Some(TurnOptions {
                cancellation_token: Some(token),
                ..Default::default()
            }),
        )
        .await
        .expect_err("must fail");

    assert!(matches!(error, Error::Cancelled));
}

#[tokio::test]
async fn aborts_run_streamed_when_token_is_cancelled_during_iteration() {
    let harness = MockCodexHarness::new(vec![Vec::new()]);
    let codex = harness.codex(infinite_mode_options()).expect("codex");
    let thread = codex.start_thread(None);

    let token = CancellationToken::new();
    let streamed = thread
        .run_streamed(
            "Hello, world!",
            Some(TurnOptions {
                cancellation_token: Some(token.clone()),
                ..Default::default()
            }),
        )
        .await
        .expect("streamed");

    let mut events = streamed.events;
    let mut seen = 0usize;
    loop {
        let next = events.next().await.expect("stream should continue");
        match next {
            Ok(_) => {
                seen += 1;
                if seen == 5 {
                    token.cancel();
                }
            }
            Err(error) => {
                assert!(matches!(error, Error::Cancelled));
                break;
            }
        }
    }
}

#[tokio::test]
async fn completes_normally_when_token_is_not_cancelled() {
    let harness = MockCodexHarness::new(vec![success_events()]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    let token = CancellationToken::new();
    let result = thread
        .run(
            "Hello, world!",
            Some(TurnOptions {
                cancellation_token: Some(token),
                ..Default::default()
            }),
        )
        .await
        .expect("run");

    assert_eq!(result.final_response, "Hi!");
    assert_eq!(result.items.len(), 1);
}
