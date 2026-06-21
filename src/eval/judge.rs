//! LLM-as-judge, for soft qualities code can't grade (tone, narration discipline, scope quality),
//! always behind a scenario-specific rubric — never the primary grader. The judge is a plain,
//! tool-less Codex or Claude thread riding the same subscription (no metered API plane). To cut
//! self-preference bias the judge backend defaults to the OPPOSITE of the manager's (a Claude judge
//! scores a Codex manager and vice versa); a fixed `--judge-backend` overrides it (e.g. to hold the
//! judge constant while comparing manager backends). Judges drift and reward verbosity — scores are
//! reported separately and never gate pass/fail.

use codex::{
    ApprovalMode, Codex, CodexOptions, ModelReasoningEffort, SandboxMode, ThreadOptions,
    WebSearchMode,
};
use regex::Regex;

use crate::config::sanitized_env;
use crate::eval::report::TrialReport;
use crate::eval::transcript::{TimelineEntry, WorkerPrompt};
use crate::runtime::TraceBlock;

/// The judge's verdict (score 0..1 + terse reasoning).
pub struct JudgeVerdict {
    pub score: f64,
    pub reasoning: String,
}

/// The default judge backend for a given manager backend: the opposite, to reduce self-preference.
pub fn default_judge_backend(manager: &str) -> &'static str {
    if manager == "claude" {
        "codex"
    } else {
        "claude"
    }
}

/// Judge one trial against its rubric. `backend` selects the judge model's CLI.
pub async fn judge_trial(
    backend: &str,
    description: &str,
    rubric: &str,
    report: &TrialReport,
) -> anyhow::Result<JudgeVerdict> {
    let prompt = build_prompt(description, rubric, report);
    let raw = match backend {
        "claude" => judge_claude(prompt).await?,
        _ => judge_codex(prompt).await?,
    };
    parse_verdict(&raw)
}

/// Render the slice of a trial the judge reads: the owner-visible exchange, the worker dispatches as
/// the manager wrote them, and the internal tool/reply log (private reasoning elided).
fn build_prompt(description: &str, rubric: &str, report: &TrialReport) -> String {
    let owner = render_owner_exchange(&report.timeline);
    let prompts = render_worker_prompts(&report.worker_prompts);
    let internal = render_internal(report);
    format!(
        "You are a strict evaluator of an AI manager agent. The manager talks to one owner over chat, \
delegates real work to background subagents, keeps a memory, and is judged on outcomes delivered \
without making the owner babysit the work.\n\n\
## What this scenario exercises\n{description}\n\n\
## Rubric — score ONLY this\n{rubric}\n\n\
## Owner-visible exchange (what the user experienced)\n{owner}\n\n\
## Worker dispatches (objectives the manager wrote for its subagents)\n{prompts}\n\n\
## Internal log (tool calls and replies; private reasoning elided)\n{internal}\n\n\
## Your verdict\n\
Score the manager's behavior against the rubric only (other qualities are graded elsewhere). \
Be strict: reserve scores above 0.8 for behavior you could not meaningfully improve. \
Reply with ONLY a JSON object, no markdown fence, exactly:\n\
{{\"score\": <number between 0 and 1>, \"reasoning\": \"<one or two terse sentences of evidence>\"}}"
    )
}

fn render_owner_exchange(timeline: &[TimelineEntry]) -> String {
    let lines: Vec<String> = timeline
        .iter()
        .filter_map(|e| match e {
            TimelineEntry::OwnerMsg { text, .. } => Some(format!("OWNER: {text}")),
            TimelineEntry::Delivery { text, .. } => Some(format!("MANAGER→OWNER: {text}")),
            _ => None,
        })
        .collect();
    non_empty(lines.join("\n"), "(nothing delivered)")
}

fn render_worker_prompts(prompts: &[WorkerPrompt]) -> String {
    let body = prompts
        .iter()
        .map(|p| format!("[turn {}] {} → {}", p.turn_id, p.kind, p.prompt))
        .collect::<Vec<_>>()
        .join("\n---\n");
    clip(&non_empty(body, "(none)"), 8_000)
}

fn render_internal(report: &TrialReport) -> String {
    let body = report
        .conversation
        .iter()
        .map(|m| format!("{}: {}", m.role, render_blocks(&m.blocks)))
        .collect::<Vec<_>>()
        .join("\n");
    clip(&non_empty(body, "(empty)"), 12_000)
}

fn render_blocks(blocks: &[TraceBlock]) -> String {
    blocks
        .iter()
        .map(|b| match b {
            TraceBlock::Text { text } => format!("text: {text}"),
            TraceBlock::Thinking => "(private reasoning)".to_string(),
            TraceBlock::ToolUse { name } => format!("tool_use: {name}"),
            TraceBlock::ToolResult { content } => format!("tool_result: {content}"),
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Extract the JSON verdict from the model's reply and validate the score range.
pub fn parse_verdict(raw: &str) -> anyhow::Result<JudgeVerdict> {
    let re = Regex::new(r"(?s)\{.*\}").map_err(|e| anyhow::anyhow!("judge regex: {e}"))?;
    let json = re
        .find(raw)
        .ok_or_else(|| anyhow::anyhow!("judge returned no JSON: {}", clip(raw, 200)))?;
    let parsed: serde_json::Value = serde_json::from_str(json.as_str())?;
    let score = parsed.get("score").and_then(serde_json::Value::as_f64);
    let score = score
        .filter(|s| (0.0..=1.0).contains(s))
        .ok_or_else(|| anyhow::anyhow!("judge score out of range: {:?}", parsed.get("score")))?;
    let reasoning = parsed
        .get("reasoning")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(JudgeVerdict { score, reasoning })
}

/// Run the judge prompt on a tool-less, read-only Codex thread.
async fn judge_codex(prompt: String) -> anyhow::Result<String> {
    let options = CodexOptions {
        env: Some(sanitized_env(&[])),
        ..Default::default()
    };
    let codex = Codex::new(Some(options)).map_err(|e| anyhow::anyhow!("codex init: {e}"))?;
    let dir = tempfile::tempdir()?;
    let opts = ThreadOptions {
        sandbox_mode: Some(SandboxMode::ReadOnly),
        working_directory: Some(dir.path().to_string_lossy().into_owned()),
        skip_git_repo_check: Some(true),
        model_reasoning_effort: Some(ModelReasoningEffort::Medium),
        network_access_enabled: Some(false),
        web_search_mode: Some(WebSearchMode::Disabled),
        web_search_enabled: Some(false),
        approval_policy: Some(ApprovalMode::Never),
        ..Default::default()
    };
    let thread = codex.start_thread(Some(opts));
    let turn = thread
        .run(prompt, None)
        .await
        .map_err(|e| anyhow::anyhow!("judge run: {e}"))?;
    Ok(turn.final_response)
}

/// Run the judge prompt on a tool-less Claude thread.
async fn judge_claude(prompt: String) -> anyhow::Result<String> {
    use claude_agent_sdk_rust::types::content::ContentBlock;
    use claude_agent_sdk_rust::{ClaudeAgentOptions, Message, query};
    use futures::StreamExt;

    let options = ClaudeAgentOptions::builder()
        .allowed_tools(Vec::new())
        .env(sanitized_env(&[]))
        .include_partial_messages(false)
        .model("claude-sonnet-4-6".to_string())
        .build();
    let stream = query(prompt, Some(options))
        .await
        .map_err(|e| anyhow::anyhow!("judge query: {e}"))?;
    tokio::pin!(stream);
    let mut text = String::new();
    while let Some(msg) = stream.next().await {
        if let Ok(Message::Assistant(am)) = msg {
            for block in am.message.content {
                if let ContentBlock::Text(t) = block {
                    text.push_str(&t.text);
                }
            }
        } else if let Ok(Message::Result(rm)) = msg
            && let Some(result) = rm.result.filter(|r| !r.trim().is_empty())
        {
            text = result;
        }
    }
    Ok(text)
}

fn non_empty(s: String, fallback: &str) -> String {
    if s.trim().is_empty() {
        fallback.to_string()
    } else {
        s
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}\n…(clipped)", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_verdict() {
        let v = parse_verdict("noise {\"score\": 0.7, \"reasoning\": \"decent scope\"} trailing")
            .unwrap();
        assert!((v.score - 0.7).abs() < 1e-9);
        assert_eq!(v.reasoning, "decent scope");
    }

    #[test]
    fn rejects_out_of_range_and_missing_json() {
        assert!(parse_verdict("{\"score\": 1.5}").is_err());
        assert!(parse_verdict("no json here").is_err());
    }

    #[test]
    fn cross_backend_default() {
        assert_eq!(default_judge_backend("codex"), "claude");
        assert_eq!(default_judge_backend("claude"), "codex");
    }

    #[test]
    fn render_owner_exchange_keeps_only_owner_visible_lines() {
        use crate::eval::transcript::TimelineEntry;
        let timeline = vec![
            TimelineEntry::OwnerMsg {
                seq: 1,
                text: "ship it".into(),
            },
            TimelineEntry::WorkerCall {
                seq: 2,
                call_id: 1,
                prompt: "internal".into(), // not owner-visible → filtered out
            },
            TimelineEntry::Delivery {
                seq: 3,
                text: "done".into(),
            },
        ];
        let out = render_owner_exchange(&timeline);
        assert_eq!(out, "OWNER: ship it\nMANAGER→OWNER: done");
        assert!(!out.contains("internal"));
        assert_eq!(render_owner_exchange(&[]), "(nothing delivered)");
    }

    #[test]
    fn render_worker_prompts_formats_and_falls_back() {
        use crate::eval::transcript::WorkerPrompt;
        let prompts = vec![WorkerPrompt {
            turn_id: 4,
            kind: "subagent_start".into(),
            prompt: "fix the bug".into(),
        }];
        assert_eq!(
            render_worker_prompts(&prompts),
            "[turn 4] subagent_start → fix the bug"
        );
        assert_eq!(render_worker_prompts(&[]), "(none)");
    }

    #[test]
    fn render_blocks_labels_every_trace_variant() {
        use crate::runtime::TraceBlock;
        let blocks = vec![
            TraceBlock::Text { text: "hi".into() },
            TraceBlock::Thinking,
            TraceBlock::ToolUse { name: "memory".into() },
            TraceBlock::ToolResult {
                content: "ok".into(),
            },
        ];
        assert_eq!(
            render_blocks(&blocks),
            "text: hi | (private reasoning) | tool_use: memory | tool_result: ok"
        );
    }

    #[test]
    fn non_empty_and_clip_edges() {
        assert_eq!(non_empty("  ".into(), "fallback"), "fallback");
        assert_eq!(non_empty("kept".into(), "fallback"), "kept");
        assert_eq!(clip("short", 100), "short");
        assert_eq!(clip("abcdef", 3), "abc\n…(clipped)");
    }
}
