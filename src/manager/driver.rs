//! One manager turn over a long-lived backend thread. Port of `src/manager/driver.ts`.
//!
//! Prepend the volatile context header → run the turn → the final agent message is the manager's
//! reply (honoring the `NO_REPLY` sentinel and `ATTACH:` lines); tool calls/reasoning are internal.
//! The driver returns a [`TurnOutcome`]; the app owns delivery + the worker-event reply gate.

use super::backend::{BackendEvent, ManagerBackend, ManagerThread, TurnInput};

/// The sentinel a turn emits to absorb an event without messaging the owner.
pub const NO_REPLY: &str = "NO_REPLY";

/// What a turn produced, for the app to deliver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// Deliver this message (with any image attachments).
    Reply {
        text: String,
        attachments: Vec<String>,
    },
    /// Nothing to say (NO_REPLY, or an empty message).
    Silent,
    /// The turn failed; deliver this owner-facing error text.
    Error(String),
}

/// Drives turns over a backend, owning the lazily-created thread + the resumable session id.
pub struct ManagerDriver {
    backend: Box<dyn ManagerBackend>,
    thread: Option<Box<dyn ManagerThread>>,
    pending_resume: Option<String>,
    current_session: Option<String>,
}

impl ManagerDriver {
    pub fn new(backend: Box<dyn ManagerBackend>) -> Self {
        Self {
            backend,
            thread: None,
            pending_resume: None,
            current_session: None,
        }
    }

    /// The current manager session id, for snapshotting.
    pub fn session_id(&self) -> Option<String> {
        self.current_session.clone()
    }

    /// Seed the resume id from a restored snapshot (before the first turn).
    pub fn adopt_session_id(&mut self, id: Option<String>) {
        self.pending_resume = id.clone();
        self.current_session = id;
    }

    /// `/new`: drop the thread so the next turn starts fresh (memory kept).
    pub fn reset(&mut self) {
        self.thread = None;
        self.pending_resume = None;
        self.current_session = None;
    }

    fn ensure_thread(&mut self) -> &mut Box<dyn ManagerThread> {
        if self.thread.is_none() {
            let resume = self.pending_resume.take();
            self.thread = Some(self.backend.thread(resume));
        }
        match &mut self.thread {
            Some(thread) => thread,
            None => unreachable!("thread was set above"),
        }
    }

    /// Run one turn. `header` is the volatile context header (prepended); `observer` sees every
    /// streamed event (for telemetry/logging).
    pub async fn run_turn(
        &mut self,
        header: &str,
        input: TurnInput,
        observer: &mut (dyn FnMut(&BackendEvent) + Send),
    ) -> TurnOutcome {
        let composed = compose(header, input);

        // Collect events live (forwarding each to the observer), then interpret them afterward.
        let mut events: Vec<BackendEvent> = Vec::new();
        {
            let thread = self.ensure_thread();
            let mut on_event = |ev: BackendEvent| {
                observer(&ev);
                events.push(ev);
            };
            if let Err(err) = thread.run_turn(composed, &mut on_event).await {
                events.push(BackendEvent::Failed(err.to_string()));
            }
        }

        self.capture_session();
        self.build_outcome(events)
    }

    /// Capture the session id once the turn has started one; resume is no longer needed.
    fn capture_session(&mut self) {
        if let Some(id) = self.thread.as_ref().and_then(|t| t.session_id()) {
            self.current_session = Some(id);
            self.pending_resume = None;
        }
    }

    /// Turn the streamed events into an outcome: a failure beats a reply; else the last message.
    fn build_outcome(&self, events: Vec<BackendEvent>) -> TurnOutcome {
        let mut final_reply: Option<String> = None;
        let mut failure: Option<String> = None;
        for ev in events {
            match ev {
                BackendEvent::AgentMessage(t) => final_reply = Some(t),
                BackendEvent::Failed(f) => {
                    failure.get_or_insert(f);
                }
                _ => {}
            }
        }
        if let Some(f) = failure {
            return TurnOutcome::Error(self.backend.format_error(&f));
        }
        final_reply.map_or(TurnOutcome::Silent, |t| finalize_reply(&t))
    }
}

/// Prepend the volatile context header to the turn input.
fn compose(header: &str, input: TurnInput) -> TurnInput {
    let text = if header.is_empty() {
        input.text
    } else {
        format!("{header}\n\n---\n\n{}", input.text)
    };
    TurnInput {
        text,
        image_path: input.image_path,
    }
}

/// Apply NO_REPLY + ATTACH extraction to a raw agent message.
fn finalize_reply(raw: &str) -> TurnOutcome {
    let body = apply_no_reply(raw);
    if body.is_empty() {
        return TurnOutcome::Silent;
    }
    let (text, attachments) = extract_attachments(&body);
    if text.is_empty() && attachments.is_empty() {
        TurnOutcome::Silent
    } else {
        TurnOutcome::Reply { text, attachments }
    }
}

/// The user-facing text, or "" when the model signaled silence (a `NO_REPLY` token on its own line).
pub fn apply_no_reply(text: &str) -> String {
    if text.lines().any(|line| line.trim() == NO_REPLY) {
        String::new()
    } else {
        text.trim().to_string()
    }
}

/// Split a reply into the text the owner reads and the image paths to send. `ATTACH: /path` lines
/// are removed from the text (so NO_REPLY + the delivery gate govern them too).
pub fn extract_attachments(reply: &str) -> (String, Vec<String>) {
    let mut attachments = Vec::new();
    let mut kept = Vec::new();
    for line in reply.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("ATTACH:") {
            let path = rest.trim();
            if path.starts_with('/') && !path.contains(char::is_whitespace) {
                attachments.push(path.to_string());
                continue;
            }
        }
        kept.push(line);
    }
    (kept.join("\n").trim().to_string(), attachments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_reply_silences_whole_message() {
        assert_eq!(apply_no_reply("NO_REPLY"), "");
        assert_eq!(apply_no_reply("thinking...\nNO_REPLY\nmore"), "");
        assert_eq!(apply_no_reply("hello there"), "hello there");
    }

    #[test]
    fn extracts_attachments() {
        let (text, att) = extract_attachments("Here it is:\nATTACH: /tmp/shot.png\ndone");
        assert_eq!(text, "Here it is:\ndone");
        assert_eq!(att, vec!["/tmp/shot.png".to_string()]);
    }

    #[test]
    fn ignores_non_absolute_attach() {
        let (text, att) = extract_attachments("ATTACH: relative.png");
        assert!(att.is_empty());
        assert!(text.contains("ATTACH: relative.png"));
    }

    #[test]
    fn finalize_reply_variants() {
        assert_eq!(
            finalize_reply("hi"),
            TurnOutcome::Reply {
                text: "hi".into(),
                attachments: vec![]
            }
        );
        assert_eq!(finalize_reply("NO_REPLY"), TurnOutcome::Silent);
        assert_eq!(
            finalize_reply("ATTACH: /a.png"),
            TurnOutcome::Reply {
                text: String::new(),
                attachments: vec!["/a.png".into()]
            }
        );
    }
}
