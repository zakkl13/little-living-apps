//! Reconstruct a trial's transcript from the JSONL eval trace the running binary emitted. Reuses the
//! very [`TraceRecord`] type the binary wrote (one source of truth for the wire shape). Token usage
//! is NOT reconstructed here — the harness reads the authoritative cumulative [`UsageMeter`] from the
//! binary's own snapshot file, which already persists the manager/worker split after every turn.

use crate::eval::transcript::{ConvMessage, TimelineEntry, WorkerPrompt, WorkerSession};
use crate::runtime::TraceRecord;

/// The transcript pieces a trace yields (everything but on-disk end state + cumulative usage).
#[derive(Default)]
pub struct ParsedTrace {
    pub timeline: Vec<TimelineEntry>,
    pub deliveries: Vec<String>,
    pub conversation: Vec<ConvMessage>,
    pub worker_prompts: Vec<WorkerPrompt>,
    pub worker_sessions: Vec<WorkerSession>,
}

/// Parse a JSONL trace body (one [`TraceRecord`] per line; malformed lines are skipped).
pub fn parse(jsonl: &str) -> ParsedTrace {
    let mut acc = Acc::default();
    for line in jsonl.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(record) = serde_json::from_str::<TraceRecord>(line) {
            acc.absorb(record);
        }
    }
    acc.parsed
}

/// Parse a trace file by path (empty result if it is absent/unreadable).
pub fn parse_file(path: &std::path::Path) -> ParsedTrace {
    std::fs::read_to_string(path)
        .map(|body| parse(&body))
        .unwrap_or_default()
}

/// Folds records into a [`ParsedTrace`], assigning a monotonic timeline seq.
#[derive(Default)]
struct Acc {
    parsed: ParsedTrace,
    seq: u64,
}

impl Acc {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn absorb(&mut self, record: TraceRecord) {
        match record {
            TraceRecord::OwnerMsg { text } => {
                let seq = self.next_seq();
                self.parsed
                    .timeline
                    .push(TimelineEntry::OwnerMsg { seq, text });
            }
            TraceRecord::Delivery { text } => self.absorb_delivery(text),
            TraceRecord::ManagerMsg { role, blocks } => {
                self.parsed.conversation.push(ConvMessage { role, blocks });
            }
            TraceRecord::WorkerPrompt {
                turn_id,
                kind,
                prompt,
            } => {
                self.parsed.worker_prompts.push(WorkerPrompt {
                    turn_id,
                    kind,
                    prompt,
                });
            }
            other => self.absorb_worker_lifecycle(other),
        }
    }

    fn absorb_delivery(&mut self, text: String) {
        self.parsed.deliveries.push(text.clone());
        let seq = self.next_seq();
        self.parsed
            .timeline
            .push(TimelineEntry::Delivery { seq, text });
    }

    fn absorb_worker_lifecycle(&mut self, record: TraceRecord) {
        match record {
            TraceRecord::WorkerCall { call_id, prompt } => self.worker_call(call_id, prompt),
            TraceRecord::WorkerNote { call_id, note } => self.worker_note(call_id, note),
            TraceRecord::WorkerDone {
                call_id,
                ok,
                response,
            } => {
                self.worker_done(call_id, ok, response);
            }
            _ => {} // Usage / Idle: not part of the reconstructed transcript.
        }
    }

    fn worker_call(&mut self, call_id: u64, prompt: String) {
        self.parsed.worker_sessions.push(WorkerSession {
            call_id,
            prompt: prompt.clone(),
            ..Default::default()
        });
        let seq = self.next_seq();
        self.parsed.timeline.push(TimelineEntry::WorkerCall {
            seq,
            call_id,
            prompt,
        });
    }

    fn worker_note(&mut self, call_id: u64, note: String) {
        if let Some(session) = self.session_mut(call_id) {
            session.notes.push(note.clone());
        }
        let seq = self.next_seq();
        self.parsed
            .timeline
            .push(TimelineEntry::WorkerNote { seq, call_id, note });
    }

    fn worker_done(&mut self, call_id: u64, ok: bool, response: String) {
        if let Some(session) = self.session_mut(call_id) {
            session.ok = ok;
            session.response = response.clone();
        }
        let seq = self.next_seq();
        self.parsed.timeline.push(TimelineEntry::WorkerDone {
            seq,
            call_id,
            ok,
            response,
        });
    }

    fn session_mut(&mut self, call_id: u64) -> Option<&mut WorkerSession> {
        self.parsed
            .worker_sessions
            .iter_mut()
            .find(|s| s.call_id == call_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstructs_timeline_and_worker_sessions() {
        let jsonl = r#"
{"type":"owner_msg","text":"fix the thing"}
{"type":"manager_msg","role":"assistant","blocks":[{"block":"text","text":"on it"}]}
{"type":"worker_prompt","turn_id":1,"kind":"start","prompt":"fix /greet"}
{"type":"worker_call","call_id":1,"prompt":"fix /greet"}
{"type":"worker_note","call_id":1,"note":"ran curl, got 200"}
{"type":"worker_done","call_id":1,"ok":true,"response":"PASS — verified 200"}
{"type":"delivery","text":"all set"}
{"type":"idle"}
"#;
        let p = parse(jsonl);
        assert_eq!(p.deliveries, vec!["all set"]);
        assert_eq!(p.conversation.len(), 1);
        assert_eq!(p.worker_prompts.len(), 1);
        assert_eq!(p.worker_prompts[0].turn_id, 1);
        assert_eq!(p.worker_sessions.len(), 1);
        let s = &p.worker_sessions[0];
        assert!(s.ok);
        assert_eq!(s.notes, vec!["ran curl, got 200"]);
        assert!(s.response.contains("PASS"));
        // The timeline is seq-ordered: owner(1) < worker_call(2) < worker_note(3) < done(4) < delivery(5).
        let seqs: Vec<u64> = p.timeline.iter().map(TimelineEntry::seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5]);
    }
}
