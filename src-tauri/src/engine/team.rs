//! EPS → Map team handoff: the `map_task_request` tool's executor.
//!
//! The tool runtime admits the call (evidence, exclusion, scope); this module
//! owns the durable [`TeamTask`], the team Map session, the Map request, and
//! the bounded wait. The Map request itself is an ordinary `map_chat` on the
//! team session, so the candidate lifecycle, stale checks, and the user's
//! trusted Apply are exactly the Map window's. A task that revises an earlier
//! task's result continues that task's team session; every other task gets a
//! fresh team session on the saved source map, and the parent's earlier team
//! sessions are retired unless the Map window can still undo their Apply.
//!
//! Map requests run on a dispatcher task spawned once with the session
//! manager. The executor injected into a session's tool runtime only sends
//! commands to it: the manager's `worker()` builds that executor, so the
//! executor must not itself await `map_chat` (which builds workers), or the
//! two futures would define each other.

use tauri::Manager;

use super::{SessionEngineManager, SessionEventSink};
use crate::{
    map_agent::MapAgentService,
    map_candidate::CandidateStateView,
    map_model::MapMentionSnapshot,
    session::SessionStore,
    team::{TeamTask, TeamTaskEvent, TeamTaskStatus, TEAM_TASK_WAIT_TIMEOUT},
    tool_exec::{TeamTaskAction, TeamTaskRequest},
};

/// One Map request the dispatcher runs on behalf of an EPS session.
pub(crate) struct TeamRun {
    pub map_session_id: String,
    pub request_id: String,
    pub mentions: Vec<MapMentionSnapshot>,
    pub parent_name: String,
    pub parent_id: String,
    pub task: TeamTask,
    /// The earlier task this request revises in the same team session.
    pub revises: Option<String>,
    /// The request's target scope rows (`team_scope_rows`), if any.
    pub scope: Option<Vec<crate::map_model::RowSpan>>,
    pub reply: tokio::sync::oneshot::Sender<Result<CandidateStateView, String>>,
}

/// One unit of Map work the dispatcher performs on behalf of an EPS session.
pub(crate) enum TeamCommand {
    Run(Box<TeamRun>),
    Cancel {
        map_session_id: String,
    },
    /// A task settled while the EPS session's autonomous run was paused for
    /// it: continue that run (the dispatcher, not the settler, awaits the
    /// engine so no worker future refers to itself).
    ResumeAutonomous {
        session_id: String,
    },
    /// A task settled after its `map_task_request` had returned `running`:
    /// continue the EPS conversation from the durable task state once the
    /// session is idle (the dispatcher awaits the engine, not the settler).
    ContinueInteractive {
        session_id: String,
        task_id: String,
    },
}

pub(crate) type TeamDispatcher = tokio::sync::mpsc::UnboundedSender<TeamCommand>;

/// The dispatcher loop: every `Run` becomes its own task so one long Map
/// request never delays another session's cancel.
pub(crate) async fn dispatch(
    mut commands: tokio::sync::mpsc::UnboundedReceiver<TeamCommand>,
    engines: SessionEngineManager,
) {
    while let Some(command) = commands.recv().await {
        let engines = engines.clone();
        match command {
            TeamCommand::Run(run) => {
                let TeamRun {
                    map_session_id,
                    request_id,
                    mentions,
                    parent_name,
                    parent_id,
                    task,
                    revises,
                    scope,
                    reply,
                } = *run;
                tokio::spawn(async move {
                    let service = (*engines.inner.app.state::<MapAgentService>()).clone();
                    let outcome = service
                        .run_team_request(
                            &engines,
                            &map_session_id,
                            &request_id,
                            mentions,
                            &parent_name,
                            &parent_id,
                            &task,
                            revises.as_deref(),
                            scope,
                        )
                        .await;
                    if let Ok(state) = &outcome {
                        crate::map_agent::notify_map_window_candidate(&engines.inner.app, state);
                    }
                    let _ = reply.send(outcome);
                });
            }
            TeamCommand::Cancel { map_session_id } => {
                tokio::spawn(async move {
                    if let Err(error) = engines.cancel_map_session(&map_session_id).await {
                        eprintln!("eud-agent: team map request cancel failed: {error}");
                    }
                });
            }
            TeamCommand::ResumeAutonomous { session_id } => {
                tokio::spawn(async move {
                    if let Err(error) = engines.autonomous_resume(&session_id).await {
                        eprintln!(
                            "eud-agent: autonomous run could not continue after its team map task settled: session={session_id} error={}",
                            error.message
                        );
                    }
                });
            }
            TeamCommand::ContinueInteractive {
                session_id,
                task_id,
            } => {
                tokio::spawn(async move {
                    if let Err(error) = engines.team_continue(&session_id, &task_id).await {
                        eprintln!(
                            "eud-agent: EPS session could not continue after its team map task settled: session={session_id} task={task_id} error={}",
                            error.message
                        );
                    }
                });
            }
        }
    }
}

/// Everything one EPS session's team executor needs beyond the admitted call.
#[derive(Clone)]
pub(super) struct TeamHandoffContext {
    pub app: tauri::AppHandle,
    pub dispatcher: TeamDispatcher,
    pub sessions: SessionStore,
    pub session_id: String,
    pub cancellation: tokio::sync::watch::Receiver<u64>,
}

/// Announce one task to its EPS session's panel as a `team_task` event.
pub(crate) fn emit_team_task(app: &tauri::AppHandle, session_id: &str, task: &TeamTask) {
    if let Err(error) = SessionEventSink::new(app.clone(), session_id).emit_scoped(
        "team_task",
        TeamTaskEvent {
            task: task.clone(),
            continuation: None,
        },
    ) {
        eprintln!("eud-agent: team task event failed: session={session_id} error={error}");
    }
}

/// Announce every settled task (from a Map window Apply/discard/undo).
pub(crate) fn emit_team_tasks(app: &tauri::AppHandle, tasks: Vec<(String, TeamTask)>) {
    for (session_id, task) in tasks {
        emit_team_task(app, &session_id, &task);
    }
}

/// Map one Map request outcome onto the task status the EPS session records.
pub(crate) fn settle_status(
    before_revision: u32,
    outcome: Result<CandidateStateView, String>,
) -> (TeamTaskStatus, Option<crate::team::TeamCandidateSummary>) {
    match outcome {
        Ok(state) => match MapAgentService::team_candidate_summary(before_revision, &state) {
            Some(candidate) => (TeamTaskStatus::CandidateReady, Some(candidate)),
            None => (
                TeamTaskStatus::Failed {
                    reason: "맵 에이전트가 후보 리비전을 만들지 않았습니다. 맵 창의 대화에서 이유를 확인하세요."
                        .to_string(),
                },
                None,
            ),
        },
        Err(error) if error.contains("cancel") => (TeamTaskStatus::Cancelled, None),
        Err(error) => (TeamTaskStatus::Failed { reason: error }, None),
    }
}

impl TeamHandoffContext {
    /// Hand a superseded candidate back to the task it was revised from, when
    /// its team session still shows exactly that candidate revision.
    fn restore_revised(&self, revised_id: &str) {
        // Read the candidate state before the store update: the update holds
        // the session store lock, which reading a session would take again.
        let intact = self
            .sessions
            .load(&self.session_id)
            .ok()
            .and_then(|record| {
                record
                    .team_tasks
                    .into_iter()
                    .find(|task| task.id == revised_id)
            })
            .filter(|task| task.status == TeamTaskStatus::Superseded)
            .and_then(|task| {
                let candidate = task.candidate?;
                let service = (*self.app.state::<MapAgentService>()).clone();
                service
                    .team_session_state(&task.map_session_id)
                    .ok()
                    .map(|state| state.current_revision == candidate.revision)
            })
            .unwrap_or(false);
        if !intact {
            return;
        }
        match self
            .sessions
            .update_team_task(&self.session_id, revised_id, |task| {
                let superseded = task.status == TeamTaskStatus::Superseded;
                if superseded {
                    task.status = TeamTaskStatus::CandidateReady;
                }
                superseded
            }) {
            Ok(Some(task)) if task.status == TeamTaskStatus::CandidateReady => {
                emit_team_task(&self.app, &self.session_id, &task)
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!(
                    "eud-agent: revised team task {revised_id} could not be restored: {error}"
                )
            }
        }
    }

    fn persist(&self, task: &TeamTask) -> Result<(), String> {
        self.sessions
            .upsert_team_task(&self.session_id, task.clone())
            .map_err(|error| format!("team task could not be saved: {error}"))?;
        emit_team_task(&self.app, &self.session_id, task);
        Ok(())
    }

    /// Run one admitted `map_task_request`. Returns the task once the Map
    /// session committed its candidate (or failed), or the still-running task
    /// when the bounded wait elapses first; the Map request then settles in
    /// the background and the durable task status follows it.
    pub(super) async fn run(self, request: TeamTaskRequest) -> Result<TeamTask, String> {
        let parent = self
            .sessions
            .load(&self.session_id)
            .map_err(|error| format!("the EPS session could not be loaded: {error}"))?;
        let service = (*self.app.state::<MapAgentService>()).clone();
        let revised = match &request.revises_task_id {
            Some(id) => Some(
                parent
                    .team_tasks
                    .iter()
                    .find(|task| &task.id == id)
                    .cloned()
                    .ok_or_else(|| format!("team task {id} does not exist in this session"))?,
            ),
            None => None,
        };
        let (team, state) = match &revised {
            // A follow-up edit continues the revised task's team session: its
            // conversation, and its candidate while that is still ready.
            Some(revised) => {
                // Another task's ready candidate would stay live in its own
                // session, so the decision on it comes first.
                if let Some(other) = parent.team_tasks.iter().find(|task| {
                    task.status == TeamTaskStatus::CandidateReady && task.id != revised.id
                }) {
                    return Err(format!(
                        "team task {} is candidate_ready; apply, discard, or revise it before revising task {}",
                        other.id, revised.id
                    ));
                }
                service.continued_team_session(&parent, revised)?
            }
            // Any other task runs in a fresh team session on the saved source
            // map: nothing from an earlier request (its provider thread,
            // transcript, or unapplied candidate) stacks under this one.
            None => {
                let (team, state, retired) = service.fresh_team_session(&parent)?;
                let engines = (*self.app.state::<SessionEngineManager>()).clone();
                for earlier in retired {
                    if let Err(error) = engines.retire_team_session(&earlier.id).await {
                        eprintln!(
                            "eud-agent: earlier team map session {} could not be retired: {error}",
                            earlier.id
                        );
                    }
                }
                (team, state)
            }
        };
        let mentions =
            MapAgentService::team_mentions(&state, &request.selection_ids, &request.location_ids)?;
        let scope = MapAgentService::team_scope_rows(
            state.baseline.width,
            state.baseline.height,
            &request.target,
            &request.protect,
        )?;
        // A ready candidate is superseded by this task: a fresh session
        // retired its session above, and a revision continues it under the
        // new task (and gets it back if the revision produces nothing).
        let revised_ready = revised
            .as_ref()
            .filter(|task| task.status == TeamTaskStatus::CandidateReady)
            .map(|task| task.id.clone());
        for ready in parent
            .team_tasks
            .iter()
            .filter(|task| task.status == TeamTaskStatus::CandidateReady)
        {
            match self
                .sessions
                .update_team_task(&self.session_id, &ready.id, |task| {
                    task.status = TeamTaskStatus::Superseded;
                    true
                }) {
                Ok(Some(updated)) => emit_team_task(&self.app, &self.session_id, &updated),
                Ok(None) => {}
                Err(error) => {
                    return Err(format!(
                        "the earlier map task could not be superseded: {error}"
                    ))
                }
            }
        }
        let now = crate::session::now_unix_millis();
        let suffix = uuid::Uuid::new_v4();
        let request_id = format!("map-{suffix}");
        let mut task = TeamTask {
            id: format!("task-{suffix}"),
            parent_request_id: request.identity.request_id.clone(),
            map_session_id: team.meta.id.clone(),
            map_request_id: Some(request_id.clone()),
            goal: request.goal,
            layers: request.layers,
            selection_ids: request.selection_ids,
            location_ids: request.location_ids,
            source_map_sha256_at_create: state.baseline.file_sha256.clone(),
            status: TeamTaskStatus::Queued,
            candidate: None,
            applied_source_sha256: None,
            applied_by: None,
            created_at: now,
            updated_at: now,
        };
        // Until the settler owns it, a failure hands the revised candidate
        // back to its own task.
        let restore_on = |error: String| {
            if let Some(revised_id) = &revised_ready {
                self.restore_revised(revised_id);
            }
            error
        };
        self.persist(&task).map_err(restore_on)?;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.dispatcher
            .send(TeamCommand::Run(Box::new(TeamRun {
                map_session_id: task.map_session_id.clone(),
                request_id,
                mentions,
                parent_name: parent.meta.name.clone(),
                parent_id: parent.meta.id.clone(),
                task: task.clone(),
                revises: request.revises_task_id.clone(),
                scope,
                reply: reply_tx,
            })))
            .map_err(|_| restore_on("the team map dispatcher is not running".to_string()))?;
        task.status = TeamTaskStatus::Running;
        task.updated_at = crate::session::now_unix_millis();
        self.persist(&task)?;
        // The run is visible from its first event: bring the Map window up on
        // the team session now, exactly as if the user had typed the request.
        if let Err(error) = crate::map_agent::open_map_window(&self.app, Some(&task.map_session_id))
        {
            eprintln!(
                "eud-agent: map window could not be opened for team task {}: {error}",
                task.id
            );
        }

        // Settle the durable task whenever the Map request ends, whether or
        // not the tool call is still waiting for it. Once the call has
        // returned `running` (`detached`), nobody is left to act on the
        // settlement in the EPS turn, so the settler starts the conversation's
        // continuation turn instead.
        let (settled_tx, settled_rx) = tokio::sync::oneshot::channel::<TeamTask>();
        let detached = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let settler = {
            let context = self.clone();
            let task_id = task.id.clone();
            let before_revision = state.current_revision;
            let detached = detached.clone();
            let revised_ready = revised_ready.clone();
            async move {
                let outcome = match reply_rx.await {
                    Ok(outcome) => outcome,
                    Err(_) => Err("the map request ended without a result".to_string()),
                };
                let (status, candidate) = settle_status(before_revision, outcome);
                // A revision that produced no candidate left the revised
                // task's candidate as it was: that task is ready again.
                if let Some(revised_id) = revised_ready
                    .as_deref()
                    .filter(|_| status != TeamTaskStatus::CandidateReady)
                {
                    context.restore_revised(revised_id);
                }
                let settled =
                    context
                        .sessions
                        .update_team_task(&context.session_id, &task_id, |task| {
                            task.status = status.clone();
                            task.candidate = candidate.clone();
                            true
                        });
                match settled {
                    Ok(Some(updated)) => {
                        emit_team_task(&context.app, &context.session_id, &updated);
                        if autonomous_paused_for_team(&context.sessions, &context.session_id) {
                            let _ = context.dispatcher.send(TeamCommand::ResumeAutonomous {
                                session_id: context.session_id.clone(),
                            });
                        } else if detached.load(std::sync::atomic::Ordering::SeqCst)
                            && updated.status.continues_interactively()
                        {
                            let _ = context.dispatcher.send(TeamCommand::ContinueInteractive {
                                session_id: context.session_id.clone(),
                                task_id: updated.id.clone(),
                            });
                        }
                        if updated.status == TeamTaskStatus::CandidateReady {
                            // The decision is the user's: bring the Map window
                            // up on the team session so the candidate can be
                            // reviewed and applied or discarded.
                            if let Err(error) = crate::map_agent::open_map_window(
                                &context.app,
                                Some(&updated.map_session_id),
                            ) {
                                eprintln!(
                                    "eud-agent: map window could not be opened for team task {}: {error}",
                                    updated.id
                                );
                            }
                        }
                        let _ = settled_tx.send(updated);
                    }
                    Ok(None) => {
                        eprintln!("eud-agent: team task {task_id} vanished before it settled")
                    }
                    Err(error) => eprintln!("eud-agent: team task settlement failed: {error}"),
                }
            }
        };
        tokio::spawn(settler);

        let mut cancellation = self.cancellation.clone();
        let generation = request.identity.cancellation_generation;
        let deadline = tokio::time::sleep(TEAM_TASK_WAIT_TIMEOUT);
        tokio::pin!(deadline);
        tokio::pin!(settled_rx);
        loop {
            tokio::select! {
                biased;
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow_and_update() != generation {
                        // The EPS request was cancelled: cancel the Map request
                        // too; the settler records `cancelled` when it ends.
                        let _ = self.dispatcher.send(TeamCommand::Cancel {
                            map_session_id: task.map_session_id.clone(),
                        });
                        return Err("the map task was cancelled with its EPS request".to_string());
                    }
                }
                settled = &mut settled_rx => {
                    return settled.map_err(|_| "the map task ended without a status".to_string());
                }
                _ = &mut deadline => {
                    // Still running: the tool returns now so the native CLI
                    // call stays inside its ceiling; the task settles later
                    // and its settlement continues the conversation.
                    detached.store(true, std::sync::atomic::Ordering::SeqCst);
                    return self
                        .sessions
                        .load(&self.session_id)
                        .map_err(|error| error.to_string())?
                        .team_tasks
                        .into_iter()
                        .find(|existing| existing.id == task.id)
                        .ok_or_else(|| "the map task vanished while running".to_string());
                }
            }
        }
    }
}

/// The `[map tasks]` prompt note of one EPS session, when it has any task.
pub(crate) fn prompt_note(sessions: &SessionStore, session_id: &str) -> Option<String> {
    let record = sessions.load(session_id).ok()?;
    crate::team::prompt_note(&record.team_tasks)
}

/// Whether the session has a task that is still queued, running, or waiting
/// for a decision, so an autonomous run pauses instead of iterating past it.
pub(crate) fn has_active_task(sessions: &SessionStore, session_id: &str) -> bool {
    sessions
        .load(session_id)
        .map(|record| record.team_tasks.iter().any(|task| task.status.is_active()))
        .unwrap_or(false)
}

/// Whether the session's autonomous run is paused waiting for a team task.
fn autonomous_paused_for_team(sessions: &SessionStore, session_id: &str) -> bool {
    sessions
        .load(session_id)
        .ok()
        .and_then(|record| record.autonomous_run)
        .is_some_and(|run| {
            run.status == crate::autonomous::AutonomousRunStatus::Paused
                && run.pause_reason == Some(crate::autonomous::AutonomousPauseReason::TeamApply)
        })
}

/// The `map_task_apply` / `map_task_discard` executor: the Map service settles
/// the task, every changed task is announced, and an open Map window sees the
/// new candidate state.
pub(crate) fn run_action(
    app: &tauri::AppHandle,
    action: TeamTaskAction,
) -> Result<TeamTask, String> {
    let service = (*app.state::<MapAgentService>()).clone();
    let (updated, settled) = service.team_task_action(&action)?;
    emit_team_tasks(app, settled);
    match service.team_session_state(&updated.map_session_id) {
        Ok(state) => crate::map_agent::notify_map_window_candidate(app, &state),
        Err(error) => eprintln!(
            "eud-agent: team session state after {:?} failed: {error}",
            action.kind
        ),
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_team_request_settles_cancelled_and_never_continues() {
        let (status, candidate) =
            settle_status(0, Err(crate::team::TEAM_MAP_REQUEST_CANCELLED.to_string()));
        assert_eq!(status, TeamTaskStatus::Cancelled);
        assert!(candidate.is_none());
        assert!(!status.continues_interactively());
    }
}
