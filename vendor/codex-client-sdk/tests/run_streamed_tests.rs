mod helpers;

use codex::{CodexOptions, ThreadEvent, TurnOptions};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::helpers::MockCodexHarness;

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

    let streamed = thread
        .run_streamed("Hello, world!", None)
        .await
        .expect("streamed");

    let mut events = Vec::new();
    let mut stream = streamed.events;
    while let Some(event) = stream.next().await {
        events.push(event.expect("event"));
    }

    assert_eq!(
        events,
        vec![
            ThreadEvent::ThreadStarted {
                thread_id: "thread_1".to_string()
            },
            ThreadEvent::TurnStarted,
            ThreadEvent::ItemCompleted {
                item: codex::ThreadItem::AgentMessage(codex::AgentMessageItem {
                    id: "item_1".to_string(),
                    text: "Hi!".to_string()
                })
            },
            ThreadEvent::TurnCompleted {
                usage: codex::Usage {
                    input_tokens: 42,
                    cached_input_tokens: 12,
                    output_tokens: 5
                }
            }
        ]
    );
    assert_eq!(thread.id().as_deref(), Some("thread_1"));
}

#[tokio::test]
async fn sends_resume_when_run_streamed_is_called_twice() {
    let harness = MockCodexHarness::new(vec![
        success_events(Some("thread_1"), "First response", "item_1"),
        success_events(None, "Second response", "item_2"),
    ]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    let mut first = thread
        .run_streamed("first input", None)
        .await
        .expect("first")
        .events;
    while first.next().await.is_some() {}

    let mut second = thread
        .run_streamed("second input", None)
        .await
        .expect("second")
        .events;
    while second.next().await.is_some() {}

    let logs = harness.logs();
    assert_eq!(logs.len(), 2);
    assert!(
        logs[1]
            .args
            .windows(2)
            .any(|window| window[0] == "resume" && window[1] == "thread_1")
    );
}

#[tokio::test]
async fn resumes_thread_by_id_when_streaming() {
    let harness = MockCodexHarness::new(vec![
        success_events(Some("thread_1"), "First response", "item_1"),
        success_events(None, "Second response", "item_2"),
    ]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");

    let original = codex.start_thread(None);
    let mut first = original
        .run_streamed("first input", None)
        .await
        .expect("first")
        .events;
    while first.next().await.is_some() {}

    let resumed = codex.resume_thread(original.id().expect("id"), None);
    let mut second = resumed
        .run_streamed("second input", None)
        .await
        .expect("second")
        .events;
    while second.next().await.is_some() {}

    assert_eq!(resumed.id(), original.id());
    let logs = harness.logs();
    assert!(
        logs[1]
            .args
            .windows(2)
            .any(|window| window[0] == "resume" && window[1] == "thread_1")
    );
}

#[tokio::test]
async fn applies_output_schema_turn_options_when_streaming() {
    let harness = MockCodexHarness::new(vec![success_events(
        Some("thread_1"),
        "Structured",
        "item_1",
    )]);
    let codex = harness.codex(CodexOptions::default()).expect("codex");
    let thread = codex.start_thread(None);

    let schema = json!({
        "type": "object",
        "properties": { "answer": { "type": "string" } },
        "required": ["answer"],
        "additionalProperties": false
    });

    let mut events = thread
        .run_streamed(
            "structured",
            Some(TurnOptions {
                output_schema: Some(schema.clone()),
                ..Default::default()
            }),
        )
        .await
        .expect("streamed")
        .events;
    while events.next().await.is_some() {}

    let logs = harness.logs();
    assert_eq!(logs[0].output_schema, Some(schema));
}
