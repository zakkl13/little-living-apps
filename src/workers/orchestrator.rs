//! The async worker tier, purely ephemeral. `start()`
//! launches a single-shot run in the background and returns immediately; when it settles, ONE
//! worker_event is sent onto the manager queue and the worker is gone. No registry, no resume.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;

use super::protocol::extract_manager_summary;
use super::runner::{RunArgs, Runner};
use crate::runtime::telemetry::Telemetry;
use crate::runtime::trace::{EvalTrace, TraceExt, TraceRecord};
use crate::runtime::{ManagerEvent, WorkerStatus};

/// Spawns single-shot workers and reports each back as exactly one event.
pub struct Orchestrator {
    runner: Arc<dyn Runner>,
    workspace_dir: PathBuf,
    events: UnboundedSender<ManagerEvent>,
    telemetry: Arc<Mutex<Telemetry>>,
    /// The eval/inspector trace (None in prod): records worker dispatch + completion for grading.
    trace: Option<Arc<EvalTrace>>,
    inflight: Arc<AtomicUsize>,
    counter: AtomicUsize,
    /// The manager turn currently in flight, set by the app at each turn boundary so worker
    /// dispatches are stamped with the turn that launched them (for `lila status` / Inspector).
    current_turn: AtomicU64,
}

impl Orchestrator {
    pub fn new(
        runner: Arc<dyn Runner>,
        workspace_dir: PathBuf,
        events: UnboundedSender<ManagerEvent>,
        telemetry: Arc<Mutex<Telemetry>>,
        trace: Option<Arc<EvalTrace>>,
    ) -> Self {
        Self {
            runner,
            workspace_dir,
            events,
            telemetry,
            trace,
            inflight: Arc::new(AtomicUsize::new(0)),
            counter: AtomicUsize::new(0),
            current_turn: AtomicU64::new(0),
        }
    }

    /// Record which manager turn is in flight (so worker dispatches can be stamped with it).
    pub fn set_turn(&self, turn_id: u64) {
        self.current_turn.store(turn_id, Ordering::SeqCst);
    }

    /// Spawn a single-shot worker for `objective` in `project` (returns its trace id immediately).
    pub fn start(&self, objective: String, project: Option<String>) -> String {
        let n = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let id = format!("w{n}");
        let project_dir = match &project {
            Some(p) => self.workspace_dir.join(p),
            None => self.workspace_dir.clone(),
        };
        let turn_id = self.current_turn.load(Ordering::SeqCst);
        if let Ok(mut t) = self.telemetry.lock() {
            t.record_worker_launch();
            t.record_worker_prompt(turn_id, id.clone(), "start", objective.clone());
        }
        self.inflight.fetch_add(1, Ordering::SeqCst);

        // Trace the dispatch: a `worker_prompt` stamped with the launching manager turn (the
        // parallel-decomposition join key) and a `worker_call` opening this worker's lane.
        let call_id = n as u64;
        if let Some(trace) = &self.trace {
            trace.emit(&TraceRecord::WorkerPrompt {
                turn_id: trace.current_turn(),
                kind: "start".into(),
                prompt: objective.clone(),
            });
            trace.emit(&TraceRecord::WorkerCall {
                call_id,
                prompt: objective.clone(),
            });
        }

        let runner = self.runner.clone();
        let events = self.events.clone();
        let inflight = self.inflight.clone();
        let telemetry = self.telemetry.clone();
        let trace = self.trace.clone();
        let worker_id = id.clone();
        tokio::spawn(async move {
            let result = runner
                .run(RunArgs {
                    prompt: objective.clone(),
                    cwd: project_dir,
                })
                .await;
            // Fold the worker's token usage into the cumulative worker totals (manager vs worker
            // split is the headline of `lila status` + the eval baseline).
            if let Ok(turn) = &result
                && let Ok(mut t) = telemetry.lock()
            {
                t.record_worker_usage(turn.usage);
            }
            let (status, summary) = match result {
                Ok(turn) if turn.ok => (
                    WorkerStatus::Completed,
                    extract_manager_summary(&turn.final_response),
                ),
                Ok(turn) => (WorkerStatus::Failed, turn.final_response),
                Err(err) => (WorkerStatus::Failed, err.to_string()),
            };
            trace.rec(TraceRecord::WorkerDone {
                call_id,
                ok: status == WorkerStatus::Completed,
                response: summary.clone(),
            });
            // Retire the run BEFORE emitting its event: the event wakes the manager loop, whose reply
            // gate reads `running()`. If the settled run still counted as in flight, the gate would
            // swallow the final report (the worker would look busy from beyond the grave).
            inflight.fetch_sub(1, Ordering::SeqCst);
            let _ = events.send(ManagerEvent::worker(worker_id, objective, status, summary));
        });
        id
    }

    /// Number of runs currently in flight (used to gate premature "all done" replies).
    pub fn running(&self) -> usize {
        self.inflight.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::runner::{LoginStatus, RunOutcome, RunnerError};
    use async_trait::async_trait;
    use tokio::sync::mpsc;

    struct ScriptedRunner {
        response: String,
        ok: bool,
    }

    #[async_trait]
    impl Runner for ScriptedRunner {
        async fn run(&self, _args: RunArgs) -> Result<RunOutcome, RunnerError> {
            Ok(RunOutcome {
                ok: self.ok,
                final_response: self.response.clone(),
                thread_id: None,
                usage: crate::runtime::TokenUsage {
                    input_tokens: 700,
                    output_tokens: 70,
                    ..Default::default()
                },
            })
        }
        async fn login_status(&self) -> LoginStatus {
            LoginStatus {
                ok: true,
                detail: "ok".into(),
            }
        }
    }

    #[tokio::test]
    async fn emits_one_event_and_retires() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let tel = Arc::new(Mutex::new(Telemetry::new()));
        let orch = Orchestrator::new(
            Arc::new(ScriptedRunner {
                response: "### SUMMARY FOR MANAGER\nPASS — done".into(),
                ok: true,
            }),
            PathBuf::from("/tmp/ws"),
            tx,
            tel.clone(),
            None,
        );
        orch.start("build the thing".into(), None);
        let event = rx.recv().await.expect("one event");
        match event {
            ManagerEvent::WorkerEvent {
                status,
                summary,
                objective,
                ..
            } => {
                assert_eq!(status, WorkerStatus::Completed);
                assert_eq!(summary, "PASS — done");
                assert_eq!(objective, "build the thing");
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert_eq!(orch.running(), 0, "retired before emit");
        let meter = tel.lock().unwrap().meter();
        assert_eq!(meter.worker_turns, 1);
        // The worker's token usage was folded into the worker totals (separate from the manager's).
        assert_eq!(meter.worker_input_tokens, 700);
        assert_eq!(meter.worker_total_tokens(), 770);
    }

    #[tokio::test]
    async fn failed_run_reports_failure() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let tel = Arc::new(Mutex::new(Telemetry::new()));
        let orch = Orchestrator::new(
            Arc::new(ScriptedRunner {
                response: "boom".into(),
                ok: false,
            }),
            PathBuf::from("/tmp/ws"),
            tx,
            tel,
            None,
        );
        orch.start("obj".into(), None);
        let event = rx.recv().await.unwrap();
        assert!(matches!(
            event,
            ManagerEvent::WorkerEvent {
                status: WorkerStatus::Failed,
                ..
            }
        ));
    }
}
