//! Background workers: the `worker_start` / `worker_cancel` executor and the
//! dispatcher work that runs a worker's turn and continues its parent.
//!
//! The executor injected into a parent's tool runtime is synchronous: it
//! persists the worker session and record and sends the turn to the team
//! dispatcher, so `worker_start` returns at once and no executor future ever
//! awaits `chat` (which builds the executors). The dispatcher runs the turn as
//! an ordinary interactive EPS chat on the worker session, records the
//! settlement on the parent, and, once the parent is idle, continues the
//! parent with [`crate::worker::WORKER_CONTINUE_TEXT`].

use super::{team::TeamCommand, team::TeamDispatcher, Phase, SessionEngineManager};
use super::{AgentEngineError, SessionEventSink};
use crate::{
    ipc,
    session::SessionStore,
    tool_exec::WorkerAction,
    worker::{WorkerEvent, WorkerRecord, WorkerStatus, WorkerTurnOutcome},
};

/// Work the dispatcher performs for workers.
pub(crate) enum WorkerCommand {
    /// Run a queued worker's turn.
    Run {
        parent_id: String,
        worker_id: String,
        child_session_id: String,
        client_turn_id: String,
        text: String,
    },
    /// Stop a worker session's running turn.
    Cancel { child_session_id: String },
    /// Continue the parent after a worker settled.
    Continue {
        parent_id: String,
        worker_id: String,
    },
}

/// Announce a worker to its parent session's panel as a `worker` event.
pub(crate) fn emit_worker(app: &tauri::AppHandle, parent_id: &str, event: WorkerEvent) {
    if let Err(error) = SessionEventSink::new(app.clone(), parent_id).emit_scoped("worker", event) {
        eprintln!("eud-agent: worker event failed: session={parent_id} error={error}");
    }
}

/// Everything one parent's worker executor needs.
#[derive(Clone)]
pub(super) struct WorkerContext {
    pub app: tauri::AppHandle,
    pub dispatcher: TeamDispatcher,
    pub sessions: SessionStore,
    pub session_id: String,
}

impl WorkerContext {
    pub(super) fn execute(&self, action: WorkerAction) -> Result<WorkerRecord, String> {
        match action {
            WorkerAction::Start {
                parent_request_id,
                goal,
                title,
            } => self.start(parent_request_id, goal, title),
            WorkerAction::Cancel { worker_id } => {
                let (before, cancelled) =
                    cancel_worker(&self.sessions, &self.session_id, &worker_id)?;
                if before == WorkerStatus::Running {
                    let _ = self
                        .dispatcher
                        .send(TeamCommand::Worker(WorkerCommand::Cancel {
                            child_session_id: cancelled.child_session_id.clone(),
                        }));
                }
                emit_worker(
                    &self.app,
                    &self.session_id,
                    WorkerEvent::status(cancelled.clone()),
                );
                Ok(cancelled)
            }
        }
    }

    fn start(
        &self,
        parent_request_id: String,
        goal: String,
        title: String,
    ) -> Result<WorkerRecord, String> {
        let parent = self
            .sessions
            .load(&self.session_id)
            .map_err(|error| format!("the parent session could not be loaded: {error}"))?;
        let child = crate::worker::child_session(&parent, &title);
        let worker = WorkerRecord::new(child.meta.id.clone(), parent_request_id, title, goal);
        // The limit is checked and the worker recorded under one store lock,
        // so parallel `worker_start` calls cannot exceed it.
        match self.sessions.add_worker(
            &self.session_id,
            worker.clone(),
            crate::worker::MAX_ACTIVE_WORKERS,
        ) {
            Ok(Ok(())) => {}
            Ok(Err(active)) => return Err(crate::worker::limit_refusal(active)),
            Err(error) => return Err(format!("the worker could not be saved: {error}")),
        }
        if let Err(error) = self.sessions.save(&child) {
            self.abandon(
                &worker,
                format!("the worker session could not be saved: {error}"),
            );
            return Err(format!("the worker session could not be saved: {error}"));
        }
        let client_turn_id = ipc::new_client_turn_id();
        // The worker session's user message is the goal alone; the worker
        // rules reach the model as the engine's `[background worker]` section.
        let text = worker.goal.clone();
        // The panel registers the worker session and records its turn as
        // sent from this event, before the turn's own stream begins.
        emit_worker(
            &self.app,
            &self.session_id,
            WorkerEvent {
                worker: worker.clone(),
                session: Some(child.meta.clone()),
                client_turn_id: Some(client_turn_id.clone()),
                text: Some(text.clone()),
                continuation: None,
            },
        );
        if self
            .dispatcher
            .send(TeamCommand::Worker(WorkerCommand::Run {
                parent_id: self.session_id.clone(),
                worker_id: worker.id.clone(),
                child_session_id: child.meta.id.clone(),
                client_turn_id,
                text,
            }))
            .is_err()
        {
            let reason = "the worker dispatcher is not running".to_string();
            self.abandon(&worker, reason.clone());
            // The worker session never ran a turn: nothing in it is worth
            // keeping, and a session no worker will ever run would only clutter
            // the session list.
            if let Err(error) = self.sessions.delete(&child.meta.id) {
                eprintln!(
                    "eud-agent: unused worker session {} could not be deleted: {error}",
                    child.meta.id
                );
            }
            return Err(reason);
        }
        Ok(worker)
    }

    /// Record a worker that could not be started as failed, already
    /// reported (the tool call returns the error, so no continuation turn is
    /// owed), and announce it.
    fn abandon(&self, worker: &WorkerRecord, reason: String) {
        match self
            .sessions
            .update_worker(&self.session_id, &worker.id, |worker| {
                worker.status = WorkerStatus::Failed { reason };
                worker.reported = true;
                true
            }) {
            Ok(Some(failed)) => {
                emit_worker(&self.app, &self.session_id, WorkerEvent::status(failed))
            }
            Ok(None) => {}
            Err(error) => eprintln!(
                "eud-agent: worker {} could not be marked failed: {error}",
                worker.id
            ),
        }
    }
}

/// Mark one queued or running worker cancelled. Returns its status before the
/// cancel (a running one also needs its turn stopped) and the updated record.
pub(super) fn cancel_worker(
    sessions: &SessionStore,
    parent_id: &str,
    worker_id: &str,
) -> Result<(WorkerStatus, WorkerRecord), String> {
    let mut before = None;
    let updated = sessions
        .update_worker(parent_id, worker_id, |worker| {
            before = Some(worker.status.clone());
            if !worker.status.is_active() {
                return false;
            }
            worker.status = WorkerStatus::Cancelled;
            true
        })
        .map_err(|error| format!("the worker could not be updated: {error}"))?
        .ok_or_else(|| format!("worker '{worker_id}' does not exist on this session"))?;
    let before = before.unwrap_or(WorkerStatus::Cancelled);
    if !before.is_active() {
        return Err(format!(
            "worker '{worker_id}' is already {}; only a queued or running worker can be cancelled",
            before.label()
        ));
    }
    Ok((before, updated))
}

/// Send the parent's next worker continuation to the dispatcher when a
/// settlement is still unreported and the command's turn was not cancelled.
/// Returns the worker it was sent for.
pub(super) fn dispatch_continuation(
    dispatcher: &TeamDispatcher,
    sessions: &SessionStore,
    parent_id: &str,
    turn_cancelled: bool,
) -> Option<String> {
    let record = sessions.load(parent_id).ok()?;
    let worker_id = crate::worker::continuation_after_command(&record, turn_cancelled)?;
    dispatcher
        .send(TeamCommand::Worker(WorkerCommand::Continue {
            parent_id: parent_id.to_string(),
            worker_id: worker_id.clone(),
        }))
        .ok()?;
    Some(worker_id)
}

/// Run one worker from queued to its settlement: mark it running (unless it
/// was cancelled while queued), run `turn`, and record the outcome. Every
/// status change is announced through `emit`. Returns the final record.
pub(super) async fn drive_worker<Fut>(
    sessions: &SessionStore,
    parent_id: &str,
    worker_id: &str,
    emit: impl Fn(&WorkerRecord),
    turn: impl FnOnce() -> Fut,
) -> Result<WorkerRecord, String>
where
    Fut: std::future::Future<Output = Result<WorkerTurnOutcome, String>>,
{
    let started = sessions
        .update_worker(parent_id, worker_id, |worker| {
            if worker.status != WorkerStatus::Queued {
                return false;
            }
            worker.status = WorkerStatus::Running;
            true
        })
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("worker {worker_id} vanished before it ran"))?;
    if started.status != WorkerStatus::Running {
        return Ok(started);
    }
    emit(&started);
    let outcome = turn().await;
    let settled = sessions
        .update_worker(parent_id, worker_id, |worker| {
            crate::worker::settle(worker, outcome);
            true
        })
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("worker {worker_id} vanished before it settled"))?;
    emit(&settled);
    Ok(settled)
}

/// Carry out one dispatcher command.
pub(super) async fn handle(command: WorkerCommand, engines: SessionEngineManager) {
    match command {
        WorkerCommand::Run {
            parent_id,
            worker_id,
            child_session_id,
            client_turn_id,
            text,
        } => {
            let sessions = engines.inner.sessions.clone();
            let app = engines.inner.app.clone();
            let settled = drive_worker(
                &sessions,
                &parent_id,
                &worker_id,
                |worker| emit_worker(&app, &parent_id, WorkerEvent::status(worker.clone())),
                || {
                    engines.worker_turn(
                        &parent_id,
                        &worker_id,
                        &child_session_id,
                        client_turn_id,
                        text,
                    )
                },
            )
            .await;
            match settled {
                Ok(worker) if worker.status.continues_interactively() => {
                    let _ = engines.inner.team_dispatcher.send(TeamCommand::Worker(
                        WorkerCommand::Continue {
                            parent_id,
                            worker_id,
                        },
                    ));
                }
                Ok(_) => {}
                Err(error) => eprintln!(
                    "eud-agent: worker {worker_id} of session {parent_id} could not settle: {error}"
                ),
            }
        }
        WorkerCommand::Cancel { child_session_id } => {
            if let Err(error) = engines.cancel(&child_session_id).await {
                eprintln!(
                    "eud-agent: worker session {child_session_id} could not be cancelled: {}",
                    error.message
                );
            }
        }
        WorkerCommand::Continue {
            parent_id,
            worker_id,
        } => {
            if let Err(error) = engines.worker_continue(&parent_id, &worker_id).await {
                eprintln!(
                    "eud-agent: session {parent_id} could not continue after worker {worker_id} settled: {}",
                    error.message
                );
            }
        }
    }
}

impl SessionEngineManager {
    /// One worker session's turn: an ordinary interactive EPS chat, with what
    /// it produced read from the engine before anything else can use it. A
    /// turn that failed after it began still reports what it committed.
    async fn worker_turn(
        &self,
        parent_id: &str,
        worker_id: &str,
        child_session_id: &str,
        client_turn_id: String,
        text: String,
    ) -> Result<WorkerTurnOutcome, String> {
        let worker = self
            .worker(child_session_id)
            .await
            .map_err(|error| error.message)?;
        let _provider_busy = self.inner.provider_service.enter_busy(worker.provider);
        // The generation before waiting for the engine: a cancel issued while
        // this turn waits bumps it, and the turn then stops at its first check.
        let generation = *worker.cancellation.borrow();
        let (result, mut outcome) = {
            let mut engine = worker.engine.lock().await;
            // A cancel that reached the engine first (the worker's own stop, or
            // its parent's) already marked the worker: run no turn at all.
            let parent = self.inner.sessions.load(parent_id).ok();
            if !crate::worker::worker_turn_may_start(parent.as_ref(), worker_id) {
                return Ok(WorkerTurnOutcome {
                    cancelled: true,
                    ..Default::default()
                });
            }
            if let Err(error) = self.inner.sessions.touch_conversation(child_session_id) {
                eprintln!("eud-agent: conversation timestamp update failed: {error}");
            }
            worker
                .runtime
                .emit_activity(crate::write_coordinator::SessionActivity::RunningRead);
            let result = engine
                .chat_from_generation(
                    ipc::ChatRequest {
                        client_turn_id,
                        text,
                        attachments: Vec::new(),
                        mentions: Vec::new(),
                        execution_mode: crate::autonomous::ExecutionMode::Interactive,
                        autonomous_policy: None,
                        clarify: false,
                    },
                    generation,
                )
                .await;
            (result, engine.worker_turn_outcome())
        };
        if let Err(error) = self.finish_read_command(&worker, result).await {
            outcome.error = Some(error.message);
        }
        Ok(outcome)
    }

    /// Continue a parent EPS session after one of its workers settled: one
    /// ordinary interactive turn on the fixed continuation text, only once the
    /// parent is idle with no live autonomous run and no pending review. A
    /// parent that is busy picks the settlement up after its current turn.
    pub(crate) async fn worker_continue(
        &self,
        parent_id: &str,
        worker_id: &str,
    ) -> Result<(), AgentEngineError> {
        let record = self
            .inner
            .sessions
            .load(parent_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        if crate::worker::next_continuation(&record).is_none() {
            return Ok(());
        }
        let worker = self.worker(parent_id).await?;
        let _provider_busy = self.inner.provider_service.enter_busy(worker.provider);
        // Awaiting the engine waits out the parent turn that is running.
        let mut engine = worker.engine.lock().await;
        // Re-read under the engine lock: the turn that just ended may have
        // reported the settlement already.
        let record = self
            .inner
            .sessions
            .load(parent_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let Some(target) =
            crate::worker::continuation_target(&record, worker_id, engine.phase == Phase::Idle)
        else {
            return Ok(());
        };
        let client_turn_id = ipc::new_client_turn_id();
        let text = crate::worker::WORKER_CONTINUE_TEXT.to_string();
        let mut event = WorkerEvent::status(target);
        event.continuation = Some(crate::team::TeamTaskContinuation {
            client_turn_id: client_turn_id.clone(),
            text: text.clone(),
        });
        if let Err(error) = worker.sink.emit_scoped("worker", event) {
            eprintln!(
                "eud-agent: worker continuation event failed: session={parent_id} error={error}"
            );
        }
        if let Err(error) = self.inner.sessions.touch_conversation(parent_id) {
            eprintln!("eud-agent: conversation timestamp update failed: {error}");
        }
        worker
            .runtime
            .emit_activity(crate::write_coordinator::SessionActivity::RunningRead);
        let result = engine
            .chat(ipc::ChatRequest {
                client_turn_id,
                text,
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: crate::autonomous::ExecutionMode::Interactive,
                autonomous_policy: None,
                clarify: false,
            })
            .await;
        let cancelled = engine.last_turn_cancelled;
        drop(engine);
        // Only a continuation that succeeded and was not stopped looks for the
        // next settlement: a failing one (a logged-out provider) must not start
        // itself again, and a Stop must not start the next one.
        let finished = self.settle_read_command(&worker, result).await;
        if finished.is_ok() {
            self.continue_unreported_workers(parent_id, cancelled);
        }
        finished
    }

    /// After a parent command: a worker that settled while it ran is still
    /// unreported, so continue the parent again, unless the user cancelled
    /// the command's turn.
    pub(super) fn continue_unreported_workers(&self, parent_id: &str, turn_cancelled: bool) {
        dispatch_continuation(
            &self.inner.team_dispatcher,
            &self.inner.sessions,
            parent_id,
            turn_cancelled,
        );
    }

    /// A turn of the parent was cancelled: EVERY queued or running worker of
    /// the parent session is cancelled with it, whichever turn started it
    /// (the parent stops, its children stop, as with Claude Code's and Codex's
    /// subagents; `parent_request_id` is informational). A cancelled worker
    /// never continues its parent. Running workers are stopped through the
    /// dispatcher, so the parent's cancel never waits for a worker's turn.
    pub(super) fn cancel_workers_of(&self, parent_id: &str) {
        let cancelled = match self.inner.sessions.cancel_active_workers(parent_id) {
            Ok(cancelled) => cancelled,
            Err(error) => {
                eprintln!(
                    "eud-agent: workers of session {parent_id} could not be cancelled: {error}"
                );
                return;
            }
        };
        for before in cancelled {
            if before.status == WorkerStatus::Running {
                let _ =
                    self.inner
                        .team_dispatcher
                        .send(TeamCommand::Worker(WorkerCommand::Cancel {
                            child_session_id: before.child_session_id.clone(),
                        }));
            }
            let mut after = before;
            after.status = WorkerStatus::Cancelled;
            emit_worker(&self.inner.app, parent_id, WorkerEvent::status(after));
        }
    }

    /// The stop of a worker session cancels its worker, so a turn that was
    /// still waiting never runs and a settled one never continues the parent.
    pub(super) fn cancel_own_worker(&self, session_id: &str) {
        let Some(parent_id) = self
            .inner
            .sessions
            .load(session_id)
            .ok()
            .and_then(|record| record.meta.worker_parent)
        else {
            return;
        };
        let worker_id = self
            .inner
            .sessions
            .load(&parent_id)
            .ok()
            .and_then(|parent| {
                parent
                    .workers
                    .into_iter()
                    .find(|worker| {
                        worker.child_session_id == session_id && worker.status.is_active()
                    })
                    .map(|worker| worker.id)
            });
        if let Some(worker_id) = worker_id {
            if let Ok((_, cancelled)) = cancel_worker(&self.inner.sessions, &parent_id, &worker_id)
            {
                emit_worker(&self.inner.app, &parent_id, WorkerEvent::status(cancelled));
            }
        }
    }

    /// Whether deleting `session_id` must wait for workers: a parent with a
    /// queued or running worker, or a worker session its parent still counts
    /// as queued or running.
    pub(super) fn worker_delete_refusal(&self, session_id: &str) -> Option<String> {
        let record = self.inner.sessions.load(session_id).ok()?;
        if record
            .workers
            .iter()
            .any(|worker| worker.status.is_active())
        {
            return Some(
                "실행 중인 작업자가 있는 세션은 삭제할 수 없습니다. 작업자가 끝나거나 중단한 뒤 다시 삭제해 주세요."
                    .to_string(),
            );
        }
        let parent = record.meta.worker_parent.as_deref()?;
        let active = self
            .inner
            .sessions
            .load(parent)
            .ok()?
            .workers
            .iter()
            .any(|worker| worker.child_session_id == session_id && worker.status.is_active());
        active.then(|| {
            "실행 중인 작업자 세션은 삭제할 수 없습니다. 작업자를 중단한 뒤 다시 삭제해 주세요."
                .to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (std::path::PathBuf, SessionStore) {
        let base =
            std::env::temp_dir().join(format!("eud-agent-worker-drive-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let dirs = crate::config::DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        (base, SessionStore::new(&dirs))
    }

    fn parent_with(
        sessions: &SessionStore,
        worker: &WorkerRecord,
    ) -> crate::session::SessionRecord {
        let binding = crate::provider::ProviderBinding::new(
            crate::provider::ProviderId::Codex,
            "gpt-test".to_string(),
            None,
        )
        .unwrap();
        let record = crate::session::SessionRecord {
            meta: crate::session::SessionMeta {
                id: "parent".to_string(),
                name: "parent".to_string(),
                project: "project".to_string(),
                kind: crate::session::SessionKind::Eps,
                provider: binding.provider,
                model: binding.model.clone(),
                created_at: 1,
                last_conversation_at: 1,
                team_parent: None,
                worker_parent: None,
                project_id: None,
                workspace_id: None,
            },
            provider_binding: binding,
            pending_request_ids: Vec::new(),
            context_usage: None,
            panel_log: serde_json::Value::Null,
            context_state: Default::default(),
            task_state: Default::default(),
            autonomous_run: None,
            team_tasks: Vec::new(),
            workers: vec![worker.clone()],
        };
        sessions.save(&record).unwrap();
        record
    }

    fn queued() -> WorkerRecord {
        WorkerRecord::new(
            "child".to_string(),
            "req".to_string(),
            "메뉴".to_string(),
            "src/menu.eps 정리".to_string(),
        )
    }

    #[tokio::test]
    async fn a_worker_runs_from_queued_through_running_to_settled() {
        let (base, sessions) = store();
        let worker = queued();
        parent_with(&sessions, &worker);
        let seen = std::sync::Mutex::new(Vec::new());
        let running_during_turn = std::sync::Mutex::new(None);
        let settled = drive_worker(
            &sessions,
            "parent",
            &worker.id,
            |record| seen.lock().unwrap().push(record.status.label()),
            || async {
                *running_during_turn.lock().unwrap() = sessions
                    .load("parent")
                    .ok()
                    .map(|record| record.workers[0].status.clone());
                Ok(WorkerTurnOutcome {
                    answer: "메뉴를 정리했습니다.".to_string(),
                    cancelled: false,
                    commits: vec!["abc123".to_string()],
                    changed_files: vec!["src/menu.eps".to_string()],
                    build_ok: Some(true),
                    error: None,
                })
            },
        )
        .await
        .unwrap();
        assert_eq!(
            *running_during_turn.lock().unwrap(),
            Some(WorkerStatus::Running)
        );
        assert_eq!(*seen.lock().unwrap(), vec!["running", "settled"]);
        assert_eq!(settled.status, WorkerStatus::Settled);
        assert_eq!(settled.summary.as_deref(), Some("메뉴를 정리했습니다."));
        let stored = sessions.load("parent").unwrap().workers.remove(0);
        assert_eq!(stored.commits, vec!["abc123".to_string()]);
        assert_eq!(stored.changed_files, vec!["src/menu.eps".to_string()]);
        assert_eq!(stored.build_ok, Some(true));
        assert!(!stored.reported);
        assert_eq!(
            crate::worker::next_continuation(&sessions.load("parent").unwrap()).as_deref(),
            Some(worker.id.as_str())
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn a_worker_cancelled_while_queued_never_runs_and_never_continues() {
        let (base, sessions) = store();
        let worker = queued();
        parent_with(&sessions, &worker);
        let (before, cancelled) = cancel_worker(&sessions, "parent", &worker.id).unwrap();
        assert_eq!(before, WorkerStatus::Queued);
        assert_eq!(cancelled.status, WorkerStatus::Cancelled);
        let ran = std::sync::atomic::AtomicBool::new(false);
        let settled = drive_worker(
            &sessions,
            "parent",
            &worker.id,
            |_| {},
            || async {
                ran.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(WorkerTurnOutcome::default())
            },
        )
        .await
        .unwrap();
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(settled.status, WorkerStatus::Cancelled);
        assert!(!settled.status.continues_interactively());
        assert!(crate::worker::next_continuation(&sessions.load("parent").unwrap()).is_none());
        let again = cancel_worker(&sessions, "parent", &worker.id).unwrap_err();
        assert!(again.contains("already cancelled"), "{again}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn a_worker_cancelled_while_running_stays_cancelled_with_its_commits() {
        let (base, sessions) = store();
        let worker = queued();
        parent_with(&sessions, &worker);
        let settled = drive_worker(
            &sessions,
            "parent",
            &worker.id,
            |_| {},
            || async {
                let (before, _) = cancel_worker(&sessions, "parent", &worker.id).unwrap();
                assert_eq!(before, WorkerStatus::Running);
                Ok(WorkerTurnOutcome {
                    cancelled: true,
                    commits: vec!["partial".to_string()],
                    ..Default::default()
                })
            },
        )
        .await
        .unwrap();
        assert_eq!(settled.status, WorkerStatus::Cancelled);
        assert_eq!(settled.commits, vec!["partial".to_string()]);
        assert!(crate::worker::next_continuation(&sessions.load("parent").unwrap()).is_none());
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn a_failed_worker_turn_settles_failed_and_continues_the_parent() {
        let (base, sessions) = store();
        let worker = queued();
        parent_with(&sessions, &worker);
        let settled = drive_worker(
            &sessions,
            "parent",
            &worker.id,
            |_| {},
            || async { Err::<WorkerTurnOutcome, _>("provider down".to_string()) },
        )
        .await
        .unwrap();
        assert_eq!(
            settled.status,
            WorkerStatus::Failed {
                reason: "provider down".to_string()
            }
        );
        assert!(crate::worker::next_continuation(&sessions.load("parent").unwrap()).is_some());
        let _ = std::fs::remove_dir_all(base);
    }
}
