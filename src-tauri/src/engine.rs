//! Agent orchestration and prompt assembly.
//!
//! This module owns the pure v2 prompt assembly seam and the agentic turn loop.
//! Callers provide already-fetched RAG/project context so the prompt helpers remain
//! unit-testable without project I/O, RAG, or Codex I/O.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

mod context;
#[cfg(test)]
mod context_tests;
mod prompts;
pub(crate) mod runtime_events;
mod team;
mod worker;

#[cfg(test)]
use prompts::{selected_luck_defense_skill, tool_catalog_section};
use prompts::{selected_task_guidance, static_prompt_baseline, CLARIFY_REQUESTED_NOTE};

pub(crate) use team::emit_team_tasks;

#[cfg(test)]
use crate::provider_runtime::{AdapterEventKind, NormalizedBlock};
use crate::{
    attachment::{AttachmentContext, AttachmentStore},
    ipc, journal,
    provider_runtime::{
        AgentTurnInput, BindingSnapshot, CompactionRequest, ForegroundRequest, JobBase, RunId,
        RunIdentity, RunOutcome, RunPolicy, RuntimeExecutor, StructuredJobKind,
        StructuredJobRequest, WorkspaceAccess, DEFAULT_MAX_TOOL_ROUNDS,
        TASK_STATE_COMPILER_DEADLINE, TASK_STATE_COMPILER_OUTPUT_TOKENS,
    },
    tool_exec::SessionToolRuntime,
    workspace::{approved_plan_path, PreparedWorkspace},
};
use parking_lot::Mutex as SyncMutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTurnResult {
    Answer {
        text: String,
    },
    Plan {
        markdown: String,
    },
    /// The user interrupted the live provider turn. Any journaled writes stay
    /// reviewable, but no answer or plan event is emitted.
    Cancelled,
    WriteTransition,
    IterationBoundary {
        reason: crate::provider_runtime::IterationBoundaryReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEngineError {
    pub message: String,
}

impl AgentEngineError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for AgentEngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AgentEngineError {}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    Agent(ipc::AgentEvent),
    ContextUsage(ipc::ContextUsageEvent),
    Answer(ipc::AnswerEvent),
    Plan(ipc::PlanEvent),
    Progress(ipc::ProgressEvent),
    Error(ipc::ErrorEvent),
    Status(ipc::StatusResponse),
    Wiki(ipc::WikiResponse),
    /// A saved session finished loading (session restore): a signal only, so the
    /// panel can flip out of its connecting state. Carries nothing rendered raw
    /// (rules.md forbids raw kind identifiers as user-facing text).
    SessionLoaded(ipc::SessionLoadedEvent),
    AutonomousRun(Box<crate::autonomous::AutonomousRunState>),
    /// What a turn boundary recorded in the project's git history.
    Git(ipc::GitTurnEvent),
}

pub(crate) trait EventSink {
    fn emit(&self, event: EngineEvent) -> Result<(), AgentEngineError>;
}

/// Provides per-turn project memory rendering.
pub trait MemoryProvider: Send + Sync {
    /// Render the `[project memory]` prompt section for the current project state.
    fn render_section(&self) -> String;
}

/// Renders the `[project state]` section fresh each turn (project name + build
/// state from the editor), so a resumed thread never carries a stale snapshot.
pub trait ProjectStateProvider: Send + Sync {
    fn render_section(&self) -> String;
}

/// Renders the `[project map]` section (source listing, entrypoints, plugins,
/// DAT override counts, accepted spec index) fresh each turn; the context
/// cursor hashes it so it is re-sent only when it changes.
pub trait ProjectMapProvider: Send + Sync {
    fn render_section(&self) -> Option<String>;
}

/// Provides the dat-edit WIKI `[wiki facts]` prompt section and records accepted
/// dat edits to the per-project ledger.
pub trait WikiProvider: Send + Sync {
    /// Render the `[wiki facts]` prompt section for the current project, or `None`
    /// when the ledger is empty/disabled (the section is skipped). `query` is the
    /// current turn text, used to select the most relevant items when the ledger
    /// exceeds the token budget.
    fn render_section(&self, query: &str) -> Option<String>;

    /// Upsert the accepted dat edits into the project ledger, persist it, and return
    /// the updated ledger for emission. Returns `None` when nothing was recorded
    /// (no accepted dat edits, or the wiki is disabled / the write failed).
    fn record_accepted(&self, entries: Vec<crate::wiki::LedgerEntry>) -> Option<ipc::WikiResponse>;
}

#[derive(Clone)]
pub struct AgentEngineConfig {
    /// Whether each engine reads its project sections from its own project's
    /// handle (`for_project`); tests use fixed sections instead.
    project_scoped: bool,
    project_state: String,
    project_memory: Option<String>,
    rag_hits: Vec<crate::rag::Hit>,
    memory_provider: Option<Arc<dyn MemoryProvider>>,
    project_state_provider: Option<Arc<dyn ProjectStateProvider>>,
    project_map_provider: Option<Arc<dyn ProjectMapProvider>>,
    wiki_provider: Option<Arc<dyn WikiProvider>>,
}

impl AgentEngineConfig {
    pub fn new(
        project_state: impl Into<String>,
        project_memory: Option<String>,
        rag_hits: Vec<crate::rag::Hit>,
    ) -> Self {
        Self {
            project_scoped: false,
            project_state: project_state.into(),
            project_memory,
            rag_hits,
            memory_provider: None,
            project_state_provider: None,
            project_map_provider: None,
            wiki_provider: None,
        }
    }

    pub fn for_tests(
        project_state: impl Into<String>,
        project_memory: Option<String>,
        rag_hits: Vec<crate::rag::Hit>,
    ) -> Self {
        Self::new(project_state, project_memory, rag_hits)
    }

    /// Read every engine's project sections (memory, wiki, state, map) from
    /// the project the engine's session belongs to.
    pub fn with_project_providers(mut self) -> Self {
        self.project_scoped = true;
        self
    }

    /// This configuration for an engine of `handle`'s project.
    pub(crate) fn for_project(&self, handle: &Arc<crate::project_registry::ProjectHandle>) -> Self {
        if !self.project_scoped {
            return self.clone();
        }
        let providers = crate::project_registry::HandleProviders::new(handle);
        self.clone()
            .with_memory_provider(Arc::new(providers.clone()))
            .with_wiki_provider(Arc::new(providers.clone()))
            .with_project_state_provider(Arc::new(providers.clone()))
            .with_project_map_provider(Arc::new(providers))
    }

    pub fn with_memory_provider(mut self, provider: Arc<dyn MemoryProvider>) -> Self {
        self.memory_provider = Some(provider);
        self
    }

    pub fn with_project_state_provider(mut self, provider: Arc<dyn ProjectStateProvider>) -> Self {
        self.project_state_provider = Some(provider);
        self
    }

    pub fn with_wiki_provider(mut self, provider: Arc<dyn WikiProvider>) -> Self {
        self.wiki_provider = Some(provider);
        self
    }

    pub fn with_project_map_provider(mut self, provider: Arc<dyn ProjectMapProvider>) -> Self {
        self.project_map_provider = Some(provider);
        self
    }

    /// The `[project map]` section for the prompt, when a provider is wired.
    fn project_map_for_prompt(&self) -> Option<String> {
        self.project_map_provider
            .as_ref()
            .and_then(|provider| provider.render_section())
    }

    /// The `[wiki facts]` section for the prompt, when a provider is wired and the
    /// ledger is non-empty. `query` (the current turn text) drives query-aware item
    /// selection when the ledger exceeds the token budget.
    fn wiki_section_for_prompt(&self, query: &str) -> Option<String> {
        self.wiki_provider
            .as_ref()
            .and_then(|provider| provider.render_section(query))
    }

    /// The `[project state]` text for the prompt: a live render when a provider
    /// is wired, otherwise the construction-time constant (tests).
    fn project_state_for_prompt(&self) -> String {
        self.project_state_provider
            .as_ref()
            .map(|provider| provider.render_section())
            .unwrap_or_else(|| self.project_state.clone())
    }

    fn project_memory_for_prompt(&self) -> Option<String> {
        self.memory_provider
            .as_ref()
            .map(|provider| provider.render_section())
            .or_else(|| self.project_memory.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Triage,
    Answer,
    PlanReview,
    Executing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteContinuation {
    Direct,
    ApprovedPlan,
}

pub(crate) struct AgentEngine<R: RuntimeExecutor, S: EventSink> {
    executor: R,
    sink: S,
    config: AgentEngineConfig,
    phase: Phase,
    thread_active: bool,
    /// Whether the last foreground turn (a chat, plan feedback, an approved
    /// plan's execution, an autonomous resume) ended cancelled rather than
    /// answered. Every foreground run resets it and a cancelled run sets it,
    /// in `run_foreground`, so no path inherits a stale value.
    last_turn_cancelled: bool,
    /// The next Map request is a "사진 배치" request (set by `map_chat`).
    map_image_placement: bool,
    hydrated: bool,
    plan_revision: u32,
    current_plan_markdown: Option<String>,
    current_request_id: Option<String>,
    current_client_turn_id: Option<String>,
    current_user_text: String,
    last_answer: String,
    approved_plan_sha256: Option<String>,
    session_id: String,
    project_id: crate::project_id::ProjectId,
    session_kind: crate::session::SessionKind,
    provider_binding: crate::provider::ProviderBinding,
    pending_write: Option<WriteContinuation>,
    pending_resume_transcript: Option<String>,
    conversation_resume_error: Option<String>,
    pending_context_delivery: Option<crate::context_state::ModelContextCursor>,
    session_store: crate::session::SessionStore,
    attachment_store: AttachmentStore,
    journal_store: journal::JournalStore,
    journal_data_dir: PathBuf,
    runtime: SessionToolRuntime,
    cancellation: tokio::sync::watch::Receiver<u64>,
    execution_mode: crate::autonomous::ExecutionMode,
    autonomous_policy: crate::autonomous::AutonomousRunPolicy,
    autonomous_pause_requested: Arc<AtomicBool>,
    /// For a worker session: its parent session id and its title, which name
    /// the worker's commits.
    worker_origin: Option<(String, String)>,
    /// The commits the current chat turn made, for a worker's settlement.
    last_turn_commits: SyncMutex<Vec<crate::git::CommitRecord>>,
    /// The cancellation generation a chat turn belongs to, taken once when it
    /// enters (or, for a worker turn, before it waited for the engine): a
    /// cancel issued after that stops the turn at its next check, the first
    /// model step included, instead of being absorbed by a later read.
    turn_generation: Option<u64>,
    /// A generation the next chat turn must use instead of the current one.
    entry_generation: Option<u64>,
    /// Worker settlements this turn's `[workers]` note showed for the first
    /// time; they are marked reported only when the turn succeeds.
    shown_worker_reports: Vec<String>,
}
impl<R: RuntimeExecutor, S: EventSink> AgentEngine<R, S> {
    // Keep each injected runtime, persistence, session, and cancellation authority explicit.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        executor: R,
        sink: S,
        config: AgentEngineConfig,
        runtime: SessionToolRuntime,
        session_store: crate::session::SessionStore,
        attachment_store: AttachmentStore,
        session: crate::session::SessionRecord,
        cancellation: tokio::sync::watch::Receiver<u64>,
    ) -> Self {
        let journal_store = runtime.journal().clone();
        let journal_data_dir = runtime.app_data_dir();
        let provider_binding = session.provider_binding.clone();
        let autonomous_run = session.autonomous_run.clone();
        let worker_origin = session
            .meta
            .worker_parent
            .clone()
            .map(|parent| (parent, session.meta.name.clone()));
        let execution_mode = if autonomous_run.is_some() {
            crate::autonomous::ExecutionMode::Autonomous
        } else {
            crate::autonomous::ExecutionMode::Interactive
        };
        let autonomous_policy = autonomous_run
            .as_ref()
            .map(|run| run.policy.clone())
            .unwrap_or_default();
        let autonomous_pause_requested = runtime.autonomous_pause_handle();
        autonomous_pause_requested.store(
            autonomous_run
                .as_ref()
                .is_some_and(|run| run.status.can_resume()),
            Ordering::SeqCst,
        );
        Self {
            executor,
            sink,
            config,
            phase: Phase::Idle,
            thread_active: false,
            last_turn_cancelled: false,
            map_image_placement: false,
            hydrated: false,
            plan_revision: 0,
            current_plan_markdown: None,
            current_request_id: None,
            current_client_turn_id: None,
            current_user_text: String::new(),
            last_answer: String::new(),
            approved_plan_sha256: None,
            project_id: crate::project_id::ProjectId::from_stored(session.meta.project_key()),
            session_id: session.meta.id,
            session_kind: session.meta.kind,
            provider_binding,
            pending_write: None,
            pending_resume_transcript: None,
            conversation_resume_error: None,
            pending_context_delivery: None,
            session_store,
            attachment_store,
            journal_store,
            journal_data_dir,
            runtime,
            cancellation,
            execution_mode,
            autonomous_policy,
            autonomous_pause_requested,
            worker_origin,
            last_turn_commits: SyncMutex::new(Vec::new()),
            turn_generation: None,
            entry_generation: None,
            shown_worker_reports: Vec::new(),
        }
    }

    pub fn autonomous_pause_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.autonomous_pause_requested)
    }

    fn autonomous_run(&self) -> Result<crate::autonomous::AutonomousRunState, AgentEngineError> {
        self.session_store
            .load(&self.session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?
            .autonomous_run
            .ok_or_else(|| AgentEngineError::new("장시간 작업 상태가 없습니다."))
    }

    async fn persist_autonomous_run(
        &mut self,
        mut run: crate::autonomous::AutonomousRunState,
    ) -> Result<(), AgentEngineError> {
        let conversation = self.executor.conversation_state();
        run.last_checkpoint = Some(conversation.clone());
        run.updated_at = crate::session::now_unix_millis();
        self.session_store
            .set_autonomous_checkpoint(&self.session_id, conversation, run.clone())
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        self.executor
            .acknowledge_persisted()
            .await
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        self.sink.emit(EngineEvent::AutonomousRun(Box::new(run)))
    }

    fn update_autonomous_progress(
        &self,
        run: &mut crate::autonomous::AutonomousRunState,
        record_fingerprint: bool,
    ) -> Result<(), AgentEngineError> {
        let now = crate::session::now_unix_millis();
        if let Some(active_started_at) = run.active_started_at.replace(now) {
            run.progress.elapsed_active_millis = run
                .progress
                .elapsed_active_millis
                .saturating_add(now.saturating_sub(active_started_at));
        }
        let (read_actions, write_actions) = self.runtime.request_action_counts();
        run.progress.read_actions = read_actions;
        run.progress.write_actions = write_actions;
        run.progress.latest_build = self.runtime.latest_build_progress().map(Into::into);
        run.project_revision = self
            .runtime
            .current_project_revision()
            .map_err(AgentEngineError::new)?;
        let record = self
            .session_store
            .load(&self.session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        if let Some(observed) = record
            .context_usage
            .as_ref()
            .map(|usage| usage.total.total_tokens.max(0) as u64)
        {
            run.progress.observed_tokens = Some(
                run.progress
                    .observed_tokens
                    .unwrap_or_default()
                    .max(observed),
            );
        }
        if record_fingerprint {
            let journal_count = self
                .journal_store
                .changeset(&run.request_id)
                .map(|changeset| changeset.items.len())
                .unwrap_or_default();
            let build = run
                .progress
                .latest_build
                .as_ref()
                .map(|build| {
                    format!(
                        "{}:{}:{}:{}",
                        build.input_revision,
                        build.diagnostics_fingerprint,
                        build.error_count,
                        build.success
                    )
                })
                .unwrap_or_else(|| "none".to_string());
            run.record_progress_fingerprint(format!(
                "{}|{}|{}|{}",
                run.project_revision, build, journal_count, record.task_state.projection.revision
            ));
        }
        Ok(())
    }

    async fn checkpoint_autonomous_boundary(
        &mut self,
        reason: crate::provider_runtime::IterationBoundaryReason,
        blocker: Option<String>,
    ) -> Result<bool, AgentEngineError> {
        let mut run = self.autonomous_run()?;
        if run.status.is_terminal() {
            return Ok(false);
        }
        self.update_autonomous_progress(&mut run, true)?;
        run.boundary_reason = Some(reason);
        run.blocker = blocker;
        if self.autonomous_pause_requested.load(Ordering::SeqCst) {
            run.status = crate::autonomous::AutonomousRunStatus::Paused;
            run.pause_reason = Some(crate::autonomous::AutonomousPauseReason::User);
            run.active_started_at = None;
        } else if let Some(limit_reason) = run.run_limit_reason() {
            run.status = crate::autonomous::AutonomousRunStatus::SafetyStopped;
            run.pause_reason = None;
            run.blocker = Some(limit_reason);
            run.active_started_at = None;
        } else {
            run.status = crate::autonomous::AutonomousRunStatus::Running;
            run.pause_reason = None;
            run.iteration = run.iteration.saturating_add(1);
        }
        let should_continue = run.status == crate::autonomous::AutonomousRunStatus::Running;
        // One long run is many commits, so a run that went wrong halfway can be
        // reverted to the iteration that was still right.
        let _ = self
            .commit_boundary(
                &format!("[장시간 작업 {}회차] {}", run.iteration, run.goal),
                &run.request_id,
            )
            .await;
        self.persist_autonomous_run(run).await?;
        Ok(should_continue)
    }

    fn autonomous_completion_blocker(&self) -> Result<Option<String>, AgentEngineError> {
        let Some(request_id) = self.current_request_id.as_deref() else {
            return Err(AgentEngineError::new("foreground run has no request id"));
        };
        let has_runtime_changes = self
            .journal_store
            .changeset(request_id)
            .map(|changeset| !changeset.items.is_empty())
            .unwrap_or(false);
        if !has_runtime_changes {
            return Ok(None);
        }
        let revision = self
            .runtime
            .current_project_revision()
            .map_err(AgentEngineError::new)?;
        let built = self
            .runtime
            .latest_build_progress()
            .is_some_and(|build| build.success && build.input_revision == revision);
        Ok((!built).then(|| {
            "현재 runtime revision의 성공한 build_run이 없어 장시간 작업을 완료할 수 없습니다."
                .to_string()
        }))
    }

    fn autonomous_continuation_turn(
        &self,
        previous: &AgentTurnInput,
        reason: crate::provider_runtime::IterationBoundaryReason,
        blocker: Option<&str>,
    ) -> Result<AgentTurnInput, AgentEngineError> {
        let run = self.autonomous_run()?;
        let record = self
            .session_store
            .load(&self.session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let acceptance = record
            .task_state
            .projection
            .acceptance_criteria
            .iter()
            .map(|fact| format!("- {}", fact.text))
            .collect::<Vec<_>>();
        let latest_build = run
            .progress
            .latest_build
            .as_ref()
            .map(|build| {
                format!(
                    "revision={}, success={}, errors={}, fingerprint={}",
                    build.input_revision,
                    build.success,
                    build.error_count,
                    build.diagnostics_fingerprint
                )
            })
            .unwrap_or_else(|| "없음".to_string());
        Ok(AgentTurnInput {
            text: format!(
                "[autonomous continuation]\n원래 목표:\n{}\n\n수락 기준:\n{}\n\n현재 blocker:\n{}\n\n최근 build:\n{}\n\n경계 이유: {:?}\n\n확인된 체크포인트 이후의 미완료 작업만 계속하세요. 완료된 도구 호출을 재실행하거나 이전 대화 전체를 재진술하지 마세요.{}",
                run.goal,
                if acceptance.is_empty() {
                    "- 명시된 원래 목표와 검증 기준".to_string()
                } else {
                    acceptance.join("\n")
                },
                blocker.unwrap_or("없음"),
                latest_build,
                reason,
                Self::continuation_delegation_hint(reason),
            ),
            image_paths: Vec::new(),
            workspace_root: previous.workspace_root.clone(),
            workspace_temp: previous.workspace_temp.clone(),
            workspace_access: previous.workspace_access,
            native_tools_enabled: previous.native_tools_enabled,
            output_schema: None,
            forbid_tools: false,
        })
    }

    /// After a context-pressure boundary the next iteration is told to stop
    /// widening its search. A fixed control phrase; nothing is delegated.
    fn continuation_delegation_hint(
        reason: crate::provider_runtime::IterationBoundaryReason,
    ) -> &'static str {
        match reason {
            crate::provider_runtime::IterationBoundaryReason::ContextPressure => {
                " 컨텍스트 여유가 부족합니다: 넓은 탐색을 더 벌이지 말고 지금까지 확인한 것으로 남은 작업을 끝내세요. 편집 대상은 편집 직전에 직접 다시 읽어야 합니다."
            }
            _ => "",
        }
    }

    fn interactive_continuation_turn(
        previous: &AgentTurnInput,
        reason: crate::provider_runtime::IterationBoundaryReason,
    ) -> AgentTurnInput {
        let hint = Self::continuation_delegation_hint(reason);
        AgentTurnInput {
            text: format!(
                "[continuation]
경계 이유: {reason:?}

확인된 체크포인트 이후의 미완료 작업만 계속하세요. 완료된 도구 호출을 재실행하거나 이전 대화 전체를 재진술하지 마세요.{hint}"
            ),
            image_paths: Vec::new(),
            workspace_root: previous.workspace_root.clone(),
            workspace_temp: previous.workspace_temp.clone(),
            workspace_access: previous.workspace_access,
            native_tools_enabled: previous.native_tools_enabled,
            output_schema: None,
            forbid_tools: false,
        }
    }

    async fn finish_autonomous_run(
        &mut self,
        status: crate::autonomous::AutonomousRunStatus,
        blocker: Option<String>,
    ) -> Result<(), AgentEngineError> {
        let mut run = self.autonomous_run()?;
        if run.status.is_terminal() {
            return Ok(());
        }
        self.update_autonomous_progress(&mut run, false)?;
        run.status = status;
        run.blocker = blocker;
        run.pause_reason = match status {
            crate::autonomous::AutonomousRunStatus::Review => {
                Some(crate::autonomous::AutonomousPauseReason::Review)
            }
            crate::autonomous::AutonomousRunStatus::WaitingInput => {
                Some(crate::autonomous::AutonomousPauseReason::WaitingInput)
            }
            _ => None,
        };
        run.active_started_at = None;
        self.persist_autonomous_run(run).await
    }

    async fn pause_autonomous_for_team_apply(&mut self) -> Result<(), AgentEngineError> {
        let mut run = self.autonomous_run()?;
        if run.status.is_terminal() {
            return Ok(());
        }
        self.update_autonomous_progress(&mut run, false)?;
        run.status = crate::autonomous::AutonomousRunStatus::Paused;
        run.pause_reason = Some(crate::autonomous::AutonomousPauseReason::TeamApply);
        run.blocker = Some(
            "팀 맵 작업이 아직 끝나지 않았습니다. 후보가 준비되면 자동으로 이어서 진행하며, 후보가 이미 준비된 경우 맵 창에서 적용·폐기하거나 계속을 누르면 AI가 후보를 검토합니다."
                .to_string(),
        );
        run.active_started_at = None;
        self.persist_autonomous_run(run).await
    }

    async fn pause_autonomous_for_unanswered_ask(&mut self) -> Result<(), AgentEngineError> {
        let mut run = self.autonomous_run()?;
        if run.status.is_terminal() {
            return Ok(());
        }
        self.update_autonomous_progress(&mut run, false)?;
        run.status = crate::autonomous::AutonomousRunStatus::Paused;
        run.pause_reason = Some(crate::autonomous::AutonomousPauseReason::UnansweredAsk);
        run.blocker = Some(
            "질문에 대한 답을 기다리는 시간이 지나 텍스트로 질문했습니다. 계속을 누르면 AI가 같은 질문을 다시 합니다."
                .to_string(),
        );
        run.active_started_at = None;
        self.persist_autonomous_run(run).await
    }

    async fn settle_autonomous_review(&mut self) -> Result<(), AgentEngineError> {
        if self.execution_mode != crate::autonomous::ExecutionMode::Autonomous {
            return Ok(());
        }
        let mut run = self.autonomous_run()?;
        self.update_autonomous_progress(&mut run, false)?;
        match run.status {
            crate::autonomous::AutonomousRunStatus::Review => {
                run.status = crate::autonomous::AutonomousRunStatus::Completed;
                run.pause_reason = None;
                run.blocker = None;
                run.active_started_at = None;
            }
            crate::autonomous::AutonomousRunStatus::Paused => {
                run.pause_reason
                    .get_or_insert(crate::autonomous::AutonomousPauseReason::User);
                run.active_started_at = None;
            }
            _ => return Ok(()),
        }
        self.persist_autonomous_run(run).await
    }

    fn boundary_context_pressure(&self) -> Result<bool, AgentEngineError> {
        if self.autonomous_pause_requested.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let record = self
            .session_store
            .load(&self.session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        Ok(record.context_usage.is_some_and(|usage| {
            usage
                .model_context_window
                .filter(|window| *window > 0)
                .is_some_and(|window| {
                    usage.last.total_tokens.saturating_mul(100) >= window.saturating_mul(85)
                })
        }))
    }

    /// Compact at a confirmed iteration boundary when the provider context is under pressure.
    /// The boundary already checkpointed a started provider conversation, so the thread counts
    /// as active even inside the first turn of a session.
    async fn compact_at_boundary(&mut self) -> Result<bool, AgentEngineError> {
        if !self.boundary_context_pressure()? {
            return Ok(false);
        }
        let previous_phase = self.phase;
        self.phase = Phase::Idle;
        self.thread_active = true;
        let result = self.compact().await;
        self.phase = previous_phase;
        result?;
        Ok(true)
    }

    fn run_identity(&self, request_id: &str) -> RunIdentity {
        RunIdentity {
            session_id: self.session_id.clone(),
            run_id: RunId::new(next_run_id()),
            request_id: request_id.to_string(),
            session_kind: self.session_kind,
            cancellation_generation: self
                .turn_generation
                .unwrap_or_else(|| *self.cancellation.borrow()),
        }
    }

    fn binding_snapshot(&self, fresh: bool) -> Result<BindingSnapshot, AgentEngineError> {
        let mut snapshot = BindingSnapshot::from_binding(&self.provider_binding, None)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        if fresh {
            snapshot.conversation =
                crate::provider::ProviderConversationState::empty(self.provider_binding.provider);
        }
        Ok(snapshot)
    }

    fn ensure_provider_conversation_ready(&self) -> Result<(), AgentEngineError> {
        match self.conversation_resume_error.as_deref() {
            Some(error) => Err(AgentEngineError::new(error)),
            None => Ok(()),
        }
    }

    /// Why the persisted provider conversation could not be resumed at hydration,
    /// while the session itself stays open for an explicit reset.
    pub(crate) fn conversation_resume_error(&self) -> Option<&str> {
        self.conversation_resume_error.as_deref()
    }

    /// Drop the persisted provider conversation (and any unresolved native receipt)
    /// so the next turn starts a fresh native session. The panel log, candidate
    /// state, and task state are untouched; only the model-side thread restarts.
    pub async fn reset_conversation(&mut self) -> Result<(), AgentEngineError> {
        if matches!(self.phase, Phase::PlanReview | Phase::Executing) {
            return Err(AgentEngineError::new(
                "현재 세션의 진행 중인 요청 또는 검토를 먼저 완료해 주세요.",
            ));
        }
        self.executor
            .reset()
            .await
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        self.thread_active = false;
        self.conversation_resume_error = None;
        let record = self
            .session_store
            .load(&self.session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let transcript = condense_transcript(&record.panel_log);
        self.pending_resume_transcript = (!transcript.trim().is_empty()).then_some(transcript);
        self.session_store
            .reset_context_epoch(&self.session_id, &self.session_prompt_baseline(), true)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        Ok(())
    }

    fn foreground_request(
        &self,
        request_id: &str,
        turn: AgentTurnInput,
    ) -> Result<ForegroundRequest, AgentEngineError> {
        let record = self
            .session_store
            .load(&self.session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        // Every foreground run starts without an inherited unanswered-ask mark.
        self.runtime.clear_expired_ask();
        Ok(ForegroundRequest {
            identity: self.run_identity(request_id),
            binding: self.binding_snapshot(false)?,
            turn,
            checkpoint: JobBase {
                revision: record.task_state.projection.revision,
                instruction_epoch: record.context_state.instruction_epoch,
                branch: record.task_state.leaf_id,
            },
            policy: RunPolicy {
                active_deadline: None,
                shutdown_grace: std::time::Duration::from_secs(2),
                max_output_bytes: 32 * 1024 * 1024,
                max_output_tokens: None,
                max_tool_rounds: DEFAULT_MAX_TOOL_ROUNDS,
                allow_resume: true,
            },
        })
    }

    async fn run_foreground(
        &mut self,
        turn: AgentTurnInput,
    ) -> Result<AgentTurnResult, AgentEngineError> {
        let request_id = self
            .current_request_id
            .clone()
            .ok_or_else(|| AgentEngineError::new("foreground run has no request id"))?;
        self.last_turn_cancelled = false;
        let mut turn = turn;
        loop {
            let request = self.foreground_request(&request_id, turn.clone())?;
            match self.executor.run_foreground(request).await {
                RunOutcome::Completed { text, .. } => {
                    if self.execution_mode == crate::autonomous::ExecutionMode::Autonomous {
                        if self.runtime.ask_expired_for_request(&request_id) {
                            // The model restated an unanswered ask as text; the
                            // reply arrives as an ordinary message, so the run
                            // pauses instead of completing or iterating.
                            self.pause_autonomous_for_unanswered_ask().await?;
                            return Ok(AgentTurnResult::Answer { text });
                        }
                        if team::has_active_task(&self.session_store, &self.session_id) {
                            // The Map Agent is still working, or the model
                            // left a ready candidate undecided: pause; the
                            // team dispatcher resumes the run when the task
                            // settles, otherwise the user does.
                            self.pause_autonomous_for_team_apply().await?;
                            return Ok(AgentTurnResult::Answer { text });
                        }
                        if let Some(blocker) = self.autonomous_completion_blocker()? {
                            let reason = if self.compact_at_boundary().await? {
                                crate::provider_runtime::IterationBoundaryReason::ContextPressure
                            } else {
                                crate::provider_runtime::IterationBoundaryReason::ProviderContinuation
                            };
                            if self
                                .checkpoint_autonomous_boundary(reason, Some(blocker.clone()))
                                .await?
                            {
                                self.runtime
                                    .begin_iteration(&request_id)
                                    .map_err(AgentEngineError::new)?;
                                turn = self.autonomous_continuation_turn(
                                    &turn,
                                    reason,
                                    Some(&blocker),
                                )?;
                                continue;
                            }
                            return Ok(AgentTurnResult::IterationBoundary { reason });
                        }
                        let status = if self
                            .journal_store
                            .changeset(&request_id)
                            .map(|changeset| !changeset.items.is_empty())
                            .unwrap_or(false)
                        {
                            crate::autonomous::AutonomousRunStatus::Review
                        } else {
                            crate::autonomous::AutonomousRunStatus::Completed
                        };
                        self.finish_autonomous_run(status, None).await?;
                    }
                    return Ok(AgentTurnResult::Answer { text });
                }
                RunOutcome::Cancelled => {
                    if self.execution_mode == crate::autonomous::ExecutionMode::Autonomous {
                        self.finish_autonomous_run(
                            crate::autonomous::AutonomousRunStatus::Cancelled,
                            Some("사용자가 장시간 작업을 중단했습니다.".to_string()),
                        )
                        .await?;
                    }
                    self.last_turn_cancelled = true;
                    return Ok(AgentTurnResult::Cancelled);
                }
                RunOutcome::WriteTransition => {
                    if self.execution_mode == crate::autonomous::ExecutionMode::Autonomous {
                        let mut run = self.autonomous_run()?;
                        self.update_autonomous_progress(&mut run, false)?;
                        run.status = crate::autonomous::AutonomousRunStatus::Running;
                        run.boundary_reason = None;
                        self.persist_autonomous_run(run).await?;
                    }
                    return Ok(AgentTurnResult::WriteTransition);
                }
                RunOutcome::IterationBoundary { reason, .. } => {
                    let reason = if self.compact_at_boundary().await? {
                        crate::provider_runtime::IterationBoundaryReason::ContextPressure
                    } else {
                        reason
                    };
                    if self.execution_mode != crate::autonomous::ExecutionMode::Autonomous {
                        // Ordinary turns never stop at a soft action/round boundary: persist the
                        // confirmed checkpoint and continue the same request in place.
                        self.update_active_session().await;
                        self.runtime
                            .begin_iteration(&request_id)
                            .map_err(AgentEngineError::new)?;
                        turn = Self::interactive_continuation_turn(&turn, reason);
                        continue;
                    }
                    if !self.checkpoint_autonomous_boundary(reason, None).await? {
                        return Ok(AgentTurnResult::IterationBoundary { reason });
                    }
                    self.runtime
                        .begin_iteration(&request_id)
                        .map_err(AgentEngineError::new)?;
                    turn = self.autonomous_continuation_turn(&turn, reason, None)?;
                }
                RunOutcome::Structured { .. } => {
                    let detail =
                        "foreground provider run returned a structured job result".to_string();
                    if self.execution_mode == crate::autonomous::ExecutionMode::Autonomous {
                        self.finish_autonomous_run(
                            crate::autonomous::AutonomousRunStatus::Failed,
                            Some(detail.clone()),
                        )
                        .await?;
                    }
                    return Err(AgentEngineError::new(detail));
                }
                RunOutcome::Failed(error) => {
                    let detail = error.to_string();
                    if self.execution_mode == crate::autonomous::ExecutionMode::Autonomous {
                        self.finish_autonomous_run(
                            crate::autonomous::AutonomousRunStatus::Failed,
                            Some(detail.clone()),
                        )
                        .await?;
                    }
                    return Err(AgentEngineError::new(detail));
                }
            }
        }
    }

    async fn commit_context_delivery(&mut self, result: &AgentTurnResult) {
        let Some(mut cursor) = self.pending_context_delivery.take() else {
            return;
        };
        if matches!(result, AgentTurnResult::Cancelled) {
            return;
        }
        let conversation = self.executor.conversation_state();
        cursor.provider = conversation.provider();
        cursor.conversation_key = conversation.conversation_key();
        if let Err(error) =
            self.session_store
                .commit_context_delivery(&self.session_id, cursor.epoch, cursor)
        {
            eprintln!("eud-agent: context delivery commit failed: {error}");
        }
    }

    fn set_client_turn_id(&mut self, client_turn_id: &str) -> Result<(), AgentEngineError> {
        uuid::Uuid::parse_str(client_turn_id)
            .map_err(|_| AgentEngineError::new("clientTurnId must be a UUID"))?;
        self.current_client_turn_id = Some(client_turn_id.to_string());
        Ok(())
    }

    /// The interview toggle reaches only a session that can ask the user: an
    /// EPS session that is not a background worker.
    fn clarify_applies(&self, requested: bool) -> bool {
        requested
            && self.session_kind == crate::session::SessionKind::Eps
            && self.worker_origin.is_none()
    }

    pub async fn chat(&mut self, req: ipc::ChatRequest) -> Result<(), AgentEngineError> {
        if self.session_kind != crate::session::SessionKind::Eps {
            return Err(AgentEngineError::new(
                "Map sessions accept conversation only through map_agent_chat.",
            ));
        }
        self.chat_with_request_id(req, None).await
    }

    /// [`Self::chat`] for a turn that belongs to cancellation `generation`,
    /// read before the caller waited for this engine: a cancel issued while
    /// the turn was still waiting stops it before its first model step.
    pub(crate) async fn chat_from_generation(
        &mut self,
        req: ipc::ChatRequest,
        generation: u64,
    ) -> Result<(), AgentEngineError> {
        self.entry_generation = Some(generation);
        let result = self.chat(req).await;
        self.entry_generation = None;
        result
    }

    pub async fn map_chat(
        &mut self,
        request_id: String,
        text: String,
        attachments: Vec<String>,
        image_placement: bool,
    ) -> Result<MapTurnEnd, AgentEngineError> {
        if self.session_kind != crate::session::SessionKind::Map {
            return Err(AgentEngineError::new(
                "the requested session is not a Map session",
            ));
        }
        self.map_image_placement = image_placement;
        let request_id_for_check = request_id.clone();
        let result = self
            .chat_with_request_id(
                ipc::ChatRequest {
                    text,
                    client_turn_id: crate::ipc::new_client_turn_id(),
                    attachments,
                    mentions: Vec::new(),
                    execution_mode: crate::autonomous::ExecutionMode::Interactive,
                    autonomous_policy: None,
                    clarify: false,
                    skill_selections: Default::default(),
                },
                Some(request_id),
            )
            .await;
        self.map_image_placement = false;
        result?;
        let unplaced = self.runtime.unplaced_required_images(&request_id_for_check);
        if !unplaced.is_empty() && !self.last_turn_cancelled {
            return Err(AgentEngineError::new(format!(
                "사진 배치를 끝내지 못했습니다: {}을(를) 배치하지 않고 요청이 끝났습니다. 다시 요청하거나 배치할 위치를 알려 주세요.",
                unplaced.join(", ")
            )));
        }
        Ok(if self.last_turn_cancelled {
            MapTurnEnd::Cancelled
        } else {
            MapTurnEnd::Completed
        })
    }

    pub async fn autonomous_resume(&mut self) -> Result<(), AgentEngineError> {
        self.ensure_provider_conversation_ready()?;
        let record = self
            .session_store
            .load(&self.session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let mut run = record
            .autonomous_run
            .clone()
            .ok_or_else(|| AgentEngineError::new("계속할 장시간 작업이 없습니다."))?;
        if !run.status.can_resume() {
            return Err(AgentEngineError::new(
                "현재 장시간 작업 상태에서는 계속할 수 없습니다.",
            ));
        }
        if !record.pending_request_ids.is_empty() {
            return Err(AgentEngineError::new(
                "변경사항 검토를 먼저 완료한 뒤 장시간 작업을 계속하세요.",
            ));
        }
        let current_revision = self
            .runtime
            .current_project_revision()
            .map_err(AgentEngineError::new)?;
        if run.project_id != self.project_id.as_str() && self.accepts_legacy_run_project(&run) {
            // A checkpoint written before project ids named the project; it
            // is rewritten to the id and persisted with the resumed status.
            run.project_id = self.project_id.as_str().to_string();
        }
        if run.project_id != self.project_id.as_str() || run.project_revision != current_revision {
            run.status = crate::autonomous::AutonomousRunStatus::SafetyStopped;
            run.blocker = Some(
                "프로젝트 identity 또는 revision이 체크포인트 이후 변경되었습니다.".to_string(),
            );
            run.active_started_at = None;
            self.persist_autonomous_run(run).await?;
            return Err(AgentEngineError::new(
                "프로젝트가 체크포인트 이후 변경되어 장시간 작업을 계속하지 않았습니다.",
            ));
        }
        if run.last_checkpoint.as_ref() != Some(&record.provider_binding.conversation)
            || self.executor.conversation_state() != record.provider_binding.conversation
        {
            run.status = crate::autonomous::AutonomousRunStatus::SafetyStopped;
            run.blocker = Some("확인된 공급자 체크포인트가 현재 세션과 다릅니다.".to_string());
            run.active_started_at = None;
            self.persist_autonomous_run(run).await?;
            return Err(AgentEngineError::new(
                "확인된 공급자 체크포인트가 달라 장시간 작업을 계속하지 않았습니다.",
            ));
        }
        if self.current_request_id.as_deref() != Some(run.request_id.as_str()) {
            self.runtime
                .begin_request(&run.request_id, self.project_id.as_str())
                .map_err(AgentEngineError::new)?;
            self.runtime
                .restore_autonomous_progress(&run.request_id, &run.progress)
                .map_err(AgentEngineError::new)?;
            self.current_request_id = Some(run.request_id.clone());
            self.current_client_turn_id = Some(run.client_turn_id.clone());
            self.current_user_text = run.goal.clone();
        }
        self.execution_mode = crate::autonomous::ExecutionMode::Autonomous;
        self.autonomous_policy = run.policy.clone();
        self.autonomous_pause_requested
            .store(false, Ordering::SeqCst);
        self.runtime
            .begin_iteration(&run.request_id)
            .map_err(AgentEngineError::new)?;
        let reason = run
            .boundary_reason
            .unwrap_or(crate::provider_runtime::IterationBoundaryReason::ProviderContinuation);
        // The pause blocker (for example an unanswered ask) is the one thing the
        // resumed run must know about; it is consumed here, not persisted.
        let blocker = run.blocker.take();
        run.status = crate::autonomous::AutonomousRunStatus::Running;
        run.pause_reason = None;
        run.active_started_at = Some(crate::session::now_unix_millis());
        self.persist_autonomous_run(run).await?;
        self.phase = Phase::Triage;
        let base = AgentTurnInput::text(String::new()).with_access(WorkspaceAccess::Read);
        let turn = self.autonomous_continuation_turn(&base, reason, blocker.as_deref())?;
        let result = self.run_foreground(turn).await?;
        self.thread_active = true;
        if let Some(ticket) = self.runtime.write_ticket() {
            self.pending_write = Some(WriteContinuation::Direct);
            self.phase = match ticket.state() {
                crate::write_coordinator::TicketState::Granted => Phase::Executing,
                crate::write_coordinator::TicketState::Cancelled => Phase::Idle,
            };
            self.update_active_session().await;
            return Ok(());
        }
        if matches!(result, AgentTurnResult::WriteTransition) {
            return Err(AgentEngineError::new(
                "provider requested a write transition without a write ticket",
            ));
        }
        let state_result = result.clone();
        self.handle_turn_result(result)?;
        let goal = self.current_user_text.clone();
        self.update_task_state_after_turn(&state_result, &goal, None)
            .await;
        self.update_active_session().await;
        Ok(())
    }
    /// A request never leaves the provider conversation behind: whichever way
    /// the turn ends, the confirmed native session (or direct transcript
    /// revision) the runtime holds is persisted before the result is returned.
    /// Without this, a failed or cancelled turn kept its boundary only in
    /// memory and the next launch had nothing to continue from.
    async fn chat_with_request_id(
        &mut self,
        req: ipc::ChatRequest,
        fixed_request_id: Option<String>,
    ) -> Result<(), AgentEngineError> {
        self.last_turn_cancelled = false;
        self.last_turn_commits.lock().clear();
        self.shown_worker_reports.clear();
        self.turn_generation = Some(
            self.entry_generation
                .take()
                .unwrap_or_else(|| *self.cancellation.borrow()),
        );
        let request_before = self.current_request_id.clone();
        let result = self.chat_turn(req, fixed_request_id).await;
        self.turn_generation = None;
        // A worker turn that failed after it began still owns what it wrote:
        // commit it as the worker's, before its claims end, so the edits are
        // revertible with the worker instead of landing in the next turn's
        // external-edit commit.
        if result.is_err() && self.worker_origin.is_some() {
            if let Some(request_id) = self
                .current_request_id
                .clone()
                .filter(|request_id| request_before.as_ref() != Some(request_id))
            {
                let _ = self.commit_boundary("", &request_id).await;
            }
        }
        self.end_turn_claims();
        let shown = std::mem::take(&mut self.shown_worker_reports);
        if result.is_ok() && !self.last_turn_cancelled {
            if let Err(error) = self
                .session_store
                .mark_workers_reported(&self.session_id, &shown)
            {
                eprintln!("eud-agent: worker settlements could not be marked reported: {error}");
            }
        }
        if result.is_err() {
            self.update_active_session().await;
        }
        result
    }

    /// End the path claims of the request a turn just left, whichever way it
    /// ended. A request that still holds its write registration (plan review,
    /// a pending write) keeps them until `release`, which ends them too.
    fn end_turn_claims(&self) {
        if self.runtime.write_ticket().is_some() {
            return;
        }
        if let Some(request_id) = self.runtime.current_request_id() {
            self.runtime.end_request_claims(&request_id);
        }
    }

    async fn chat_turn(
        &mut self,
        req: ipc::ChatRequest,
        fixed_request_id: Option<String>,
    ) -> Result<(), AgentEngineError> {
        self.ensure_provider_conversation_ready()?;

        let execution_mode = req.execution_mode;
        let autonomous_policy = req.autonomous_policy.clone().unwrap_or_default();
        if execution_mode == crate::autonomous::ExecutionMode::Autonomous {
            if self.session_kind != crate::session::SessionKind::Eps {
                return Err(AgentEngineError::new(
                    "장시간 작업은 현재 메인 EPS 세션에서만 실행할 수 있습니다.",
                ));
            }
            autonomous_policy
                .validate()
                .map_err(AgentEngineError::new)?;
            let workers_active = self
                .session_store
                .load(&self.session_id)
                .map(|record| {
                    record
                        .workers
                        .iter()
                        .any(|worker| worker.status.is_active())
                })
                .unwrap_or(false);
            if workers_active {
                return Err(AgentEngineError::new(
                    "작업자가 실행 중인 동안에는 장시간 작업을 시작할 수 없습니다. 작업자가 끝나거나 작업자를 중단한 뒤 다시 시도해 주세요.",
                ));
            }
        }
        if matches!(self.phase, Phase::PlanReview | Phase::Executing) {
            return Err(AgentEngineError::new(
                "현재 세션의 진행 중인 요청 또는 검토를 먼저 완료해 주세요.",
            ));
        }
        if let Some(unknown) = req
            .skill_selections
            .keys()
            .find(|skill| crate::skills::find(skill).is_none())
        {
            return Err(AgentEngineError::new(format!(
                "알 수 없는 스킬입니다: {unknown}"
            )));
        }
        let resolved_mentions = self.resolve_mentions(&req.mentions)?;
        self.set_client_turn_id(&req.client_turn_id)?;
        let request_id = fixed_request_id.unwrap_or_else(next_request_id);
        self.runtime
            .begin_request(&request_id, self.project_id.as_str())
            .map_err(AgentEngineError::new)?;
        self.runtime.set_client_turn_id(&req.client_turn_id);
        if self.session_kind == crate::session::SessionKind::Eps && self.worker_origin.is_none() {
            if !req.skill_selections.is_empty() {
                self.session_store
                    .set_task_skill_selections(
                        &self.session_id,
                        &req.client_turn_id,
                        req.skill_selections
                            .iter()
                            .map(|(skill, enabled)| (skill.as_str(), *enabled)),
                    )
                    .map_err(|error| AgentEngineError::new(error.to_string()))?;
            }
        }
        self.commit_external_edits_before_turn().await;
        self.execution_mode = execution_mode;
        self.autonomous_policy = autonomous_policy.clone();
        self.autonomous_pause_requested
            .store(false, Ordering::SeqCst);
        self.current_plan_markdown = None;
        self.approved_plan_sha256 = None;
        self.plan_revision = 0;
        self.current_request_id = Some(request_id.clone());
        self.current_user_text = req.text.clone();
        self.last_answer.clear();
        self.phase = Phase::Triage;
        let mut attachment_context = self.resolve_attachments(&req.attachments)?;
        let audio_files = std::mem::take(&mut attachment_context.audio_files);
        if self.session_kind == crate::session::SessionKind::Map && !audio_files.is_empty() {
            return Err(AgentEngineError::new(
                "오디오 첨부는 메인 EPS 대화에서만 사용할 수 있습니다.",
            ));
        }
        let audio_refs =
            Self::bind_audio_attachments(self.runtime.clone(), &request_id, audio_files).await?;
        let map_image_refs = if self.session_kind == crate::session::SessionKind::Map {
            self.runtime
                .bind_map_images(&request_id, &attachment_context.images)
                .map_err(AgentEngineError::new)?
        } else {
            Vec::new()
        };
        let plain_user_text = if req.text.trim().is_empty() && !req.attachments.is_empty() {
            "첨부한 파일을 분석해 주세요."
        } else if req.text.trim().is_empty() && !req.mentions.is_empty() {
            "참조한 리소스를 바탕으로 요청을 수행해 주세요."
        } else {
            req.text.as_str()
        };
        let mut user_text = attachment_context.append_text_files(plain_user_text);
        if std::mem::take(&mut self.map_image_placement) {
            if map_image_refs.is_empty() {
                return Err(AgentEngineError::new(
                    "사진 배치에는 이미지 첨부가 필요합니다. 사진을 첨부한 뒤 다시 보내 주세요.",
                ));
            }
            self.runtime
                .require_image_placement(&request_id)
                .map_err(AgentEngineError::new)?;
            user_text.push_str(
                "\n\n[image placement requested]\nThe user turned on 사진 배치 for this message: \
                 place every image under [map image refs] with map_image_place. \
                 map_candidate_finalize is refused until each one is on the draft.",
            );
        }
        if !map_image_refs.is_empty() {
            user_text.push_str("\n\n[map image refs]\n");
            user_text.push_str(&serde_json::to_string(&map_image_refs).map_err(|error| {
                AgentEngineError::new(format!(
                    "map image references could not be serialized: {error}"
                ))
            })?);
        }
        if !audio_refs.is_empty() {
            user_text.push_str("\n\n[audio attachments]\n");
            user_text.push_str(&serde_json::to_string(&audio_refs).map_err(|error| {
                AgentEngineError::new(format!(
                    "trusted audio references could not be serialized: {error}"
                ))
            })?);
        }
        if execution_mode == crate::autonomous::ExecutionMode::Autonomous {
            let revision = self
                .runtime
                .current_project_revision()
                .map_err(AgentEngineError::new)?;
            let run = crate::autonomous::AutonomousRunState::new(
                user_text.clone(),
                req.client_turn_id.clone(),
                request_id.clone(),
                self.project_id.as_str().to_string(),
                revision,
                autonomous_policy,
                crate::session::now_unix_millis(),
            );
            self.persist_autonomous_run(run).await?;
        }

        let mut turn_text = if self.session_kind == crate::session::SessionKind::Map {
            let memory = self.config.project_memory_for_prompt();
            let project_state = self.config.project_state_for_prompt();
            if self.thread_active {
                format!(
                    "[map agent continuation]\n{}\n{}\n\n{}",
                    project_state,
                    memory.as_deref().unwrap_or("[project memory]\n(none)"),
                    user_text
                )
            } else {
                format!(
                    "{}\n\n{}",
                    build_map_system_prompt(&project_state, memory.as_deref()),
                    user_text
                )
            }
        } else if !self.thread_active && self.pending_resume_transcript.is_some() {
            String::new()
        } else {
            self.prepare_eps_context(&user_text, resolved_mentions.as_deref(), None, false)
                .await?
        };
        if self.session_kind == crate::session::SessionKind::Eps {
            if let Some(note) = team::prompt_note(&self.session_store, &self.session_id) {
                turn_text.push_str("\n\n");
                turn_text.push_str(&note);
            }
            // A worker session keeps its rules on every turn, including what
            // the user later types into its pane; the user text stays theirs.
            if self.worker_origin.is_some() {
                turn_text.push_str("\n\n");
                turn_text.push_str(self.runtime.worker_profile().prompt());
            } else if execution_mode == crate::autonomous::ExecutionMode::Interactive {
                turn_text.push_str("\n\n");
                turn_text.push_str(crate::worker::profile::PARENT_PROMPT);
            }
        }
        let clarify_requested = execution_mode == crate::autonomous::ExecutionMode::Interactive
            && self.clarify_applies(req.clarify);
        if clarify_requested {
            turn_text.push_str("\n\n");
            turn_text.push_str(CLARIFY_REQUESTED_NOTE);
        }
        if execution_mode == crate::autonomous::ExecutionMode::Autonomous {
            turn_text.push_str(
                "\n\n[autonomous execution]\n이 요청은 사용자가 명시적으로 장시간 작업으로 시작했습니다. iteration 경계에서는 확인된 상태에서 계속하고, 완료된 도구 호출은 반복하지 마세요. canonical runtime 변경 후에는 현재 revision의 build_run 성공을 확인한 뒤에만 완료하세요. ASK 또는 변경사항 검토는 사용자의 결정을 기다리세요.",
            );
        }

        // An EPS chat turn writes from its first call: a read turn that only
        // switched on its first mutation cost a refused call, a restarted turn,
        // and a re-read of every target.
        let workspace_access = if self.session_kind == crate::session::SessionKind::Eps
            && self.runtime.worker_profile().can_write()
        {
            self.runtime
                .register_write_request("interactive chat turn")
                .map_err(AgentEngineError::new)?;
            WorkspaceAccess::Write
        } else {
            WorkspaceAccess::Read
        };
        let result = self
            .run_first_turn_with_resume_fallback(
                AgentTurnInput {
                    text: turn_text,
                    image_paths: attachment_context.image_paths,
                    workspace_root: None,
                    workspace_temp: None,
                    workspace_access,
                    native_tools_enabled: self.runtime.worker_profile().can_write(),
                    output_schema: None,
                    forbid_tools: false,
                },
                &user_text,
                &req.mentions,
                clarify_requested,
            )
            .await?;
        // The turn's own runs are over; bookkeeping after them (the task-state
        // compiler) runs under the current generation as before.
        self.turn_generation = None;
        if self.session_kind == crate::session::SessionKind::Eps {
            self.commit_context_delivery(&result).await;
        }
        self.last_turn_cancelled = matches!(&result, AgentTurnResult::Cancelled);
        self.thread_active = if self.last_turn_cancelled {
            self.executor.conversation_state().is_started()
        } else {
            true
        };
        let result = self.reinterpret_plan(result);
        if workspace_access == WorkspaceAccess::Write {
            if matches!(result, AgentTurnResult::WriteTransition) {
                return Err(AgentEngineError::new(
                    "a write turn requested a write transition",
                ));
            }
            let is_plan = matches!(result, AgentTurnResult::Plan { .. });
            let state_result = result.clone();
            self.handle_turn_result(result)?;
            self.update_task_state_after_turn(
                &state_result,
                &user_text,
                resolved_mentions.as_deref(),
            )
            .await;
            if is_plan {
                // Plan review waits on the user. A turn that changed nothing
                // gives its registration back until approval registers again;
                // one that did keep it, and approval continues the same request.
                if self.runtime.abort_unmutated_write_intent().is_err() {
                    let _ = self.commit_boundary(&user_text, &request_id).await;
                }
            } else {
                self.commit_boundary(&user_text, &request_id).await;
                self.settle_request().await?;
                self.settle_write_lifecycle()?;
            }
            self.update_active_session().await;
            return Ok(());
        }
        if let Some(ticket) = self.runtime.write_ticket() {
            self.pending_write = Some(WriteContinuation::Direct);
            self.phase = match ticket.state() {
                crate::write_coordinator::TicketState::Granted => Phase::Executing,
                crate::write_coordinator::TicketState::Cancelled => Phase::Idle,
            };
            self.update_active_session().await;
            return Ok(());
        }
        if matches!(result, AgentTurnResult::WriteTransition) {
            return Err(AgentEngineError::new(
                "provider requested a write transition without a write ticket",
            ));
        }
        let state_result = result.clone();
        self.handle_turn_result(result)?;
        if self.session_kind == crate::session::SessionKind::Eps {
            self.update_task_state_after_turn(
                &state_result,
                &user_text,
                resolved_mentions.as_deref(),
            )
            .await;
        }
        if self.runtime.worker_profile().can_write() {
            let _ = self.commit_boundary(&user_text, &request_id).await;
        }
        self.update_active_session().await;
        Ok(())
    }

    async fn run_first_turn_with_resume_fallback(
        &mut self,
        input: AgentTurnInput,
        user_text: &str,
        mention_instances: &[crate::mentions::MentionInstance],
        clarify_requested: bool,
    ) -> Result<AgentTurnResult, AgentEngineError> {
        let Some(transcript) = self.pending_resume_transcript.take() else {
            return self.run_foreground(input).await;
        };
        let image_paths = input.image_paths.clone();

        if !self.thread_active {
            let resolved_mentions = self.resolve_mentions(mention_instances)?;
            return self
                .fresh_start_with_transcript(
                    &transcript,
                    user_text,
                    image_paths,
                    resolved_mentions.as_deref(),
                    input.workspace_access,
                    false,
                    clarify_requested,
                )
                .await;
        }

        self.run_foreground(input).await
    }

    /// Drop the seeded provider thread and replay the durable transcript through
    /// a new instruction epoch with one full baseline and task snapshot.
    #[allow(
        clippy::too_many_arguments,
        reason = "the replayed turn carries every per-turn input of the turn it replaces"
    )]
    async fn fresh_start_with_transcript(
        &mut self,
        transcript: &str,
        user_text: &str,
        image_paths: Vec<PathBuf>,
        resolved_mentions: Option<&str>,
        workspace_access: WorkspaceAccess,
        reset_epoch: bool,
        clarify_requested: bool,
    ) -> Result<AgentTurnResult, AgentEngineError> {
        self.executor
            .reset()
            .await
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        self.thread_active = false;
        if reset_epoch {
            self.session_store
                .reset_context_epoch(&self.session_id, &self.session_prompt_baseline(), true)
                .map_err(|error| AgentEngineError::new(error.to_string()))?;
        }
        let mut turn_text = self
            .prepare_eps_context(user_text, resolved_mentions, Some(transcript), true)
            .await?;
        // A resumed worker session keeps its rules too.
        if self.worker_origin.is_some() {
            turn_text.push_str("\n\n");
            turn_text.push_str(self.runtime.worker_profile().prompt());
        } else {
            turn_text.push_str("\n\n");
            turn_text.push_str(crate::worker::profile::PARENT_PROMPT);
        }
        if clarify_requested {
            turn_text.push_str("\n\n");
            turn_text.push_str(CLARIFY_REQUESTED_NOTE);
        }
        let result = self
            .run_foreground(AgentTurnInput {
                text: turn_text,
                image_paths,
                workspace_root: None,
                workspace_temp: None,
                workspace_access,
                native_tools_enabled: self.runtime.worker_profile().can_write(),
                output_schema: None,
                forbid_tools: false,
            })
            .await?;
        self.thread_active = true;
        Ok(result)
    }

    /// Record whatever changed outside the app before this turn touches
    /// anything, so the turn's own commit holds only the turn's work.
    ///
    /// This is what replaces the workspace's three-way merge: the user's
    /// SCMDraft or editor change is not folded into the agent's view, it
    /// becomes its own commit that the turn then starts from.
    async fn commit_external_edits_before_turn(&self) {
        let Ok(root) = self.runtime.project_root() else {
            return;
        };
        if !crate::git::auto_commit_ready(&root) {
            return;
        }
        let writes = self.runtime.write_coordinator().clone();
        let scope = self.git_scope();
        let committed = run_git_mutation(writes.clone(), scope.clone(), move || {
            crate::git::commit_external_edits_except(&root, &writes.claimed_in_flight(&scope))
        })
        .await;
        let event = match committed {
            Ok(None) => return,
            Ok(external) => ipc::GitTurnEvent {
                external,
                turn: None,
                warning: None,
            },
            Err(error) => ipc::GitTurnEvent {
                external: None,
                turn: None,
                warning: Some(format!(
                    "앱 밖에서 바뀐 파일을 기록하지 못했습니다: {error}"
                )),
            },
        };
        let _ = self.sink.emit(EngineEvent::Git(event));
    }

    /// Commit the work one boundary finished, and report what was recorded.
    ///
    /// A repository problem is reported, never raised: the work is already on
    /// disk, and failing the turn over a missing history entry would cost the
    /// user more than the entry is worth.
    async fn commit_boundary(
        &self,
        summary: &str,
        request_id: &str,
    ) -> Option<crate::git::CommitRecord> {
        let Ok(root) = self.runtime.project_root() else {
            return None;
        };
        // A release `release_prepare` set up is tagged on this commit once the
        // request built the current revision successfully.
        let release = self.runtime.pending_release(request_id);
        if !crate::git::auto_commit_ready(&root) {
            if let Some(version) = release {
                let _ = self.sink.emit(EngineEvent::Git(ipc::GitTurnEvent {
                    external: None,
                    turn: None,
                    warning: Some(format!(
                        "git 자동 커밋을 쓰지 않는 프로젝트라 배포 태그 {}{version}를 만들지 않았습니다.",
                        crate::release::TAG_PREFIX
                    )),
                }));
            }
            return None;
        }
        let built = release.is_some() && self.runtime.built_current_revision();
        let writes = self.runtime.write_coordinator().clone();
        let scope = self.git_scope();
        // A worker's commits carry its title and name the parent session.
        let (summary, trailer) = match &self.worker_origin {
            Some((parent, title)) => (
                crate::worker::worker_commit_summary(title),
                format!("parent-session: {parent}\n"),
            ),
            None => (summary.to_string(), String::new()),
        };
        let (session_id, request_id) = (self.session_id.clone(), request_id.to_string());
        let tag_request = request_id.clone();
        let committed = run_git_mutation(writes.clone(), scope.clone(), move || {
            let mut owned = commit_owned_paths_with(
                &root,
                &writes,
                &scope,
                &session_id,
                &request_id,
                &summary,
                &trailer,
            )?;
            let mut tagged = false;
            if let Some(version) = &release {
                match crate::release::tag_release(&root, version, owned.record.as_ref(), built) {
                    Ok(_) => tagged = true,
                    Err(warning) => {
                        owned.warning = Some(match owned.warning.take() {
                            Some(earlier) => format!("{earlier}\n{warning}"),
                            None => warning,
                        });
                    }
                }
            }
            Ok((owned, tagged))
        })
        .await;
        let committed = committed.map(|(owned, tagged)| {
            if tagged {
                self.runtime.clear_pending_release(&tag_request);
            }
            owned
        });
        let (record, event) = match committed {
            Ok(OwnedCommit {
                record: None,
                warning: None,
            }) => return None,
            Ok(OwnedCommit { record, warning }) => (
                record.clone(),
                ipc::GitTurnEvent {
                    external: None,
                    turn: record,
                    warning,
                },
            ),
            Err(error) => (
                None,
                ipc::GitTurnEvent {
                    external: None,
                    turn: None,
                    warning: Some(format!("이 턴을 기록하지 못했습니다: {error}")),
                },
            ),
        };
        let _ = self.sink.emit(EngineEvent::Git(event));
        if let Some(record) = &record {
            self.last_turn_commits.lock().push(record.clone());
        }
        record
    }

    /// What the chat turn that just ended produced, for a worker's
    /// settlement: its answer, whether it was cancelled, its commits and the
    /// files they changed, and the verdict of its last build.
    pub(crate) fn worker_turn_outcome(&self) -> crate::worker::WorkerTurnOutcome {
        let commits = self
            .last_turn_commits
            .lock()
            .iter()
            .map(|record| record.sha.clone())
            .collect::<Vec<_>>();
        let changed_files = match self.runtime.project_root() {
            Ok(root) if !commits.is_empty() => crate::worker::changed_files(&root, &commits),
            _ => Vec::new(),
        };
        crate::worker::WorkerTurnOutcome {
            answer: self.last_answer.clone(),
            cancelled: self.last_turn_cancelled,
            commits,
            changed_files,
            build_ok: self
                .runtime
                .last_build_evidence()
                .map(|evidence| evidence.ok),
            error: None,
        }
    }

    /// The key git mutations serialize under and path claims are kept under,
    /// shared by EPS and Map sessions and by a history revert; see
    /// `ToolServices::git_scope`. Without an open project it falls back to
    /// `eps:` of this session's own project id.
    fn git_scope(&self) -> String {
        self.runtime
            .git_scope()
            .unwrap_or_else(|| crate::write_coordinator::TxnKey::git(&self.project_id))
    }

    /// Whether an autonomous checkpoint whose project is a manifest name
    /// (written before project ids) belongs to this session's project: the
    /// open project carries that live name and is this session's project.
    fn accepts_legacy_run_project(&self, run: &crate::autonomous::AutonomousRunState) -> bool {
        self.runtime.services_native_open().is_ok_and(|project| {
            project.manifest().name == run.project_id
                && crate::project_id::ProjectId::of(&project) == self.project_id
        })
    }

    /// The session workspace: the CLI cwd, temp dir, and artifact root. The
    /// foreground prepares it on its first turn; a plan may be approved before
    /// any foreground turn ran (or after a restart), so that path prepares it.
    pub(crate) async fn prepared_workspace(&self) -> Result<PreparedWorkspace, AgentEngineError> {
        if let Some(workspace) = self.executor.current_workspace() {
            return Ok(workspace);
        }
        let manager = self.runtime.workspace_manager();
        let session_id = self.session_id.clone();
        tokio::task::spawn_blocking(move || manager.prepare_session_current(&session_id))
            .await
            .map_err(|error| AgentEngineError::new(error.to_string()))?
            .map_err(AgentEngineError::new)
    }

    /// After a successful turn, persist the exact provider conversation and
    /// still-pending changeset ownership. The panel log is saved separately.
    async fn update_active_session(&mut self) {
        let conversation = self.executor.conversation_state();
        let pending_request_ids = self.live_pending_request_ids();
        if let Err(error) = self.session_store.update_runtime_state(
            &self.session_id,
            conversation,
            pending_request_ids,
        ) {
            eprintln!("eud-agent: active session update failed: {error}");
            return;
        }
        if let Err(error) = self.executor.acknowledge_persisted().await {
            eprintln!("eud-agent: persisted provider receipt acknowledgement failed: {error}");
        }
    }

    /// The single live (un-archived) changeset req-id, if the current request still
    /// has journaled items (decision C: at most one is reconnected). Returns empty
    /// otherwise so a settled session drops its pending list.
    fn live_pending_request_ids(&self) -> Vec<String> {
        let Some(request_id) = self.current_request_id.clone() else {
            return Vec::new();
        };
        match self.journal_store.changeset(&request_id) {
            Ok(changeset) if !changeset.items.is_empty() => vec![request_id],
            _ => Vec::new(),
        }
    }

    /// A `propose_plan` tool call during the turn parks its markdown on the
    /// runtime; if the open request left one, the turn ends as a plan review
    /// rather than a plain answer (feature 11: propose_plan ends the turn).
    fn reinterpret_plan(&self, result: AgentTurnResult) -> AgentTurnResult {
        if matches!(
            &result,
            AgentTurnResult::Cancelled | AgentTurnResult::WriteTransition
        ) {
            if let Some(request_id) = self.current_request_id.as_deref() {
                let _ = self.runtime.take_pending_plan(request_id);
            }
            return result;
        }
        if let Some(request_id) = self.current_request_id.as_deref() {
            if let Some(markdown) = self.runtime.take_pending_plan(request_id) {
                return AgentTurnResult::Plan { markdown };
            }
        }
        result
    }

    pub async fn plan_feedback(
        &mut self,
        req: ipc::PlanFeedbackRequest,
    ) -> Result<(), AgentEngineError> {
        self.ensure_provider_conversation_ready()?;
        let resolved_mentions = self.resolve_mentions(&req.mentions)?;
        self.set_client_turn_id(&req.client_turn_id)?;
        self.phase = Phase::PlanReview;
        let mut attachment_context = self.resolve_attachments(&req.attachments)?;
        let audio_files = std::mem::take(&mut attachment_context.audio_files);
        let request_id = self
            .current_request_id
            .clone()
            .ok_or_else(|| AgentEngineError::new("no request is awaiting plan feedback"))?;
        let audio_refs =
            Self::bind_audio_attachments(self.runtime.clone(), &request_id, audio_files).await?;
        let plain_user_text = if req.text.trim().is_empty() && !req.attachments.is_empty() {
            "첨부한 파일을 반영해 계획을 수정해 주세요."
        } else if req.text.trim().is_empty() && !req.mentions.is_empty() {
            "참조한 리소스를 반영해 계획을 수정해 주세요."
        } else {
            req.text.as_str()
        };
        let mut user_text = attachment_context.append_text_files(plain_user_text);
        if !audio_refs.is_empty() {
            user_text.push_str("\n\n[audio attachments]\n");
            user_text.push_str(&serde_json::to_string(&audio_refs).map_err(|error| {
                AgentEngineError::new(format!(
                    "trusted audio references could not be serialized: {error}"
                ))
            })?);
        }
        let turn_text = self
            .prepare_eps_context(&user_text, resolved_mentions.as_deref(), None, false)
            .await?;
        let result = self
            .run_foreground(AgentTurnInput {
                text: turn_text,
                image_paths: attachment_context.image_paths,
                workspace_root: None,
                workspace_temp: None,
                workspace_access: WorkspaceAccess::Read,
                native_tools_enabled: self.runtime.worker_profile().can_write(),
                output_schema: None,
                forbid_tools: false,
            })
            .await?;
        self.commit_context_delivery(&result).await;
        self.thread_active = true;
        let result = self.reinterpret_plan(result);
        let state_result = result.clone();
        self.handle_turn_result(result)?;
        self.update_task_state_after_turn(&state_result, &user_text, resolved_mentions.as_deref())
            .await;
        self.update_active_session().await;
        Ok(())
    }

    pub async fn plan_approve(&mut self) -> Result<(), AgentEngineError> {
        self.ensure_provider_conversation_ready()?;
        if self.runtime.current_request_id().is_none() || self.current_plan_markdown.is_none() {
            return Err(AgentEngineError::new(
                "no request is awaiting plan approval",
            ));
        }
        let plan = self.current_plan_markdown.as_deref().unwrap_or_default();
        let sha256 = crate::task_state::sha256_bytes(plan.as_bytes());
        self.approved_plan_sha256 = Some(sha256.clone());
        let ticket = self
            .runtime
            .register_write_request("approved plan execution")
            .map_err(AgentEngineError::new)?;
        self.pending_write = Some(WriteContinuation::ApprovedPlan);
        self.phase = match ticket.state() {
            crate::write_coordinator::TicketState::Granted => Phase::Executing,
            crate::write_coordinator::TicketState::Cancelled => Phase::Idle,
        };
        Ok(())
    }

    pub async fn continue_pending_write(&mut self) -> Result<(), AgentEngineError> {
        let result = self.continue_pending_write_turn().await;
        self.end_turn_claims();
        result
    }

    async fn continue_pending_write_turn(&mut self) -> Result<(), AgentEngineError> {
        self.ensure_provider_conversation_ready()?;
        let ticket = self
            .runtime
            .write_ticket()
            .ok_or_else(|| AgentEngineError::new("no write ticket is pending"))?;
        if ticket.state() != crate::write_coordinator::TicketState::Granted
            || !self.runtime.owns_write_registration()
        {
            return Err(AgentEngineError::new(
                "the concurrent workspace write registration is no longer active",
            ));
        }
        let continuation = self
            .pending_write
            .ok_or_else(|| AgentEngineError::new("no write continuation is pending"))?;
        let request_id = self
            .current_request_id
            .clone()
            .ok_or_else(|| AgentEngineError::new("no request is awaiting write execution"))?;
        self.phase = Phase::Executing;

        let instruction = match continuation {
            WriteContinuation::Direct => format!(
                "The isolated live-project write registration is ready for request `{request_id}`. \
Re-read every mutation target because accepted project state may have changed since the read turn. \
Continue the requested change now, run the mandatory build, and stop only after verification."
            ),
            WriteContinuation::ApprovedPlan => {
                let markdown = self
                    .current_plan_markdown
                    .clone()
                    .ok_or_else(|| AgentEngineError::new("no plan is awaiting approval"))?;
                // A plan may be approved before any foreground turn prepared
                // the session workspace (or after a restart).
                let workspace = self.prepared_workspace().await?;
                self.runtime
                    .workspace_manager()
                    .record_plan_approval(&workspace.id, &request_id, self.plan_revision, &markdown)
                    .map_err(|error| AgentEngineError::new(error.to_string()))?;
                approved_plan_execution_instruction(&request_id)?
            }
        };

        let turn_text = self
            .prepare_eps_context(&instruction, None, None, false)
            .await?;
        let result = self
            .run_foreground(AgentTurnInput::text(turn_text).with_access(WorkspaceAccess::Write))
            .await?;
        self.commit_context_delivery(&result).await;
        self.thread_active = true;
        let result = self.reinterpret_plan(result);
        let state_result = result.clone();
        self.handle_turn_result(result)?;
        self.pending_write = None;
        let compiler_user_text = self.current_user_text.clone();
        // This turn applied the change, so this turn records it. Leaving it for
        // the next turn's external-edit sweep would file the agent's own work
        // as something the user did outside the app.
        self.commit_boundary(&compiler_user_text, &request_id).await;
        self.settle_request().await?;
        self.settle_write_lifecycle()?;
        self.current_request_id = None;
        self.update_task_state_after_turn(&state_result, &compiler_user_text, None)
            .await;
        self.update_active_session().await;
        Ok(())
    }

    pub fn recover_write_failure(&mut self) -> Result<(), AgentEngineError> {
        self.pending_write = None;
        self.settle_write_lifecycle()
    }

    fn recover_read_failure(&mut self) -> Result<(), AgentEngineError> {
        self.pending_write = None;
        // An EPS chat turn writes from its first call, so a failed one may
        // have mutated: it releases like a failed write turn.
        if self.runtime.abort_unmutated_write_intent().is_err() {
            self.runtime
                .release_write_registration()
                .map_err(AgentEngineError::new)?;
        }
        self.runtime.clear_current();
        self.current_request_id = None;
        self.phase = Phase::Idle;
        Ok(())
    }

    fn settle_write_lifecycle(&mut self) -> Result<(), AgentEngineError> {
        self.runtime
            .release_write_registration()
            .map_err(AgentEngineError::new)?;
        self.phase = Phase::Idle;
        Ok(())
    }

    pub async fn settle_request(&mut self) -> Result<(), AgentEngineError> {
        let Some(request_id) = self.current_request_id.clone() else {
            return Ok(());
        };
        // A sound import that was never built is not a finished turn: the map
        // carries the new wav but no plugin references it yet.
        if self.runtime.sound_build_required() {
            return Err(AgentEngineError::new(
                "map sound import requires one complete build_run attempt",
            ));
        }
        let decision_ids = journal::DecisionIds::All;
        let accepted_entries = self.collect_applied_entries(&request_id);
        if accepted_entries.is_empty() {
            return Ok(());
        }
        let accepted_wiki_entries = self.collect_applied_wiki_entries(&request_id);
        let accepted_workspace_entries = accepted_entries
            .iter()
            .filter(|entry| matches!(entry.target, journal::JournalTarget::WorkspacePath { .. }))
            .cloned()
            .collect::<Vec<_>>();

        let runtime = self.runtime.clone();
        let settled: bool = runtime
            .project_transaction(|| {
                (|| {
                    self.runtime
                        .workspace_manager()
                        .record_accepted_entries(&request_id, &accepted_workspace_entries)
                        .map_err(|error| AgentEngineError::new(error.to_string()))?;
                    if let Some(payload) = self.record_applied_wiki_edits(accepted_wiki_entries) {
                        self.sink.emit(EngineEvent::Wiki(payload))?;
                    }
                    self.journal_store
                        .accept_entries(&request_id, &decision_ids)
                        .map_err(|error| AgentEngineError::new(error.to_string()))
                })()
            })
            .map_err(AgentEngineError::new)??;

        if settled {
            if let Ok(record) = self.session_store.load(&self.session_id) {
                let event = crate::task_state::TaskStateEvent::new(
                    self.current_client_turn_id.clone(),
                    Some(request_id.clone()),
                    crate::task_state::TaskStateEventKind::RequestAccepted {
                        journal_entry_ids: accepted_entries
                            .iter()
                            .map(|entry| entry.id.clone())
                            .collect(),
                        harness_job_id: None,
                    },
                );
                if let Err(error) = self.session_store.append_task_event(
                    &self.session_id,
                    record.task_state.leaf_id.as_deref(),
                    event,
                ) {
                    eprintln!("eud-agent: request settlement task-state append failed: {error}");
                }
            }
            self.settle_autonomous_review().await?;
            self.drop_pending_request_from_session(&request_id);
            self.current_client_turn_id = None;
            self.approved_plan_sha256 = None;
            self.runtime.clear_audio_cache();
        }
        self.update_active_session().await;
        Ok(())
    }

    /// Remove `request_id` from the active session record's `pendingRequestIds`
    /// (decision C: the reconnect list). Best-effort and a no-op when no session is
    /// active or the record is gone.
    fn drop_pending_request_from_session(&mut self, request_id: &str) {
        if let Err(error) = self
            .session_store
            .drop_pending_request(&self.session_id, request_id)
        {
            eprintln!("eud-agent: session pending-id drop failed: {error}");
        }
    }

    /// Collect the ACCEPTED dat property changes of the current changeset as wiki
    /// ledger entries. Returns an empty vec when there is no journal/changeset, no
    /// dat edits, or the decision is a reject (the wiki records accepted dat edits
    /// only). Must be called BEFORE a full-accept archives the journal.
    fn collect_applied_wiki_entries(&self, request_id: &str) -> Vec<crate::wiki::LedgerEntry> {
        let Ok(changeset) = self.journal_store.changeset(request_id) else {
            return Vec::new();
        };
        let Some(journal) = self.load_journal(request_id) else {
            return Vec::new();
        };
        crate::wiki::applied_ledger_entries(&changeset, &journal)
    }

    fn collect_applied_entries(&self, request_id: &str) -> Vec<journal::JournalEntry> {
        self.journal_store
            .selected_entries(request_id, &journal::DecisionIds::All)
            .unwrap_or_default()
    }

    /// Load the raw journal for a request so the wiki hook can read each property's
    /// `ts`. The journal may live only in the in-memory store, so persist it first
    /// (idempotent and cheap; the executor already persists on each write) and read
    /// it back through the same loader the summary fallback uses.
    fn load_journal(&self, request_id: &str) -> Option<journal::Journal> {
        let _ = self.journal_store.persist(request_id);
        journal::JournalStore::load(&self.journal_data_dir, request_id).ok()
    }

    /// Upsert the applied dat edits to the project ledger via the wiki
    /// provider, returning the updated ledger for emission (or `None` when nothing
    /// was recorded / no provider is wired).
    fn record_applied_wiki_edits(
        &self,
        entries: Vec<crate::wiki::LedgerEntry>,
    ) -> Option<ipc::WikiResponse> {
        if entries.is_empty() {
            return None;
        }
        self.config
            .wiki_provider
            .as_ref()
            .and_then(|provider| provider.record_accepted(entries))
    }

    /// Compact the live provider conversation without changing panel history or
    /// backend-owned plan/review state.
    pub async fn compact(&mut self) -> Result<(), AgentEngineError> {
        if !self.thread_active || !self.executor.conversation_state().is_started() {
            return Err(AgentEngineError::new(
                "압축할 provider 대화가 없습니다. 먼저 메시지를 보내 주세요.",
            ));
        }
        if matches!(self.phase, Phase::Triage | Phase::Executing) {
            return Err(AgentEngineError::new(
                "현재 provider 작업이 끝난 뒤 대화를 압축해 주세요.",
            ));
        }
        let record = self
            .session_store
            .load(&self.session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let next_instruction_epoch = record
            .context_state
            .instruction_epoch
            .max(1)
            .saturating_add(1);
        let workspace_root = match self.executor.current_workspace() {
            Some(workspace) => workspace.root,
            None => {
                let workspace = self.runtime.workspace_manager();
                let session_id = self.session_id.clone();
                tokio::task::spawn_blocking(move || workspace.prepare_session_current(&session_id))
                    .await
                    .map_err(|error| AgentEngineError::new(error.to_string()))?
                    .map_err(|error| AgentEngineError::new(error.to_string()))?
                    .root
            }
        };
        let request_id = format!("compact-{}", next_request_id());
        let request = CompactionRequest {
            identity: self.run_identity(&request_id),
            binding: self.binding_snapshot(false)?,
            workspace_root,
            next_instruction_epoch,
            policy: RunPolicy {
                active_deadline: None,
                shutdown_grace: std::time::Duration::from_secs(2),
                max_output_bytes: 1024 * 1024,
                max_output_tokens: None,
                max_tool_rounds: 0,
                allow_resume: false,
            },
        };
        self.executor
            .compact(request)
            .await
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let committed_epoch = self
            .session_store
            .record_compaction_boundary(&self.session_id, &self.session_prompt_baseline())
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        if committed_epoch != next_instruction_epoch {
            return Err(AgentEngineError::new(
                "compaction instruction epoch changed before commit",
            ));
        }
        self.update_active_session().await;
        Ok(())
    }

    /// Replace the model-visible conversation with the durable panel-log prefix
    /// selected by a message edit. The active saved session is retained, while
    /// the next chat starts a fresh Codex thread seeded from that prefix.
    pub async fn rewind(&mut self, panel_log: serde_json::Value) -> Result<(), AgentEngineError> {
        if matches!(self.phase, Phase::PlanReview | Phase::Executing) {
            return Err(AgentEngineError::new(
                "현재 세션의 진행 중인 요청 또는 검토를 먼저 완료해 주세요.",
            ));
        }
        self.executor
            .reset()
            .await
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        self.thread_active = false;
        self.conversation_resume_error = None;
        self.phase = Phase::Idle;
        self.current_plan_markdown = None;
        self.runtime.clear_current();
        self.current_request_id = None;
        self.current_client_turn_id = None;
        self.approved_plan_sha256 = None;
        self.pending_context_delivery = None;

        let transcript = condense_transcript(&panel_log);
        self.pending_resume_transcript = (!transcript.trim().is_empty()).then_some(transcript);

        self.session_store
            .move_task_leaf_for_rewind(&self.session_id, panel_log)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        Ok(())
    }

    fn resolve_attachments(&self, ids: &[String]) -> Result<AttachmentContext, AgentEngineError> {
        if ids.is_empty() {
            return Ok(AttachmentContext {
                image_paths: Vec::new(),
                images: Vec::new(),
                text_files: Vec::new(),
                audio_files: Vec::new(),
            });
        }
        self.attachment_store
            .bind_and_resolve(ids, &self.session_id)
            .map_err(AgentEngineError::new)
    }

    async fn bind_audio_attachments(
        runtime: SessionToolRuntime,
        request_id: &str,
        attachments: Vec<crate::attachment::ResolvedAudioAttachment>,
    ) -> Result<Vec<crate::audio::TrustedAudioRef>, AgentEngineError> {
        if attachments.is_empty() {
            return Ok(Vec::new());
        }
        let request_id = request_id.to_string();
        tokio::task::spawn_blocking(move || {
            runtime.bind_audio_attachments(&request_id, attachments)
        })
        .await
        .map_err(|error| {
            AgentEngineError::new(format!("audio attachment probe task failed: {error}"))
        })?
        .map_err(AgentEngineError::new)
    }

    /// Hydrate this worker's persisted thread and pending review exactly once.
    /// No other session is reset or activated.
    pub async fn hydrate(&mut self) -> Result<crate::session::SessionRecord, AgentEngineError> {
        let record = self
            .session_store
            .load(&self.session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        if self.hydrated {
            self.sink
                .emit(EngineEvent::SessionLoaded(ipc::SessionLoadedEvent {
                    id: self.session_id.clone(),
                }))?;
            return Ok(record);
        }

        let transcript = condense_transcript(&record.panel_log);
        let staged = (!transcript.is_empty()).then_some(transcript);
        let conversation = record.provider_binding.conversation.clone();
        let mut seeded_conversation = false;
        let seed_error = if conversation.is_started() {
            match self.executor.seed(conversation).await {
                Ok(()) => {
                    seeded_conversation = true;
                    self.thread_active = true;
                    self.pending_resume_transcript = staged;
                    None
                }
                Err(error) => Some(error),
            }
        } else {
            if staged.is_some() {
                self.pending_resume_transcript = staged;
            }
            None
        };

        if let Some(request_id) = record.pending_request_ids.first() {
            self.runtime
                .begin_request(request_id, self.project_id.as_str())
                .map_err(AgentEngineError::new)?;
            self.runtime
                .restore_review(self.project_id.as_str(), request_id)
                .map_err(AgentEngineError::new)?;
        }
        if let Some(run) = record.autonomous_run.as_ref() {
            self.execution_mode = crate::autonomous::ExecutionMode::Autonomous;
            self.autonomous_policy = run.policy.clone();
            self.current_user_text = run.goal.clone();
            self.current_client_turn_id = Some(run.client_turn_id.clone());
            self.autonomous_pause_requested
                .store(run.status.can_resume(), Ordering::SeqCst);
            if record.pending_request_ids.is_empty() && run.status.can_resume() {
                self.runtime
                    .begin_request(&run.request_id, self.project_id.as_str())
                    .map_err(AgentEngineError::new)?;
                self.runtime
                    .restore_autonomous_progress(&run.request_id, &run.progress)
                    .map_err(AgentEngineError::new)?;
                self.current_request_id = Some(run.request_id.clone());
            }
            self.sink
                .emit(EngineEvent::AutonomousRun(Box::new(run.clone())))?;
        }
        self.hydrated = true;
        self.sink
            .emit(EngineEvent::SessionLoaded(ipc::SessionLoadedEvent {
                id: self.session_id.clone(),
            }))?;
        if let Some(error) = seed_error {
            let message = format!("persisted provider conversation could not be resumed: {error}");
            self.conversation_resume_error = Some(message.clone());
            return Err(AgentEngineError::new(message));
        }
        self.conversation_resume_error = None;
        if seeded_conversation {
            self.update_active_session().await;
        }
        Ok(record)
    }

    /// Reconnect the single pending (un-archived) changeset for an opened session
    /// (decision C). Rehydrates the journal by id, points `current_request_id` at it
    /// (so a later `changeset_decision` guard passes), and re-emits the existing
    /// `changeset` event. A missing journal / empty changeset degrades gracefully
    /// (skip + log), never panics.
    fn resolve_mentions(
        &self,
        mentions: &[crate::mentions::MentionInstance],
    ) -> Result<Option<String>, AgentEngineError> {
        if self.session_kind != crate::session::SessionKind::Eps && !mentions.is_empty() {
            return Err(AgentEngineError::new(
                "Map sessions use their separate candidate mention contract.",
            ));
        }
        let project = self.runtime.map_context().map_err(AgentEngineError::new)?;
        self.runtime
            .mentions()
            .resolve_all(
                &project,
                mentions,
                self.runtime.project_root().ok().as_deref(),
            )
            .map_err(AgentEngineError::new)
    }

    async fn update_task_state_after_turn(
        &mut self,
        result: &AgentTurnResult,
        compiler_user_text: &str,
        resolved_mentions: Option<&str>,
    ) {
        if matches!(result, AgentTurnResult::Cancelled)
            || (self.worker_origin.is_none()
                && compiler_user_text != crate::worker::WORKER_CONTINUE_TEXT)
        {
            self.compile_task_state_after_turn(result, compiler_user_text, resolved_mentions)
                .await;
        }
        let request_goal = (compiler_user_text != crate::worker::WORKER_CONTINUE_TEXT)
            .then_some(compiler_user_text);
        self.persist_work_checkpoint(request_goal);
    }

    fn persist_work_checkpoint(&self, request_goal: Option<&str>) {
        if self.worker_origin.is_some() {
            return;
        }
        let result = (|| -> anyhow::Result<()> {
            self.runtime.project_root().map_err(anyhow::Error::msg)?;
            let workspace = self
                .runtime
                .workspace_manager()
                .prepare_session_current(&self.session_id)
                .map_err(anyhow::Error::msg)?;
            let record = self.session_store.load(&self.session_id)?;
            let revision = self
                .runtime
                .current_project_revision()
                .map_err(anyhow::Error::msg)?;
            crate::work_checkpoint::WorkCheckpoint::capture(
                &workspace.id,
                &record,
                revision,
                request_goal,
                self.current_plan_markdown.as_deref(),
                self.runtime.last_build_evidence(),
            )
            .save(&workspace.root)
        })();
        if let Err(error) = result {
            eprintln!("eud-agent: project work checkpoint unavailable: {error}");
        }
    }

    async fn compile_task_state_after_turn(
        &mut self,
        result: &AgentTurnResult,
        compiler_user_text: &str,
        resolved_mentions: Option<&str>,
    ) {
        let Some(request_id) = self.current_request_id.clone() else {
            return;
        };
        let Some(client_turn_id) = self.current_client_turn_id.clone() else {
            return;
        };
        let record = match self.session_store.load(&self.session_id) {
            Ok(record) => record,
            Err(error) => {
                eprintln!("eud-agent: task-state load failed: {error}");
                return;
            }
        };
        let expected_leaf = record.task_state.leaf_id.clone();
        if matches!(result, AgentTurnResult::Cancelled) {
            let event = crate::task_state::TaskStateEvent::new(
                Some(client_turn_id),
                Some(request_id),
                crate::task_state::TaskStateEventKind::TurnCancelled,
            );
            if let Err(error) = self.session_store.append_task_event(
                &self.session_id,
                expected_leaf.as_deref(),
                event,
            ) {
                eprintln!("eud-agent: cancelled task-state event append failed: {error}");
            }
            return;
        }

        let foreground_result = match result {
            AgentTurnResult::Answer { text } => text.as_str(),
            AgentTurnResult::Plan { markdown } => markdown.as_str(),
            AgentTurnResult::Cancelled
            | AgentTurnResult::WriteTransition
            | AgentTurnResult::IterationBoundary { .. } => return,
        };
        let workspace_root = self
            .executor
            .current_workspace()
            .map(|workspace| workspace.workspace_root);
        let artifact_candidates = match workspace_root.as_deref() {
            Some(root) => match crate::task_state::collect_artifact_candidates(root) {
                Ok(candidates) => candidates,
                Err(error) => {
                    eprintln!("eud-agent: task-state artifact catalog failed: {error}");
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        let journal_summary = self
            .journal_store
            .changeset(&request_id)
            .ok()
            .and_then(|changeset| serde_json::to_string(&changeset).ok())
            .unwrap_or_else(|| "{\"items\":[]}".to_string());
        let build_evidence = self
            .runtime
            .last_build_evidence()
            .and_then(|evidence| serde_json::to_value(evidence).ok());
        let approved_plan = self
            .approved_plan_sha256
            .as_ref()
            .and(self.current_plan_markdown.as_deref());
        let input = crate::task_state::TaskStateCompilerInput {
            previous_projection: &record.task_state.projection,
            current_user_text: compiler_user_text,
            resolved_mentions,
            request_id: &request_id,
            client_turn_id: &client_turn_id,
            approved_plan,
            foreground_result,
            journal_summary: &journal_summary,
            build_evidence: build_evidence.as_ref(),
            artifact_candidates: &artifact_candidates,
        };
        let prompt = match input.prompt() {
            Ok(prompt) => prompt,
            Err(error) => {
                self.record_task_compilation_failure("input_too_large", error);
                return;
            }
        };
        if workspace_root.is_none() {
            return;
        }
        let compiler_workspace = match crate::provider_runtime::CompilerInputWorkspace::prepare(
            &self.runtime.data_dirs(),
        ) {
            Ok(workspace) => workspace,
            Err(error) => {
                self.record_task_compilation_failure("workspace_error", error.to_string());
                return;
            }
        };
        let base = JobBase {
            revision: record.task_state.projection.revision,
            instruction_epoch: record.context_state.instruction_epoch,
            branch: expected_leaf.clone(),
        };
        let structured_request = StructuredJobRequest {
            identity: self.run_identity(&format!("{request_id}-task-state")),
            binding: match self.binding_snapshot(true) {
                Ok(binding) => binding,
                Err(error) => {
                    self.record_task_compilation_failure("binding_error", error.to_string());
                    return;
                }
            },
            kind: StructuredJobKind::TaskStateCompiler,
            prompt,
            workspace_root: compiler_workspace.root().to_path_buf(),
            output_schema: crate::task_state::compiler_output_schema(),
            base: base.clone(),
            policy: RunPolicy {
                active_deadline: Some(TASK_STATE_COMPILER_DEADLINE),
                shutdown_grace: std::time::Duration::from_secs(2),
                max_output_bytes: crate::task_state::MAX_SEMANTIC_EVENT_BYTES,
                max_output_tokens: Some(TASK_STATE_COMPILER_OUTPUT_TOKENS),
                max_tool_rounds: 0,
                allow_resume: false,
            },
        };
        let outcome = self.executor.run_structured(structured_request).await;
        if let Err(error) = compiler_workspace.close() {
            eprintln!("eud-agent: task-state compiler cleanup failed: {error}");
            if let Err(error) = self.sink.emit(EngineEvent::Progress(ipc::ProgressEvent {
                stage: ipc::ProgressStage::TaskStateWarning,
                detail: Some("상태 계산용 임시 작업 폴더를 정리하지 못했습니다.".into()),
                provider: Some(self.provider_binding.provider),
                model: Some(self.provider_binding.model.clone()),
            })) {
                eprintln!("eud-agent: compiler cleanup warning delivery failed: {error}");
            }
        }
        let output = match outcome {
            RunOutcome::Structured {
                value,
                base: returned_base,
            } if returned_base == base => value,
            RunOutcome::Structured { .. } => {
                self.record_task_compilation_failure(
                    "stale_base",
                    "task-state compiler returned a stale base",
                );
                return;
            }
            RunOutcome::Failed(error) => {
                let kind = if matches!(
                    error,
                    crate::provider_runtime::ProviderRuntimeError::TimedOut
                ) {
                    "timeout"
                } else {
                    "runtime_error"
                };
                self.record_task_compilation_failure(kind, error.to_string());
                return;
            }
            RunOutcome::Cancelled => {
                self.record_task_compilation_failure("cancelled", "task-state compiler cancelled");
                return;
            }
            RunOutcome::Completed { .. }
            | RunOutcome::IterationBoundary { .. }
            | RunOutcome::WriteTransition => {
                self.record_task_compilation_failure(
                    "invalid_outcome",
                    "task-state compiler returned a foreground outcome",
                );
                return;
            }
        };
        let latest = match self.session_store.load(&self.session_id) {
            Ok(latest) => latest,
            Err(error) => {
                self.record_task_compilation_failure("reload_failed", error.to_string());
                return;
            }
        };
        if latest.task_state.projection.revision != base.revision
            || latest.context_state.instruction_epoch != base.instruction_epoch
            || latest.task_state.leaf_id != base.branch
        {
            self.record_task_compilation_failure(
                "append_conflict",
                "task-state base changed while compiler was running",
            );
            return;
        }
        let output = match serde_json::to_string(&output) {
            Ok(output) => output,
            Err(error) => {
                self.record_task_compilation_failure("invalid_output", error.to_string());
                return;
            }
        };
        let delta = match crate::task_state::parse_compiler_delta(&output) {
            Ok(delta) => delta,
            Err(error) => {
                self.record_task_compilation_failure("invalid_output", error);
                return;
            }
        };
        let accepted_journal_entry_ids = HashSet::new();
        let approved_plan_evidence = match (
            self.current_plan_markdown.as_deref(),
            self.approved_plan_sha256.as_deref(),
        ) {
            (Some(markdown), Some(sha256)) => Some(crate::task_state::ApprovedPlanEvidence {
                request_id: &request_id,
                markdown,
                sha256,
            }),
            _ => None,
        };
        let validation = crate::task_state::ProvenanceValidationContext {
            client_turn_id: &client_turn_id,
            user_text: compiler_user_text,
            request_id: &request_id,
            approved_plan: approved_plan_evidence,
            workspace_root: workspace_root.as_deref(),
            accepted_journal_entry_ids: &accepted_journal_entry_ids,
        };
        if let Err(error) = crate::task_state::validate_compiler_delta(
            &record.task_state.projection,
            &delta,
            &validation,
        ) {
            self.record_task_compilation_failure("provenance_invalid", error);
            return;
        }
        let event = crate::task_state::TaskStateEvent::new(
            Some(client_turn_id),
            Some(request_id),
            crate::task_state::TaskStateEventKind::SemanticDelta { delta },
        );
        if let Err(error) =
            self.session_store
                .append_task_event(&self.session_id, expected_leaf.as_deref(), event)
        {
            self.record_task_compilation_failure("append_conflict", error.to_string());
        }
    }

    fn record_task_compilation_failure(&mut self, reason_code: &str, detail: impl Into<String>) {
        let detail = crate::task_state::bounded_compilation_detail(detail);
        eprintln!(
            "eud-agent: task-state compilation failed: session={} request={} client_turn={} reason={reason_code}: {}",
            self.session_id,
            self.current_request_id.as_deref().unwrap_or("<none>"),
            self.current_client_turn_id.as_deref().unwrap_or("<none>"),
            detail.as_deref().unwrap_or("<no detail>")
        );

        match self.session_store.load(&self.session_id) {
            Ok(record) => {
                let event = crate::task_state::TaskStateEvent::new(
                    self.current_client_turn_id.clone(),
                    self.current_request_id.clone(),
                    crate::task_state::TaskStateEventKind::StateCompilationFailed {
                        reason_code: reason_code.to_string(),
                        detail,
                    },
                );
                if let Err(error) = self.session_store.append_task_event(
                    &self.session_id,
                    record.task_state.leaf_id.as_deref(),
                    event,
                ) {
                    eprintln!("eud-agent: task-state failure event append failed: {error}");
                }
            }
            Err(error) => {
                eprintln!("eud-agent: task-state failure event load failed: {error}");
            }
        }
        let _ = self.sink.emit(EngineEvent::Progress(ipc::ProgressEvent {
            stage: ipc::ProgressStage::TaskStateWarning,
            detail: Some(
                "작업 결과는 유지되지만 구조화된 활성 작업 상태를 갱신하지 못했습니다.".to_string(),
            ),
            provider: Some(self.provider_binding.provider),
            model: Some(self.provider_binding.model.clone()),
        }));
    }

    fn handle_turn_result(&mut self, result: AgentTurnResult) -> Result<(), AgentEngineError> {
        match result {
            AgentTurnResult::Answer { text } => {
                self.last_answer = text.clone();
                self.phase = Phase::Answer;
                self.sink
                    .emit(EngineEvent::Answer(ipc::AnswerEvent { text }))?;
                self.phase = Phase::Idle;
            }
            AgentTurnResult::Plan { markdown } => {
                self.plan_revision = self
                    .plan_revision
                    .checked_add(1)
                    .ok_or_else(|| AgentEngineError::new("plan revision overflow"))?;
                self.current_plan_markdown = Some(markdown.clone());
                self.phase = Phase::PlanReview;
                self.sink.emit(EngineEvent::Plan(ipc::PlanEvent {
                    markdown,
                    revision: self.plan_revision,
                }))?;
            }
            AgentTurnResult::Cancelled => {
                self.phase = Phase::Idle;
            }
            AgentTurnResult::WriteTransition => {
                return Err(AgentEngineError::new(
                    "write transition reached answer handling",
                ));
            }
            AgentTurnResult::IterationBoundary { .. } => {
                self.phase = Phase::Idle;
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct MapEventContext {
    request_id: String,
    candidate_revision: String,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionEvent<T> {
    session_id: String,
    /// The session's project, so a window drops another project's events.
    #[serde(skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate_revision: Option<String>,
    #[serde(flatten)]
    payload: T,
}

/// Who started a Map request: the Map window's user or an EPS session's
/// `map_task_request`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MapRunOrigin {
    User,
    Team,
}

/// How a Map request's turn ended without an error: answered, or stopped by a
/// cancellation (which the engine does not report as a failure).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapTurnEnd {
    Completed,
    Cancelled,
}

pub(crate) struct MapChatResult {
    pub end: MapTurnEnd,
    pub pending_workflow_progress: Vec<crate::tool_exec::PendingWorkflowProgress>,
}

/// The request bubble of one Map run, as the Map window shows it: the raw
/// user text (or the team task goal), the mentions it carried, and its origin.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapRunPrompt {
    pub text: String,
    pub mentions: Vec<crate::map_model::MapMentionSnapshot>,
    pub origin: MapRunOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_name: Option<String>,
}

/// One scoped session event exactly as the Map window received (or missed) it.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapRunEvent {
    pub name: String,
    pub payload: serde_json::Value,
}

/// The latest Map request of one session: its prompt and every scoped event
/// it emitted so far, so a Map window that opens (or switches) mid-run can
/// show it exactly like a request typed there. Kept after the run ends until
/// the next request replaces it.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapRunTranscript {
    pub request_id: String,
    pub candidate_revision: String,
    pub in_flight: bool,
    pub prompt: MapRunPrompt,
    pub events: Vec<MapRunEvent>,
    /// Oldest events were dropped to stay within the transcript bounds.
    pub truncated: bool,
}

/// Header of a run that just started, emitted as `map_run_started` (the
/// scoped wrapper adds `sessionId`/`requestId`/`candidateRevision`).
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MapRunStartedEvent {
    prompt: MapRunPrompt,
}

const MAP_RUN_MAX_EVENTS: usize = 2000;
const MAP_RUN_MAX_BYTES: usize = 4 * 1024 * 1024;
/// The session events the Map window's turn listeners consume.
const MAP_RUN_EVENT_NAMES: [&str; 5] = ["agent_event", "context_usage", "answer", "ask", "error"];

/// The bounded event record behind [`MapRunTranscript`].
struct MapRunRecord {
    request_id: String,
    candidate_revision: String,
    in_flight: bool,
    prompt: MapRunPrompt,
    events: VecDeque<(MapRunEvent, usize)>,
    bytes: usize,
    truncated: bool,
}

impl MapRunRecord {
    fn push(&mut self, name: &str, payload: serde_json::Value) {
        let size = payload.to_string().len();
        self.events.push_back((
            MapRunEvent {
                name: name.to_string(),
                payload,
            },
            size,
        ));
        self.bytes += size;
        while self.events.len() > MAP_RUN_MAX_EVENTS
            || (self.bytes > MAP_RUN_MAX_BYTES && self.events.len() > 1)
        {
            if let Some((_, dropped)) = self.events.pop_front() {
                self.bytes -= dropped;
                self.truncated = true;
            }
        }
    }

    fn snapshot(&self) -> MapRunTranscript {
        MapRunTranscript {
            request_id: self.request_id.clone(),
            candidate_revision: self.candidate_revision.clone(),
            in_flight: self.in_flight,
            prompt: self.prompt.clone(),
            events: self.events.iter().map(|(event, _)| event.clone()).collect(),
            truncated: self.truncated,
        }
    }
}

/// The latest Map run of one session, shared by every clone of its sink and
/// readable without the engine lock.
#[derive(Clone, Default)]
struct MapRunRecorder(Arc<parking_lot::RwLock<Option<MapRunRecord>>>);

impl MapRunRecorder {
    /// Start a fresh in-flight transcript, replacing the previous run's.
    fn start(&self, request_id: String, candidate_revision: String, prompt: MapRunPrompt) {
        *self.0.write() = Some(MapRunRecord {
            request_id,
            candidate_revision,
            in_flight: true,
            prompt,
            events: VecDeque::new(),
            bytes: 0,
            truncated: false,
        });
    }

    /// Append one scoped turn event to the in-flight run it belongs to; other
    /// event names, unscoped events, and events of another request are not
    /// part of the transcript.
    fn record<T: serde::Serialize>(
        &self,
        name: &str,
        event: &SessionEvent<T>,
    ) -> Result<(), serde_json::Error> {
        if !MAP_RUN_EVENT_NAMES.contains(&name) {
            return Ok(());
        }
        let Some(request_id) = event.request_id.as_deref() else {
            return Ok(());
        };
        let mut run = self.0.write();
        let Some(record) = run
            .as_mut()
            .filter(|record| record.in_flight && record.request_id == request_id)
        else {
            return Ok(());
        };
        record.push(name, serde_json::to_value(event)?);
        Ok(())
    }

    /// The run ended; its transcript stays readable until the next start.
    fn finish(&self, request_id: &str) {
        let mut run = self.0.write();
        if let Some(record) = run
            .as_mut()
            .filter(|record| record.request_id == request_id)
        {
            record.in_flight = false;
        }
    }

    fn snapshot(&self) -> Option<MapRunTranscript> {
        self.0.read().as_ref().map(MapRunRecord::snapshot)
    }
}

#[derive(Clone)]
pub(crate) struct SessionEventSink {
    app: tauri::AppHandle,
    session_id: String,
    /// The session's project: its events reach only that project's windows.
    project: Option<crate::project_id::ProjectId>,
    map_context: Arc<parking_lot::RwLock<Option<MapEventContext>>>,
    map_run: MapRunRecorder,
}

impl SessionEventSink {
    /// The sink of a stored session, routed to its project's windows.
    pub(crate) fn new(app: tauri::AppHandle, session_id: impl Into<String>) -> Self {
        let session_id = session_id.into();
        let project = tauri::Manager::try_state::<SessionEngineManager>(&app)
            .and_then(|manager| manager.session_project_id(&session_id));
        Self::for_project(app, session_id, project)
    }

    pub(crate) fn for_project(
        app: tauri::AppHandle,
        session_id: impl Into<String>,
        project: Option<crate::project_id::ProjectId>,
    ) -> Self {
        Self {
            app,
            session_id: session_id.into(),
            project,
            map_context: Arc::new(parking_lot::RwLock::new(None)),
            map_run: MapRunRecorder::default(),
        }
    }

    /// Emit a project-owned event to the project's windows. A session whose
    /// project is not open has no window to show it.
    fn emit_owned<T: serde::Serialize + Clone>(&self, name: &str, payload: T) -> tauri::Result<()> {
        match &self.project {
            Some(project) => crate::project_events::emit_project(
                &self.app,
                project,
                name,
                crate::project_events::ProjectScoped::new(project, payload),
            ),
            None => {
                eprintln!(
                    "eud-agent: {name} event dropped: session {} has no open project",
                    self.session_id
                );
                Ok(())
            }
        }
    }

    /// Scope the following events to one Map request, start its transcript
    /// (replacing the previous run's), and announce the run's header so an
    /// idle Map window can adopt it.
    pub(crate) fn set_map_context(
        &self,
        request_id: String,
        candidate_revision: String,
        prompt: MapRunPrompt,
    ) {
        *self.map_context.write() = Some(MapEventContext {
            request_id: request_id.clone(),
            candidate_revision: candidate_revision.clone(),
        });
        self.map_run
            .start(request_id, candidate_revision, prompt.clone());
        if let Err(error) = self.emit_scoped("map_run_started", MapRunStartedEvent { prompt }) {
            eprintln!(
                "eud-agent: map run start event failed: session={} error={error}",
                self.session_id
            );
        }
    }

    /// End the request's scope; its transcript stays readable, marked as no
    /// longer in flight, until the next request starts.
    pub(crate) fn clear_map_context(&self, request_id: &str) {
        let mut context = self.map_context.write();
        if context
            .as_ref()
            .is_some_and(|context| context.request_id == request_id)
        {
            *context = None;
        }
        self.map_run.finish(request_id);
    }

    /// The latest Map run's transcript, without touching the engine.
    pub(crate) fn map_run_snapshot(&self) -> Option<MapRunTranscript> {
        self.map_run.snapshot()
    }

    fn scoped<T>(&self, payload: T) -> SessionEvent<T> {
        let context = self.map_context.read().clone();
        SessionEvent {
            session_id: self.session_id.clone(),
            project_id: self
                .project
                .as_ref()
                .map(|project| project.as_str().to_string()),
            request_id: context.as_ref().map(|context| context.request_id.clone()),
            candidate_revision: context.map(|context| context.candidate_revision),
            payload,
        }
    }

    pub(crate) fn emit_scoped<T>(&self, name: &str, payload: T) -> tauri::Result<()>
    where
        T: serde::Serialize + Clone,
    {
        let event = self.scoped(payload);
        if let Err(error) = self.map_run.record(name, &event) {
            eprintln!(
                "eud-agent: map run event could not be recorded: session={} event={name} error={error}",
                self.session_id
            );
        }
        match &self.project {
            Some(project) => crate::project_events::emit_project(&self.app, project, name, event),
            None => {
                eprintln!(
                    "eud-agent: {name} event dropped: session {} has no open project",
                    self.session_id
                );
                Ok(())
            }
        }
    }
}

impl EventSink for SessionEventSink {
    fn emit(&self, event: EngineEvent) -> Result<(), AgentEngineError> {
        let result = match event {
            EngineEvent::Agent(payload) => self.emit_scoped("agent_event", payload),
            EngineEvent::ContextUsage(payload) => self.emit_scoped("context_usage", payload),
            EngineEvent::Answer(payload) => self.emit_scoped("answer", payload),
            EngineEvent::Plan(payload) => self.emit_scoped("plan", payload),
            EngineEvent::Progress(payload) => self.emit_scoped("progress", payload),
            EngineEvent::Error(payload) => self.emit_scoped("error", payload),
            EngineEvent::Status(payload) => self.emit_owned("status", payload),
            EngineEvent::AutonomousRun(payload) => self.emit_scoped("autonomous_run", payload),
            EngineEvent::Git(payload) => self.emit_scoped("git", payload),
            EngineEvent::Wiki(payload) => self.emit_owned("wiki", payload),
            EngineEvent::SessionLoaded(payload) => self.emit_owned("session_loaded", payload),
        };
        result.map_err(|err| AgentEngineError::new(format!("failed to emit event: {err}")))
    }
}

#[cfg(test)]
mod map_run_tests {
    use super::*;

    fn prompt(origin: MapRunOrigin) -> MapRunPrompt {
        MapRunPrompt {
            text: "Z1 테두리를 마을 패턴으로".to_string(),
            mentions: Vec::new(),
            origin,
            team_task_id: (origin == MapRunOrigin::Team).then(|| "task-1".to_string()),
            parent_session_name: (origin == MapRunOrigin::Team).then(|| "메인".to_string()),
        }
    }

    /// The scoped event exactly as `SessionEventSink::scoped` builds it while
    /// the map context is `request_id`.
    fn scoped<T>(request_id: Option<&str>, payload: T) -> SessionEvent<T> {
        SessionEvent {
            session_id: "map-1".to_string(),
            project_id: None,
            request_id: request_id.map(str::to_string),
            candidate_revision: request_id.map(|_| "r0:aaa".to_string()),
            payload,
        }
    }

    fn agent_event(detail: &str) -> ipc::AgentEvent {
        ipc::AgentEvent {
            kind: "tool_call".to_string(),
            detail: detail.to_string(),
            data: None,
        }
    }

    fn started(origin: MapRunOrigin) -> MapRunRecorder {
        let recorder = MapRunRecorder::default();
        recorder.start("map-a".to_string(), "r0:aaa".to_string(), prompt(origin));
        recorder
    }

    #[test]
    fn transcript_records_scoped_turn_events_of_the_matching_request_only() {
        let recorder = MapRunRecorder::default();
        assert!(recorder.snapshot().is_none());
        recorder
            .record(
                "agent_event",
                &scoped(None, agent_event("before any request")),
            )
            .unwrap();
        assert!(recorder.snapshot().is_none());

        recorder.start(
            "map-a".to_string(),
            "r0:aaa".to_string(),
            prompt(MapRunOrigin::Team),
        );
        let header = recorder.snapshot().unwrap();
        assert!(header.in_flight);
        assert!(header.events.is_empty());
        assert_eq!(header.prompt.origin, MapRunOrigin::Team);
        assert_eq!(header.prompt.team_task_id.as_deref(), Some("task-1"));
        assert_eq!(header.prompt.parent_session_name.as_deref(), Some("메인"));
        assert_eq!(
            serde_json::to_value(&header.prompt).unwrap(),
            serde_json::json!({
                "text": "Z1 테두리를 마을 패턴으로",
                "mentions": [],
                "origin": "team",
                "teamTaskId": "task-1",
                "parentSessionName": "메인",
            })
        );

        recorder
            .record(
                "agent_event",
                &scoped(Some("map-a"), agent_event("terrain.set")),
            )
            .unwrap();
        recorder
            .record(
                "progress",
                &scoped(
                    Some("map-a"),
                    ipc::ProgressEvent {
                        stage: ipc::ProgressStage::Provider,
                        detail: Some("not a turn event".to_string()),
                        provider: None,
                        model: None,
                    },
                ),
            )
            .unwrap();
        recorder
            .record("agent_event", &scoped(None, agent_event("unscoped")))
            .unwrap();
        recorder
            .record(
                "agent_event",
                &scoped(Some("map-other"), agent_event("other request")),
            )
            .unwrap();
        recorder
            .record(
                "answer",
                &scoped(
                    Some("map-a"),
                    ipc::AnswerEvent {
                        text: "done".to_string(),
                    },
                ),
            )
            .unwrap();

        let run = recorder.snapshot().unwrap();
        assert_eq!(run.request_id, "map-a");
        assert_eq!(run.candidate_revision, "r0:aaa");
        assert!(run.in_flight);
        assert!(!run.truncated);
        assert_eq!(
            run.events
                .iter()
                .map(|event| event.name.as_str())
                .collect::<Vec<_>>(),
            ["agent_event", "answer"]
        );
        // The payload is the scoped event the panel receives.
        assert_eq!(run.events[0].payload["sessionId"], "map-1");
        assert_eq!(run.events[0].payload["requestId"], "map-a");
        assert_eq!(run.events[0].payload["candidateRevision"], "r0:aaa");
        assert_eq!(run.events[0].payload["detail"], "terrain.set");
        assert_eq!(run.events[1].payload["text"], "done");
        let serialized = serde_json::to_value(&run).unwrap();
        assert_eq!(serialized["requestId"], "map-a");
        assert_eq!(serialized["inFlight"], true);
        assert_eq!(serialized["events"][1]["name"], "answer");

        recorder.finish("map-a");
        recorder
            .record(
                "agent_event",
                &scoped(Some("map-a"), agent_event("after the run")),
            )
            .unwrap();
        let ended = recorder.snapshot().unwrap();
        assert!(!ended.in_flight, "finishing the request ends the run");
        assert_eq!(
            ended.events.len(),
            2,
            "events after the run are not recorded"
        );
        assert_eq!(
            ended.prompt.text, run.prompt.text,
            "the ended run stays readable"
        );

        recorder.start(
            "map-b".to_string(),
            "r0:aaa".to_string(),
            prompt(MapRunOrigin::User),
        );
        let fresh = recorder.snapshot().unwrap();
        assert_eq!(fresh.request_id, "map-b");
        assert!(fresh.in_flight);
        assert!(
            fresh.events.is_empty(),
            "a new request starts a fresh transcript"
        );
        assert_eq!(fresh.prompt.origin, MapRunOrigin::User);
        assert_eq!(serde_json::to_value(fresh.prompt.origin).unwrap(), "user");
    }

    #[test]
    fn finishing_another_request_keeps_the_current_run_in_flight() {
        let recorder = started(MapRunOrigin::User);
        recorder.finish("map-stale");
        recorder
            .record(
                "agent_event",
                &scoped(Some("map-a"), agent_event("still recorded")),
            )
            .unwrap();
        let run = recorder.snapshot().unwrap();
        assert!(run.in_flight);
        assert_eq!(run.events.len(), 1);
    }

    #[test]
    fn transcript_drops_the_oldest_events_beyond_its_bounds() {
        let recorder = started(MapRunOrigin::User);
        for index in 0..(MAP_RUN_MAX_EVENTS + 5) {
            recorder
                .record(
                    "agent_event",
                    &scoped(Some("map-a"), agent_event(&format!("event {index}"))),
                )
                .unwrap();
        }
        let run = recorder.snapshot().unwrap();
        assert!(run.truncated);
        assert_eq!(run.events.len(), MAP_RUN_MAX_EVENTS);
        assert_eq!(run.events[0].payload["detail"], "event 5");
        assert_eq!(
            run.events[MAP_RUN_MAX_EVENTS - 1].payload["detail"],
            format!("event {}", MAP_RUN_MAX_EVENTS + 4)
        );

        // Byte bound: three 1.5 MiB events keep only the newest two.
        let recorder = started(MapRunOrigin::User);
        let large = "x".repeat(3 * MAP_RUN_MAX_BYTES / 8);
        for index in 0..3 {
            recorder
                .record(
                    "agent_event",
                    &scoped(Some("map-a"), agent_event(&format!("{index}{large}"))),
                )
                .unwrap();
        }
        let run = recorder.snapshot().unwrap();
        assert!(run.truncated);
        assert_eq!(run.events.len(), 2);
        assert!(run.events[0].payload["detail"]
            .as_str()
            .unwrap()
            .starts_with('1'));
        assert!(run.events[1].payload["detail"]
            .as_str()
            .unwrap()
            .starts_with('2'));
    }

    #[test]
    fn the_run_header_carries_the_prompt_and_no_events() {
        let header = serde_json::to_value(MapRunStartedEvent {
            prompt: prompt(MapRunOrigin::Team),
        })
        .unwrap();
        assert_eq!(header["prompt"]["origin"], "team");
        assert_eq!(header["prompt"]["text"], "Z1 테두리를 마을 패턴으로");
        assert_eq!(header["prompt"]["teamTaskId"], "task-1");
        assert_eq!(header["prompt"]["parentSessionName"], "메인");
        assert!(header.get("events").is_none());
        let scoped_header = serde_json::to_value(scoped(
            Some("map-a"),
            MapRunStartedEvent {
                prompt: prompt(MapRunOrigin::User),
            },
        ))
        .unwrap();
        assert_eq!(scoped_header["sessionId"], "map-1");
        assert_eq!(scoped_header["requestId"], "map-a");
        assert_eq!(scoped_header["candidateRevision"], "r0:aaa");
        assert_eq!(scoped_header["prompt"]["origin"], "user");
        assert!(scoped_header["prompt"].get("teamTaskId").is_none());
        // The header is not itself part of the transcript.
        let recorder = started(MapRunOrigin::User);
        recorder
            .record(
                "map_run_started",
                &scoped(
                    Some("map-a"),
                    MapRunStartedEvent {
                        prompt: prompt(MapRunOrigin::User),
                    },
                ),
            )
            .unwrap();
        assert!(recorder.snapshot().unwrap().events.is_empty());
    }

    #[tokio::test]
    async fn a_busy_engine_reports_no_resume_error() {
        let engine = tokio::sync::Mutex::new(Some("resume failed".to_string()));
        assert_eq!(
            idle_resume_error(&engine, |error| error.clone()),
            Some("resume failed".to_string())
        );
        let running_turn = engine.lock().await;
        assert_eq!(
            idle_resume_error(&engine, |error| error.clone()),
            None,
            "a running turn proves the conversation is alive"
        );
        drop(running_turn);
        assert!(idle_resume_error(&engine, |error| error.clone()).is_some());
    }
}

pub(crate) struct SessionWorker {
    engine:
        tokio::sync::Mutex<AgentEngine<crate::provider_runtime::ProviderRuntime, SessionEventSink>>,
    provider: crate::provider::ProviderId,
    cancellation: tokio::sync::watch::Sender<u64>,
    autonomous_pause_requested: Arc<AtomicBool>,
    runtime: SessionToolRuntime,
    sink: SessionEventSink,
}

/// The engine's conversation resume error when it is idle; `None` while a
/// turn holds the engine, because a running turn is a resumed conversation
/// and a window bootstrap must never wait for it.
fn idle_resume_error<E>(
    engine: &tokio::sync::Mutex<E>,
    resume_error: impl FnOnce(&E) -> Option<String>,
) -> Option<String> {
    match engine.try_lock() {
        Ok(engine) => resume_error(&engine),
        Err(_) => None,
    }
}

fn cancel_worker_generation(
    cancellation: &tokio::sync::watch::Sender<u64>,
) -> Result<(), AgentEngineError> {
    let generation = (*cancellation.borrow())
        .checked_add(1)
        .ok_or_else(|| AgentEngineError::new("turn cancellation generation overflow"))?;
    cancellation.send_replace(generation);
    Ok(())
}

type ProjectRecoveryResult = Result<HashMap<String, String>, String>;

/// The session engines and review recovery of ONE
/// project. Its `ProjectHandle` owns it, so closing a project drops them.
#[derive(Default)]
pub(crate) struct ProjectEngineState {
    workers: tokio::sync::Mutex<HashMap<String, Arc<SessionWorker>>>,
}

/// A window with no project was closed while a project was being opened.
const OPENING_ELSEWHERE: &str = "다른 창에서 프로젝트를 여는 중입니다.";

/// Why a project is being closed: its window switches to another project,
/// or the user closes its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseKind {
    Switch,
    Window,
}

impl CloseKind {
    fn busy(self) -> &'static str {
        match self {
            Self::Switch => "진행 중인 작업이나 검토가 있습니다. 작업을 완료하거나 취소한 뒤 프로젝트를 전환해 주세요.",
            Self::Window => "진행 중인 작업이나 검토 대기 중인 계획이 있습니다.",
        }
    }

    fn building(self) -> &'static str {
        match self {
            Self::Switch => "빌드가 진행 중입니다. 빌드가 끝난 뒤 프로젝트를 전환해 주세요.",
            Self::Window => "빌드가 진행 중입니다.",
        }
    }
}

/// End project `id`: its write registrations and path claims are released
/// (restored when it is opened again) and it leaves the registry with its
/// window bindings.
fn release_and_close(
    registry: &crate::project_registry::ProjectRegistry,
    writes: &crate::write_coordinator::ProjectWriteCoordinator,
    id: &crate::project_id::ProjectId,
) {
    writes.release_project(&[
        crate::write_coordinator::TxnKey::eps(id),
        crate::write_coordinator::TxnKey::map(id),
    ]);
    registry.close(id);
}

/// Close `current` (unless it is `project` itself) and register `project`,
/// binding `window` to it: the closing project's write registrations, path
/// claims, and window bindings end with it, and are restored when it is
/// opened again. Registering the window's own project again is that open
/// project; a project open in another window is refused.
pub(crate) fn commit_switch(
    registry: &crate::project_registry::ProjectRegistry,
    writes: &crate::write_coordinator::ProjectWriteCoordinator,
    current: Option<&crate::project_registry::ProjectHandle>,
    project: &crate::native_project::NativeProject,
    window: &str,
) -> Result<Arc<crate::project_registry::ProjectHandle>, String> {
    let id = crate::project_id::ProjectId::of(project);
    // A project open in another window stays there; this window never
    // shares it.
    if registry.get(&id).is_some() && current.map(|current| current.id()) != Some(&id) {
        return Err(crate::project_launch::ALREADY_OPEN_ELSEWHERE.to_string());
    }
    if let Some(current) = current.filter(|current| current.id() != &id) {
        release_and_close(registry, writes, current.id());
    }
    let handle = match registry.register(project) {
        Ok(handle) | Err(crate::project_registry::RegisterError::AlreadyOpen(handle)) => handle,
        Err(error) => return Err(error.to_string()),
    };
    registry.bind_window(window, handle.id());
    Ok(handle)
}

/// Recover `handle`'s project once: bind the stored sessions it owns (moved,
/// legacy, Map) and restore their pending reviews under its keys. Runs as
/// admitted work, so a project being closed is never recovered into.
pub(crate) fn recover_project(
    handle: &crate::project_registry::ProjectHandle,
    binder: &crate::session_binding::SessionBinder,
    sessions: &crate::session::SessionStore,
    dirs: &crate::config::DataDirs,
    writes: &crate::write_coordinator::ProjectWriteCoordinator,
    session_id: Option<&str>,
) -> Result<(), String> {
    let _admitted = handle.admit()?;
    let mut recovered = handle.recovery().lock();
    let result = recovered.get_or_insert_with(|| {
        let open = crate::session_binding::OpenProject::open(dirs, handle.open_project()?);
        binder.bind_open_project_sessions(&open)?;
        restore_pending_review(sessions, dirs, writes, handle.id())
    });
    match result {
        Err(error) => Err(error.clone()),
        Ok(session_errors) => session_id
            .and_then(|id| session_errors.get(id))
            .cloned()
            .map_or(Ok(()), Err),
    }
}

/// The sessions of `sessions` that belong to `handle`'s project: bound to its
/// id, or written before ids and naming it while no other open project has
/// that name.
pub(crate) fn sessions_of_project(
    sessions: Vec<crate::session::SessionMeta>,
    handle: &crate::project_registry::ProjectHandle,
    registry: &crate::project_registry::ProjectRegistry,
) -> Vec<crate::session::SessionMeta> {
    sessions
        .into_iter()
        .filter(|meta| {
            crate::session_binding::open_project_of_meta(meta, registry)
                .is_ok_and(|owner| owner.id() == handle.id())
        })
        .collect()
}

/// Whether the project of the session `meta` describes is open. A dispatcher
/// continuing a session of a closed project does nothing and leaves the
/// settlement unreported.
pub(crate) fn project_is_open(
    meta: &crate::session::SessionMeta,
    registry: &crate::project_registry::ProjectRegistry,
) -> bool {
    crate::session_binding::open_project_of_meta(meta, registry).is_ok()
}

/// The engine state of `handle`'s project.
fn engine_state(handle: &crate::project_registry::ProjectHandle) -> &ProjectEngineState {
    handle.runtime::<ProjectEngineState>()
}

impl crate::project_registry::ProjectRuntime for ProjectEngineState {
    /// Drop what the project owns; close admission has made sure nothing
    /// runs.
    fn clear(&self) {
        if let Ok(mut workers) = self.workers.try_lock() {
            workers.clear();
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone)]
pub(crate) struct SessionEngineManager {
    inner: Arc<SessionEngineManagerInner>,
}

struct SessionEngineManagerInner {
    /// The open projects; each owns its sessions' engines.
    registry: crate::project_registry::ProjectRegistry,
    sessions: crate::session::SessionStore,
    attachments: AttachmentStore,
    services: crate::tool_exec::ToolServices,
    provider_service: crate::provider_service::ProviderService,
    config: AgentEngineConfig,
    app: tauri::AppHandle,
    dirs: crate::config::DataDirs,
    fallback_cwd: PathBuf,
    /// Binds stored sessions to their project before an engine uses them.
    binder: crate::session_binding::SessionBinder,
    /// Team Map requests run on a dispatcher spawned with the manager; see
    /// `engine::team` for why the per-session executor only sends to it.
    team_dispatcher: team::TeamDispatcher,
}

pub(crate) fn restore_pending_review(
    sessions: &crate::session::SessionStore,
    dirs: &crate::config::DataDirs,
    writes: &crate::write_coordinator::ProjectWriteCoordinator,
    project: &crate::project_id::ProjectId,
) -> ProjectRecoveryResult {
    let mut pending = Vec::new();
    let mut session_errors = HashMap::new();
    let key = crate::write_coordinator::TxnKey::eps(project);
    for meta in sessions
        .list()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|meta| meta.project_id.as_deref() == Some(project.as_str()))
        .filter(|meta| meta.kind == crate::session::SessionKind::Eps)
    {
        let mut record = match sessions.load(&meta.id) {
            Ok(record) => record,
            Err(error) => {
                session_errors.insert(
                    meta.id,
                    format!("pending review state cannot be loaded: {error}"),
                );
                continue;
            }
        };
        let pending_before = record.pending_request_ids.len();
        record.pending_request_ids.retain(|request_id| {
            let live_journal_missing = matches!(
                journal::JournalStore::load(dirs.app_data(), request_id),
                Err(journal::JournalError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound
            );
            !(live_journal_missing
                && journal::JournalStore::archived_exists(dirs.app_data(), request_id))
        });
        if record.pending_request_ids.len() != pending_before {
            if let Err(error) = sessions.save(&record) {
                session_errors.insert(
                    record.meta.id,
                    format!("pending review state cannot be repaired: {error}"),
                );
                continue;
            }
        }
        for request_id in record.pending_request_ids {
            pending.push((record.meta.id.clone(), request_id));
        }
    }
    for (session_id, request_id) in pending {
        let journal = match journal::JournalStore::load(dirs.app_data(), &request_id) {
            Ok(journal) => journal,
            Err(error) => {
                session_errors.entry(session_id).or_insert_with(|| {
                    format!("pending review `{request_id}` cannot be recovered: {error}")
                });
                continue;
            }
        };
        if journal.entries.is_empty() {
            session_errors.entry(session_id).or_insert_with(|| {
                format!("pending review `{request_id}` has an empty undecided journal")
            });
            continue;
        }
        if let Err(error) = writes.restore_review(&key, &session_id, &request_id) {
            session_errors.entry(session_id).or_insert(error);
        }
    }
    Ok(session_errors)
}

fn rewind_unrecoverable_pending_session(
    sessions: &crate::session::SessionStore,
    dirs: &crate::config::DataDirs,
    session_id: &str,
    expected_project: &str,
    panel_log: serde_json::Value,
) -> Result<Option<String>, String> {
    let record = sessions
        .load(session_id)
        .map_err(|error| error.to_string())?;
    if record.pending_request_ids.is_empty()
        || (!expected_project.is_empty() && record.meta.project != expected_project)
    {
        return Ok(None);
    }
    for request_id in &record.pending_request_ids {
        match journal::JournalStore::load(dirs.app_data(), request_id) {
            Ok(journal) if !journal.entries.is_empty() => return Ok(None),
            Ok(_) => {}
            Err(journal::JournalError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "pending review `{request_id}` cannot be discarded by rewind: {error}"
                ));
            }
        }
    }

    sessions
        .move_task_leaf_for_rewind(session_id, panel_log)
        .map_err(|error| error.to_string())?;
    Ok(Some(record.meta.project_key().to_string()))
}

impl SessionEngineManager {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        sessions: crate::session::SessionStore,
        attachments: AttachmentStore,
        services: crate::tool_exec::ToolServices,
        provider_service: crate::provider_service::ProviderService,
        config: AgentEngineConfig,
        app: tauri::AppHandle,
        dirs: crate::config::DataDirs,
        fallback_cwd: PathBuf,
        registry: crate::project_registry::ProjectRegistry,
    ) -> Self {
        let (team_dispatcher, team_commands) = tokio::sync::mpsc::unbounded_channel();
        let binder = crate::session_binding::SessionBinder::new(sessions.clone(), dirs.clone());
        let manager = Self {
            inner: Arc::new(SessionEngineManagerInner {
                registry,
                sessions,
                attachments,
                services,
                provider_service,
                config,
                app,
                dirs: dirs.clone(),
                fallback_cwd,
                binder,
                team_dispatcher,
            }),
        };
        tauri::async_runtime::spawn(team::dispatch(team_commands, manager.clone()));
        manager
    }

    /// Open a project in window `window`, in place of the one it shows.
    /// `open` validates and activates the new project (migrating a legacy
    /// authority first) and returns it; the window's project is closed only
    /// once that succeeded, so a failed open leaves it open and untouched.
    /// Admission refuses while that project has any work: an engine running
    /// or holding a write, a Map or import window, or a build.
    /// Called off the async runtime, after any native picker has closed.
    ///
    /// `target` is the folder or manifest the user chose, when there is one:
    /// a project open in another window is refused before anything runs, and
    /// that window is focused instead.
    ///
    /// `open` returns `None` when it opened nothing (an import waiting for
    /// the user's approval); the window's project then stays.
    pub(crate) fn switch_project<T>(
        &self,
        window: &str,
        target: Option<&std::path::Path>,
        open: impl FnOnce() -> Result<(Option<crate::native_project::NativeProject>, T), String>,
    ) -> Result<T, String> {
        // One open, switch, or close at a time: a second one (a double
        // click, a file launch racing the launcher) waits for nothing and is
        // refused.
        let _switching = self
            .inner
            .registry
            .try_begin_switch()
            .ok_or_else(|| CloseKind::Switch.busy().to_string())?;
        let current = self.inner.registry.for_window(window).ok();
        if let Some(id) = target.and_then(crate::native_runtime::project_id_at) {
            let here = current.as_ref().is_some_and(|current| current.id() == &id);
            if !here && self.inner.registry.get(&id).is_some() {
                crate::project_launch::focus_window(
                    &self.inner.app,
                    &crate::project_launch::main_window_of(&self.inner.registry, &id),
                );
                return Err(crate::project_launch::ALREADY_OPEN_ELSEWHERE.to_string());
            }
        }
        let Some(current) = current else {
            let (project, value) = open()?;
            if let Some(project) = project {
                let handle = commit_switch(
                    &self.inner.registry,
                    self.inner.services.writes(),
                    None,
                    &project,
                    window,
                )?;
                self.warm_mentions(&handle);
            }
            return Ok(value);
        };
        self.close_admitted(
            &current,
            CloseKind::Switch,
            || {
                let (project, value) = open()?;
                Ok(match project {
                    Some(project)
                        if &crate::project_id::ProjectId::of(&project) != current.id() =>
                    {
                        Ok((project, value))
                    }
                    // Nothing opened, or the window's project again: nothing
                    // to close or register.
                    _ => Err(value),
                })
            },
            |(project, value)| {
                let handle = commit_switch(
                    &self.inner.registry,
                    self.inner.services.writes(),
                    Some(&current),
                    &project,
                    window,
                )?;
                self.warm_mentions(&handle);
                Ok(value)
            },
        )
    }

    /// The root of the project window `window` shows.
    pub(crate) fn window_project_root(&self, window: &str) -> Option<String> {
        self.inner
            .registry
            .for_window(window)
            .ok()
            .map(|handle| handle.root().to_string_lossy().into_owned())
    }

    /// Close the project of window `window`, which the user is closing.
    /// Admission is the switch's, except that the project's Map and import
    /// windows close with it. Returns the closed project and the windows it
    /// had; `None` when the window showed no project.
    pub(crate) fn close_window(
        &self,
        window: &str,
    ) -> Result<Option<(crate::project_id::ProjectId, Vec<String>)>, String> {
        // An open or switch elsewhere never blocks this close: a bound
        // window's own switch holds its close gate, which admission tries.
        // Only a window with no project yet may be the one opening one.
        let switching = self.inner.registry.try_begin_switch();
        let Ok(current) = self.inner.registry.for_window(window) else {
            return match switching {
                Some(_) => Ok(None),
                None => Err(OPENING_ELSEWHERE.to_string()),
            };
        };
        drop(switching);
        let id = current.id().clone();
        let labels = self.inner.registry.window_labels(&id);
        self.close_admitted(
            &current,
            CloseKind::Window,
            || Ok(Ok(())),
            |()| {
                release_and_close(&self.inner.registry, self.inner.services.writes(), &id);
                Ok(())
            },
        )?;
        Ok(Some((id, labels)))
    }

    /// Close project `id` without admission, because its window is already
    /// gone. Its engines stop being reachable; nothing is waited for.
    pub(crate) fn drop_project(&self, id: &crate::project_id::ProjectId) {
        if let Some(handle) = self.inner.registry.get(id) {
            if let Ok(mut workers) = engine_state(&handle).workers.try_lock() {
                workers.clear();
            }
        }
        release_and_close(&self.inner.registry, self.inner.services.writes(), id);
    }

    /// Close `window`'s project although it has work, because the user
    /// confirmed it: every session's turn and background worker is
    /// cancelled, a pending question is withdrawn, and the project closes
    /// without waiting for any of them. Durable sessions, pending plans and
    /// reviews stay and return when the project is opened again. `None` when
    /// the window has no project.
    pub(crate) fn force_close_window(
        &self,
        window: &str,
    ) -> Option<(crate::project_id::ProjectId, Vec<String>)> {
        let current = self.inner.registry.for_window(window).ok()?;
        let id = current.id().clone();
        let labels = self.inner.registry.window_labels(&id);
        if let Ok(workers) = engine_state(&current).workers.try_lock() {
            for (session_id, worker) in workers.iter() {
                worker.runtime.cancel_pending_ask();
                self.cancel_workers_of(session_id);
                if let Err(error) = cancel_worker_generation(&worker.cancellation) {
                    eprintln!(
                        "eud-agent: session {session_id} could not be cancelled on close: {error}"
                    );
                }
            }
        }
        self.drop_project(&id);
        Some((id, labels))
    }

    /// Run `before_close` and then `close` while nothing of `current` can
    /// start: its close gate is held for writing and every engine is idle,
    /// with no write, build, or (for a switch) Map or import
    /// window. `before_close` returning `Err(value)` keeps the project open;
    /// otherwise the project's idle engines are dropped before `close` runs.
    fn close_admitted<T, R>(
        &self,
        current: &crate::project_registry::ProjectHandle,
        kind: CloseKind,
        before_close: impl FnOnce() -> Result<Result<T, R>, String>,
        close: impl FnOnce(T) -> Result<R, String>,
    ) -> Result<R, String> {
        let busy = || kind.busy().to_string();
        let _closing = current.close_gate().try_write().map_err(|_| busy())?;
        // A close that won the gate first (the window closing while its
        // switch began) leaves nothing to switch from.
        if current.is_closed() {
            return Err(busy());
        }
        let mut workers = engine_state(current)
            .workers
            .try_lock()
            .map_err(|_| busy())?;
        if kind == CloseKind::Switch
            && self
                .inner
                .registry
                .window_labels(current.id())
                .iter()
                .filter(|label| {
                    label.starts_with(crate::project_registry::MAP_WINDOW_PREFIX)
                        || label.starts_with(crate::project_registry::IMPORT_WINDOW_PREFIX)
                })
                .any(|label| tauri::Manager::get_webview_window(&self.inner.app, label).is_some())
        {
            return Err(
                "맵 작업을 저장하고 Map Agent와 맵 가져오기 창을 닫은 뒤 프로젝트를 전환해 주세요."
                    .to_string(),
            );
        }
        if current.is_building() {
            return Err(kind.building().to_string());
        }
        let mut idle = Vec::with_capacity(workers.len());
        for worker in workers.values() {
            // A cloned handle includes commands waiting to acquire the engine lock.
            if Arc::strong_count(worker) != 1 {
                return Err(busy());
            }
            let engine = worker.engine.try_lock().map_err(|_| busy())?;
            if engine.phase != Phase::Idle || worker.runtime.write_ticket().is_some() {
                return Err(busy());
            }
            idle.push(engine);
        }
        let value = match before_close()? {
            Ok(value) => value,
            Err(kept) => return Ok(kept),
        };
        drop(idle);
        // Durable sessions stay; their engines rehydrate when the project is
        // opened again.
        workers.clear();
        drop(workers);
        close(value)
    }

    /// Load the newly opened project's saved map for `@` mention search in
    /// the background, so the first search does not wait for it.
    fn warm_mentions(&self, handle: &crate::project_registry::ProjectHandle) {
        let Ok(context) = handle.map_context() else {
            return;
        };
        let mentions = self.inner.services.mentions();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = mentions.warmup(&context) {
                eprintln!("eud-agent: mention context warmup skipped: {error}");
            }
        });
    }

    /// Run one git mutation the user asked for (a history revert) inside the
    /// same project transaction EPS turns commit under, so it never races a
    /// turn commit on the index lock.
    pub(crate) fn git_transaction<T>(
        &self,
        project: &crate::project_id::ProjectId,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.inner
            .services
            .writes()
            .transaction(&crate::write_coordinator::TxnKey::git(project), operation)?
    }

    /// Run a memory or wiki save the user made under the open project's
    /// `eps:` key, which EPS tool writes also take.
    pub(crate) async fn direct_project_write<T>(
        &self,
        project: &crate::project_id::ProjectId,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.inner
            .services
            .writes()
            .transaction(&crate::write_coordinator::TxnKey::eps(project), operation)?
    }

    /// Restore the pending reviews of `project` once. For the open project,
    /// first bind what belongs to it: sessions whose folder moved here,
    /// legacy sessions that name it, and Map sessions without an id.
    fn ensure_project_recovery(
        &self,
        handle: &crate::project_registry::ProjectHandle,
        session_id: Option<&str>,
    ) -> Result<(), String> {
        recover_project(
            handle,
            &self.inner.binder,
            &self.inner.sessions,
            &self.inner.dirs,
            self.inner.services.writes(),
            session_id,
        )
    }

    fn clear_session_recovery_error(&self, project_id: &str, session_id: &str) {
        let Some(handle) = self
            .inner
            .registry
            .get(&crate::project_id::ProjectId::from_stored(project_id))
        else {
            return;
        };
        if let Some(Ok(session_errors)) = handle.recovery().lock().as_mut() {
            session_errors.remove(session_id);
        };
    }

    /// Recover the open project. A project that is not open is recovered
    /// when it is opened: its sessions are bound by id, and binding a legacy
    /// session needs that project's live manifest.
    fn recover_all_projects(&self) -> Result<(), String> {
        for handle in self.inner.registry.handles() {
            self.ensure_project_recovery(&handle, None)?;
        }
        Ok(())
    }

    /// `handle`'s project as sessions are bound against it.
    fn open_project_of(
        &self,
        handle: &crate::project_registry::ProjectHandle,
    ) -> Result<crate::session_binding::OpenProject, String> {
        Ok(crate::session_binding::OpenProject::open(
            &self.inner.dirs,
            handle.open_project()?,
        ))
    }

    /// The open project `record` belongs to, with `record` bound to it.
    fn resolve_session_project(
        &self,
        record: crate::session::SessionRecord,
    ) -> Result<
        (
            crate::session::SessionRecord,
            Arc<crate::project_registry::ProjectHandle>,
        ),
        String,
    > {
        self.inner.binder.resolve(record, &self.inner.registry)
    }

    /// The open project of a stored session, for commands that need its
    /// project without an engine.
    fn session_handle(
        &self,
        session_id: &str,
    ) -> Result<Arc<crate::project_registry::ProjectHandle>, AgentEngineError> {
        let record = self
            .inner
            .sessions
            .load(session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        crate::session_binding::open_project_of_meta(&record.meta, &self.inner.registry)
            .map_err(AgentEngineError::new)
    }

    /// The open project a stored session belongs to, read from the session
    /// index; `None` when that project is not open.
    pub(crate) fn session_project_id(
        &self,
        session_id: &str,
    ) -> Option<crate::project_id::ProjectId> {
        let meta = self
            .inner
            .sessions
            .list()
            .ok()?
            .into_iter()
            .find(|meta| meta.id == session_id)?;
        crate::session_binding::open_project_of_meta(&meta, &self.inner.registry)
            .ok()
            .map(|handle| handle.id().clone())
    }

    /// The engine of `session_id`, whichever open project holds it.
    async fn registered_worker(&self, session_id: &str) -> Option<Arc<SessionWorker>> {
        for handle in self.inner.registry.handles() {
            if let Some(worker) = engine_state(&handle)
                .workers
                .lock()
                .await
                .get(session_id)
                .cloned()
            {
                return Some(worker);
            }
        }
        None
    }

    /// Drop the engine of `session_id`; the next command rebuilds it.
    async fn unregister_worker(&self, session_id: &str) {
        for handle in self.inner.registry.handles() {
            engine_state(&handle)
                .workers
                .lock()
                .await
                .remove(session_id);
        }
    }

    async fn worker(&self, session_id: &str) -> Result<Arc<SessionWorker>, AgentEngineError> {
        if let Some(worker) = self.registered_worker(session_id).await {
            return Ok(worker);
        }
        let record = self
            .inner
            .sessions
            .load(session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        // Binding and recovery read the filesystem (and may examine an old
        // folder on a slow share), so they run off the async runtime.
        let manager = self.clone();
        let (record, handle) = tokio::task::spawn_blocking(move || {
            let (record, handle) = manager.resolve_session_project(record)?;
            if record.meta.kind == crate::session::SessionKind::Eps {
                manager.ensure_project_recovery(&handle, Some(&record.meta.id))?;
            }
            Ok::<_, String>((record, handle))
        })
        .await
        .map_err(|error| AgentEngineError::new(error.to_string()))?
        .map_err(AgentEngineError::new)?;
        // Keep worker creation inside the project's close admission,
        // including its async provider setup and initial hydration.
        let _admitted = handle.close_gate().read().await;
        handle.ensure_open().map_err(AgentEngineError::new)?;
        let mut workers = engine_state(&handle).workers.lock().await;
        if let Some(worker) = workers.get(session_id).cloned() {
            return Ok(worker);
        }
        let services = self
            .inner
            .services
            .for_project(&handle)
            .map_err(AgentEngineError::new)?;
        let runtime = if record.meta.kind == crate::session::SessionKind::Map {
            services.map_session(session_id.to_string())
        } else {
            services.session(session_id.to_string())
        };
        runtime.set_provider_identity(
            record.provider_binding.provider,
            record.provider_binding.model.clone(),
        );
        if record.meta.kind == crate::session::SessionKind::Map
            || (record.meta.kind == crate::session::SessionKind::Eps
                && record.meta.worker_parent.is_none())
        {
            let progress_sessions = self.inner.sessions.clone();
            let progress_runtime = runtime.clone();
            let progress_kind = record.meta.kind;
            runtime.set_workflow_progress_executor(move |invocation| {
                if invocation.session_id != progress_runtime.session_id()
                    || progress_runtime.current_request_id().as_deref()
                        != Some(invocation.request_id.as_str())
                {
                    return Err("stale provider run cannot update task progress".to_string());
                }
                let current = progress_sessions
                    .load(&invocation.session_id)
                    .map_err(|error| error.to_string())?;
                if progress_kind == crate::session::SessionKind::Map {
                    if current.task_state.projection.revision
                        != invocation.request.expected_task_revision
                    {
                        return Err(format!(
                            "stale task state revision: expected {}, current {}",
                            invocation.request.expected_task_revision,
                            current.task_state.projection.revision
                        ));
                    }
                    return Ok(serde_json::json!({
                        "ok": true,
                        "staged": true,
                        "taskRevision": current.task_state.projection.revision,
                    }));
                }
                let project_revision = progress_runtime.current_project_revision()?;
                let mut completed_evidence = Vec::new();
                let entry_ids = progress_runtime
                    .journal()
                    .selected_entries(&invocation.request_id, &crate::journal::DecisionIds::All)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|entry| entry.id)
                    .collect::<Vec<_>>();
                if !entry_ids.is_empty() {
                    completed_evidence.push(
                        crate::task_state::WorkflowCompletedEvidence::AcceptedJournal {
                            project_revision: project_revision.clone(),
                            request_id: invocation.request_id.clone(),
                            entry_ids,
                        },
                    );
                }
                if progress_runtime
                    .last_build_evidence()
                    .is_some_and(|build| build.ok && build.error_count == 0)
                {
                    completed_evidence.push(
                        crate::task_state::WorkflowCompletedEvidence::SuccessfulBuild {
                            project_revision: project_revision.clone(),
                            request_id: invocation.request_id.clone(),
                        },
                    );
                }
                let provenance = current
                    .task_state
                    .projection
                    .workflow
                    .provenance
                    .unwrap_or_else(crate::task_state::WorkflowProvenance::local);
                let state = progress_sessions
                    .replace_task_workflow_progress(
                        &invocation.session_id,
                        crate::task_state::WorkflowProgressUpdate {
                            expected_task_revision: invocation.request.expected_task_revision,
                            client_turn_id: Some(invocation.client_turn_id),
                            request_id: Some(invocation.request_id),
                            progress: crate::task_state::WorkflowProgress {
                                mode: invocation.request.mode,
                                phase: invocation.request.phase,
                                last_completed: invocation.request.last_completed,
                                prerequisites: invocation.request.prerequisites,
                                next_action: invocation.request.next_action,
                                pending_playtests: invocation.request.pending_playtests,
                            },
                            completed_evidence,
                            provenance,
                        },
                    )
                    .map_err(|error| error.to_string())?;
                if let Ok(workspace) = progress_runtime
                    .workspace_manager()
                    .prepare_session_current(&invocation.session_id)
                {
                    let record = progress_sessions
                        .load(&invocation.session_id)
                        .map_err(|error| error.to_string())?;
                    crate::work_checkpoint::refresh_session(
                        &workspace.root,
                        &workspace.id,
                        &record,
                    )
                    .map_err(|error| error.to_string())?;
                }
                Ok(serde_json::json!({
                    "ok": true,
                    "taskRevision": state.projection.revision,
                    "projectRevision": project_revision,
                }))
            });
        }
        let sink = SessionEventSink::for_project(
            self.inner.app.clone(),
            session_id.to_string(),
            Some(handle.id().clone()),
        );
        let ask_sink = sink.clone();
        runtime.set_ask_emitter(move |event| {
            ask_sink
                .emit_scoped("ask", event)
                .map_err(|error| format!("failed to emit ask event: {error}"))
        });
        let progress_sink = sink.clone();
        runtime.set_progress_emitter(move |event| {
            progress_sink
                .emit_scoped("progress", event)
                .map_err(|error| format!("failed to emit progress event: {error}"))
        });
        let autonomous_sink = sink.clone();
        runtime.set_autonomous_emitter(move |event| {
            autonomous_sink
                .emit_scoped("autonomous_run", event)
                .map_err(|error| format!("failed to emit autonomous run event: {error}"))
        });
        let (cancellation, cancellation_rx) = tokio::sync::watch::channel(0_u64);
        runtime.set_cancellation(cancellation_rx.clone());
        let binding = BindingSnapshot::from_binding(&record.provider_binding, None)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        if record.meta.kind == crate::session::SessionKind::Eps {
            let team = team::TeamHandoffContext {
                app: self.inner.app.clone(),
                dispatcher: self.inner.team_dispatcher.clone(),
                sessions: self.inner.sessions.clone(),
                session_id: session_id.to_string(),
                cancellation: cancellation_rx.clone(),
            };
            runtime.set_team_executor(move |request| Box::pin(team.clone().run(request)));
            let app = self.inner.app.clone();
            runtime.set_team_action_executor(move |action| team::run_action(&app, action));
            runtime.set_worker_parent(record.meta.worker_parent.clone());
            if let Some(parent_id) = &record.meta.worker_parent {
                let parent = self.inner.sessions.load(parent_id).map_err(|error| {
                    AgentEngineError::new(format!("worker parent could not be loaded: {error}"))
                })?;
                if let Some(worker) = parent
                    .workers
                    .iter()
                    .find(|worker| worker.child_session_id == session_id)
                {
                    runtime.set_worker_profile(worker.profile);
                }
            }
            if record.meta.worker_parent.is_none() {
                let workers = worker::WorkerContext {
                    app: self.inner.app.clone(),
                    dispatcher: self.inner.team_dispatcher.clone(),
                    sessions: self.inner.sessions.clone(),
                    session_id: session_id.to_string(),
                };
                runtime.set_worker_executor(move |action| workers.execute(action));
            }
        }
        let palette_app = self.inner.app.clone();
        let palette_project = handle.id().clone();
        runtime.set_palette_notifier(move || {
            let _ = crate::project_events::emit_project(
                &palette_app,
                &palette_project,
                crate::map_agent::MAP_SELECTION_PALETTE_EVENT,
                crate::project_events::ProjectSignal::new(&palette_project),
            );
        });
        let adapter = crate::provider_runtime::production_adapter(
            &record.provider_binding,
            &self.inner.dirs,
            self.inner.fallback_cwd.clone(),
        )
        .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let executor = crate::provider_runtime::ProviderRuntime::new(
            adapter,
            binding,
            self.inner.dirs.clone(),
            self.inner.fallback_cwd.clone(),
            runtime.clone(),
            cancellation_rx.clone(),
            Arc::new(
                runtime_events::SessionRuntimeEventSink::new(
                    sink.clone(),
                    self.inner.sessions.clone(),
                    session_id.to_string(),
                )
                .with_path_claims(runtime.clone()),
            ),
        )
        .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let engine = AgentEngine::new(
            executor,
            sink.clone(),
            self.inner.config.for_project(&handle),
            runtime.clone(),
            self.inner.sessions.clone(),
            self.inner.attachments.clone(),
            record,
            cancellation_rx,
        );
        let autonomous_pause_requested = engine.autonomous_pause_handle();
        let worker = Arc::new(SessionWorker {
            provider: engine.provider_binding.provider,
            engine: tokio::sync::Mutex::new(engine),
            cancellation,
            autonomous_pause_requested,
            runtime,
            sink: sink.clone(),
        });

        workers.insert(session_id.to_string(), Arc::clone(&worker));
        worker.engine.lock().await.hydrate().await?;
        Ok(worker)
    }

    async fn execute_granted_write(
        &self,
        worker: Arc<SessionWorker>,
    ) -> Result<(), AgentEngineError> {
        let mut engine = worker.engine.lock().await;
        match engine.continue_pending_write().await {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = engine.recover_write_failure();
                worker
                    .runtime
                    .emit_activity(crate::write_coordinator::SessionActivity::Error);
                let _ = engine.sink.emit(EngineEvent::Error(ipc::ErrorEvent {
                    message: error.message.clone(),
                }));
                engine.update_active_session().await;
                Err(error)
            }
        }
    }

    async fn drive_pending_write(
        &self,
        worker: Arc<SessionWorker>,
    ) -> Result<(), AgentEngineError> {
        let Some(ticket) = worker.runtime.write_ticket() else {
            return Ok(());
        };
        match ticket.state() {
            crate::write_coordinator::TicketState::Granted => {
                self.execute_granted_write(worker).await
            }
            crate::write_coordinator::TicketState::Cancelled => Ok(()),
        }
    }

    /// End one session command, and continue the session for any worker
    /// settlement no turn has shown yet, whether the command succeeded or not:
    /// a settlement that arrived while the session was busy waits for exactly
    /// this point.
    async fn finish_read_command(
        &self,
        worker: &Arc<SessionWorker>,
        result: Result<(), AgentEngineError>,
    ) -> Result<(), AgentEngineError> {
        let finished = self.settle_read_command(worker, result).await;
        self.continue_after_command(worker).await;
        finished
    }

    /// Dispatch the session's next worker continuation after a command, never
    /// after a turn the user cancelled. The cancel flag is read under the
    /// engine lock, waiting for it if another holder (compaction, a rewind,
    /// a cancel) has it: callers hold no lock here, and a skipped dispatch
    /// would lose the continuation. `worker_continue` itself backs off when
    /// the session is busy by then.
    async fn continue_after_command(&self, worker: &Arc<SessionWorker>) {
        let cancelled = worker.engine.lock().await.last_turn_cancelled;
        self.continue_unreported_workers(worker.runtime.session_id(), cancelled);
    }

    async fn settle_read_command(
        &self,
        worker: &Arc<SessionWorker>,
        result: Result<(), AgentEngineError>,
    ) -> Result<(), AgentEngineError> {
        if let Err(error) = result {
            let mut engine = worker.engine.lock().await;
            let cleanup = engine.recover_read_failure();
            worker
                .runtime
                .emit_activity(crate::write_coordinator::SessionActivity::Error);
            let message = match cleanup {
                Ok(()) => error.message.clone(),
                Err(cleanup_error) => format!(
                    "{}; failed to settle write intent: {}",
                    error.message, cleanup_error.message
                ),
            };
            let _ = engine
                .sink
                .emit(EngineEvent::Error(ipc::ErrorEvent { message }));
            engine.update_active_session().await;
            return Err(error);
        }
        // A plan awaiting review may keep the registration of the chat turn
        // that wrote before proposing it; approval, not this command, continues it.
        if worker.runtime.write_ticket().is_some()
            && worker.engine.lock().await.pending_write.is_some()
        {
            return self.drive_pending_write(Arc::clone(worker)).await;
        }
        let phase = worker.engine.lock().await.phase;
        let activity = match phase {
            Phase::PlanReview => crate::write_coordinator::SessionActivity::Review,
            _ => crate::write_coordinator::SessionActivity::Idle,
        };
        worker.runtime.emit_activity(activity);
        Ok(())
    }

    async fn chat(
        &self,
        session_id: &str,
        request: ipc::ChatRequest,
    ) -> Result<(), AgentEngineError> {
        let worker = self.worker(session_id).await?;
        let _provider_busy = self.inner.provider_service.enter_busy(worker.provider);
        if worker.runtime.kind() != crate::session::SessionKind::Eps {
            return Err(AgentEngineError::new(
                "Map sessions accept conversation only through map_agent_chat.",
            ));
        }
        if let Err(error) = self.inner.sessions.touch_conversation(session_id) {
            eprintln!("eud-agent: conversation timestamp update failed: {error}");
        }
        worker
            .runtime
            .emit_activity(crate::write_coordinator::SessionActivity::RunningRead);
        let result = {
            let mut engine = worker.engine.lock().await;
            engine.chat(request).await
        };
        self.finish_read_command(&worker, result).await
    }

    /// Continue an EPS conversation after its team map task settled in the
    /// background (the `map_task_request` had returned `running`): one
    /// ordinary interactive turn on the fixed continuation text, exactly as if
    /// the user had pressed "이어서 진행". The turn starts only once the
    /// session is idle; an autonomous run, a pending review, or a task the
    /// user already moved past leaves the settlement to `[map tasks]` on the
    /// session's own next turn instead.
    pub(crate) async fn team_continue(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<(), AgentEngineError> {
        let record = self
            .inner
            .sessions
            .load(session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        if record.meta.kind != crate::session::SessionKind::Eps {
            return Err(AgentEngineError::new(
                "only an EPS session continues after a team map task",
            ));
        }
        // A session of a closed project does not continue; the settlement
        // reaches it through `[map tasks]` once the project is open again.
        if !project_is_open(&record.meta, &self.inner.registry) {
            return Ok(());
        }
        if record
            .autonomous_run
            .as_ref()
            .is_some_and(|run| !run.status.is_terminal())
        {
            return Ok(());
        }
        let worker = self.worker(session_id).await?;
        let _provider_busy = self.inner.provider_service.enter_busy(worker.provider);
        // Awaiting the engine waits out the turn that received `running`.
        let mut engine = worker.engine.lock().await;
        if engine.phase != Phase::Idle {
            eprintln!(
                "eud-agent: team map task {task_id} settled while session {session_id} is not idle; its next turn reports it"
            );
            return Ok(());
        }
        // The task the settler announced must still be the one to act on: the
        // user may have applied, discarded, or replaced it meanwhile, in which
        // case the next user turn sees the durable state.
        let task = self
            .inner
            .sessions
            .load(session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?
            .team_tasks
            .into_iter()
            .find(|task| task.id == task_id);
        let Some(task) = task.filter(|task| task.status.continues_interactively()) else {
            return Ok(());
        };
        let client_turn_id = ipc::new_client_turn_id();
        let text = crate::team::TEAM_TASK_CONTINUE_TEXT.to_string();
        // The panel records the turn as sent from this event: it precedes the
        // turn's own stream on the same session channel.
        if let Err(error) = worker.sink.emit_scoped(
            "team_task",
            crate::team::TeamTaskEvent {
                task,
                continuation: Some(crate::team::TeamTaskContinuation {
                    client_turn_id: client_turn_id.clone(),
                    text: text.clone(),
                }),
            },
        ) {
            eprintln!(
                "eud-agent: team continuation event failed: session={session_id} error={error}"
            );
        }
        if let Err(error) = self.inner.sessions.touch_conversation(session_id) {
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
                skill_selections: Default::default(),
            })
            .await;
        drop(engine);
        self.finish_read_command(&worker, result).await
    }

    /// Delete a retired team Map session (a fresh one replaced it for the next
    /// task) together with its candidate files. Never waits on a running
    /// turn: a session the user is chatting in stays, and is retired by a
    /// later task once it is idle.
    pub(crate) async fn retire_team_session(&self, session_id: &str) -> Result<(), String> {
        let busy = match self.registered_worker(session_id).await {
            Some(worker) => worker
                .engine
                .try_lock()
                .map(|engine| engine.phase != Phase::Idle)
                .unwrap_or(true),
            None => false,
        };
        if busy {
            return Err("the earlier team map session is still running a turn".to_string());
        }
        let project_id = self
            .inner
            .sessions
            .load(session_id)
            .map_err(|error| error.to_string())?
            .meta
            .project;
        self.delete_map_session(session_id).await?;
        let service = tauri::Manager::state::<crate::map_agent::MapAgentService>(&self.inner.app);
        if let Err(error) = service.discard_session_candidate(&project_id, session_id) {
            eprintln!("eud-agent: retired team Map session candidate cleanup failed: {error}");
        }
        Ok(())
    }

    pub(crate) async fn delete_map_session(&self, session_id: &str) -> Result<(), String> {
        let record = self
            .inner
            .sessions
            .load(session_id)
            .map_err(|error| error.to_string())?;
        if record.meta.kind != crate::session::SessionKind::Map {
            return Err("the requested session is not a Map session".to_string());
        }
        self.delete_session(session_id)
            .await
            .map_err(|error| error.message)
    }

    /// The Map worker, even when its first hydration failed only because the
    /// persisted provider conversation could not be resumed: that worker stays
    /// registered, and the Map window must open so the user can reset it.
    /// Never waits on a running turn: a busy engine is a live conversation.
    async fn map_worker(&self, session_id: &str) -> Result<Arc<SessionWorker>, String> {
        let worker = match self.worker(session_id).await {
            Ok(worker) => worker,
            Err(error) => {
                let registered = self.registered_worker(session_id).await;
                match registered {
                    Some(worker)
                        if worker
                            .engine
                            .try_lock()
                            .map(|engine| engine.conversation_resume_error().is_some())
                            .unwrap_or(true) =>
                    {
                        worker
                    }
                    _ => return Err(error.message),
                }
            }
        };
        if worker.runtime.kind() != crate::session::SessionKind::Map {
            return Err("the requested session is not a Map session".to_string());
        }
        Ok(worker)
    }

    /// Open a Map session; returns the resume error the user must clear before
    /// chatting, or `None` when the persisted conversation continues. A turn
    /// running on the session (a team request, or one started before the
    /// window reopened) proves the conversation is alive, so the bootstrap
    /// never waits for it.
    pub(crate) async fn open_map_session(
        &self,
        session_id: &str,
    ) -> Result<Option<String>, String> {
        let worker = self.map_worker(session_id).await?;
        Ok(idle_resume_error(&worker.engine, |engine| {
            engine.conversation_resume_error().map(str::to_string)
        }))
    }

    /// The latest Map run transcript of a session, without locking its engine;
    /// `None` when the session has no worker or never ran a request.
    pub(crate) async fn map_run_snapshot(&self, session_id: &str) -> Option<MapRunTranscript> {
        self.registered_worker(session_id)
            .await
            .and_then(|worker| worker.sink.map_run_snapshot())
    }

    pub(crate) async fn map_conversation_reset(&self, session_id: &str) -> Result<(), String> {
        let worker = self.map_worker(session_id).await?;
        let mut engine = worker.engine.lock().await;
        engine
            .reset_conversation()
            .await
            .map_err(|error| error.message)
    }

    /// Rewind a Map session's model-visible conversation to the panel-log
    /// prefix before an edited message. The candidate map and its revisions
    /// are untouched: only the provider thread and task branch move. A
    /// running turn (the window's own or a team request) is a live
    /// conversation, so the rewind refuses instead of waiting behind it.
    pub(crate) async fn map_conversation_rewind(
        &self,
        session_id: &str,
        panel_log: serde_json::Value,
    ) -> Result<(), String> {
        let worker = self.map_worker(session_id).await?;
        let Ok(mut engine) = worker.engine.try_lock() else {
            return Err("진행 중인 맵 요청을 먼저 중단한 뒤 메시지를 수정해 주세요.".to_string());
        };
        engine
            .rewind(panel_log)
            .await
            .map_err(|error| error.message)
    }

    /// Run one Map request; `prompt` is the request bubble the Map window
    /// shows for it, whether the window sent it or an EPS session's team task.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn map_chat(
        &self,
        session_id: &str,
        request_id: String,
        candidate_revision: String,
        text: String,
        attachments: Vec<String>,
        image_placement: bool,
        prompt: MapRunPrompt,
    ) -> Result<MapChatResult, String> {
        let worker = self
            .worker(session_id)
            .await
            .map_err(|error| error.message)?;
        let _provider_busy = self.inner.provider_service.enter_busy(worker.provider);
        if worker.runtime.kind() != crate::session::SessionKind::Map {
            return Err("the requested session is not a Map session".to_string());
        }
        if let Err(error) = self.inner.sessions.touch_conversation(session_id) {
            eprintln!("eud-agent: map conversation timestamp update failed: {error}");
        }
        worker
            .runtime
            .emit_activity(crate::write_coordinator::SessionActivity::RunningRead);
        let origin = prompt.origin;
        worker
            .sink
            .set_map_context(request_id.clone(), candidate_revision, prompt);
        let result = {
            let mut engine = worker.engine.lock().await;
            engine
                .map_chat(request_id.clone(), text, attachments, image_placement)
                .await
        };
        if let (MapRunOrigin::Team, Err(error)) = (origin, &result) {
            // A user-typed request reports its failure through the invoke
            // result; a team run has no caller in the window, so its failure
            // closes the run's transcript as the terminal event it replays.
            let _ = worker.sink.emit(EngineEvent::Error(ipc::ErrorEvent {
                message: error.message.clone(),
            }));
        }
        worker.sink.clear_map_context(&request_id);
        let end = result.as_ref().map_or(MapTurnEnd::Completed, |end| *end);
        let pending_workflow_progress = if result.is_ok() && end == MapTurnEnd::Completed {
            worker.runtime.take_pending_workflow_progress()
        } else {
            worker.runtime.discard_pending_workflow_progress();
            Vec::new()
        };
        self.finish_read_command(&worker, result.map(|_| ()))
            .await
            .map_err(|error| error.message)?;
        Ok(MapChatResult {
            end,
            pending_workflow_progress,
        })
    }

    pub(crate) async fn cancel_map_session(&self, session_id: &str) -> Result<(), String> {
        let worker = self
            .worker(session_id)
            .await
            .map_err(|error| error.message)?;
        if worker.runtime.kind() != crate::session::SessionKind::Map {
            return Err("the requested session is not a Map session".to_string());
        }
        self.cancel(session_id).await.map_err(|error| error.message)
    }

    async fn plan_feedback(
        &self,
        session_id: &str,
        request: ipc::PlanFeedbackRequest,
    ) -> Result<(), AgentEngineError> {
        let worker = self.worker(session_id).await?;
        let _provider_busy = self.inner.provider_service.enter_busy(worker.provider);
        if let Err(error) = self.inner.sessions.touch_conversation(session_id) {
            eprintln!("eud-agent: conversation timestamp update failed: {error}");
        }
        worker
            .runtime
            .emit_activity(crate::write_coordinator::SessionActivity::RunningRead);
        let result = {
            let mut engine = worker.engine.lock().await;
            let result = engine.plan_feedback(request).await;
            if result.is_ok() {
                engine.update_active_session().await;
            }
            result
        };
        self.finish_read_command(&worker, result).await
    }

    async fn plan_approve(&self, session_id: &str) -> Result<(), AgentEngineError> {
        let worker = self.worker(session_id).await?;
        let _provider_busy = self.inner.provider_service.enter_busy(worker.provider);
        let approved = {
            let mut engine = worker.engine.lock().await;
            let approved = engine.plan_approve().await;
            if approved.is_ok() {
                engine.update_active_session().await;
            }
            approved
        };
        let finished = match approved {
            Ok(()) => self.drive_pending_write(Arc::clone(&worker)).await,
            Err(error) => Err(error),
        };
        self.continue_after_command(&worker).await;
        finished
    }

    async fn compact(&self, session_id: &str) -> Result<(), AgentEngineError> {
        let worker = self.worker(session_id).await?;
        let _provider_busy = self.inner.provider_service.enter_busy(worker.provider);
        worker
            .runtime
            .emit_activity(crate::write_coordinator::SessionActivity::RunningRead);
        let (result, phase) = {
            let mut engine = worker.engine.lock().await;
            let result = engine.compact().await;
            (result, engine.phase)
        };
        let activity = if matches!(phase, Phase::PlanReview) {
            crate::write_coordinator::SessionActivity::Review
        } else {
            crate::write_coordinator::SessionActivity::Idle
        };
        worker.runtime.emit_activity(activity);
        result
    }

    async fn rewind(
        &self,
        session_id: &str,
        panel_log: serde_json::Value,
    ) -> Result<(), AgentEngineError> {
        match self.worker(session_id).await {
            Ok(worker) => worker.engine.lock().await.rewind(panel_log).await,
            Err(worker_error) => {
                let expected_project =
                    project_name_from_state(&self.inner.config.project_state_for_prompt());
                let recovered_project = rewind_unrecoverable_pending_session(
                    &self.inner.sessions,
                    &self.inner.dirs,
                    session_id,
                    &expected_project,
                    panel_log,
                )
                .map_err(AgentEngineError::new)?;
                let Some(project_id) = recovered_project else {
                    return Err(worker_error);
                };
                self.clear_session_recovery_error(&project_id, session_id);
                Ok(())
            }
        }
    }

    async fn pending_ask(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionEvent<ipc::AskEvent>>, AgentEngineError> {
        let worker = self.registered_worker(session_id).await;
        Ok(worker.and_then(|worker| {
            worker
                .runtime
                .pending_ask()
                .map(|event| worker.sink.scoped(event))
        }))
    }

    async fn answer_ask(
        &self,
        session_id: &str,
        request: ipc::AskResponseRequest,
    ) -> Result<(), AgentEngineError> {
        let worker = self
            .registered_worker(session_id)
            .await
            .ok_or_else(|| AgentEngineError::new("ask session is not active"))?;
        worker
            .runtime
            .answer_ask(&request.request_id, request.answers)
            .map_err(AgentEngineError::new)
    }

    async fn autonomous_pause(&self, session_id: &str) -> Result<(), AgentEngineError> {
        let worker = self.worker(session_id).await?;
        let mut record = self
            .inner
            .sessions
            .load(session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let run = record
            .autonomous_run
            .as_mut()
            .ok_or_else(|| AgentEngineError::new("일시 중지할 장시간 작업이 없습니다."))?;
        if run.status != crate::autonomous::AutonomousRunStatus::Running {
            return Err(AgentEngineError::new(
                "현재 장시간 작업은 실행 중 상태가 아닙니다.",
            ));
        }
        worker
            .autonomous_pause_requested
            .store(true, Ordering::SeqCst);
        run.status = crate::autonomous::AutonomousRunStatus::Pausing;
        run.pause_reason = Some(crate::autonomous::AutonomousPauseReason::User);
        run.updated_at = crate::session::now_unix_millis();
        let run_payload = run.clone();
        self.inner
            .sessions
            .save(&record)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        worker
            .sink
            .emit_scoped("autonomous_run", run_payload)
            .map_err(|error| AgentEngineError::new(error.to_string()))
    }

    async fn autonomous_resume(&self, session_id: &str) -> Result<(), AgentEngineError> {
        let worker = self.worker(session_id).await?;
        let _provider_busy = self.inner.provider_service.enter_busy(worker.provider);
        worker
            .runtime
            .emit_activity(crate::write_coordinator::SessionActivity::RunningRead);
        let result = {
            let mut engine = worker.engine.lock().await;
            engine.autonomous_resume().await
        };
        self.finish_read_command(&worker, result).await
    }

    async fn autonomous_stop(&self, session_id: &str) -> Result<(), AgentEngineError> {
        let worker = self.worker(session_id).await?;
        worker
            .autonomous_pause_requested
            .store(true, Ordering::SeqCst);
        worker.runtime.cancel_pending_ask();
        let record = self
            .inner
            .sessions
            .load(session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let run = record
            .autonomous_run
            .as_ref()
            .ok_or_else(|| AgentEngineError::new("중단할 장시간 작업이 없습니다."))?;
        if run.status.is_terminal() {
            return Err(AgentEngineError::new("장시간 작업이 이미 종료되었습니다."));
        }
        let run_payload = self
            .inner
            .sessions
            .update_autonomous_status(
                session_id,
                crate::autonomous::AutonomousRunStatus::Cancelled,
                None,
                Some("사용자가 장시간 작업을 중단했습니다.".to_string()),
            )
            .map_err(|error| AgentEngineError::new(error.to_string()))?
            .ok_or_else(|| AgentEngineError::new("중단할 장시간 작업이 없습니다."))?;
        worker
            .sink
            .emit_scoped("autonomous_run", run_payload)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        cancel_worker_generation(&worker.cancellation)?;
        let mut engine = worker.engine.lock().await;
        if worker.runtime.write_ticket().is_some() {
            engine.recover_write_failure()?;
        } else {
            engine.phase = Phase::Idle;
            worker
                .runtime
                .emit_activity(crate::write_coordinator::SessionActivity::Idle);
        }
        engine.update_active_session().await;
        Ok(())
    }

    async fn cancel(&self, session_id: &str) -> Result<(), AgentEngineError> {
        let worker = self.worker(session_id).await?;
        worker.runtime.cancel_pending_ask();
        if let Ok(engine) = worker.engine.try_lock() {
            if engine.phase == Phase::PlanReview {
                return Err(AgentEngineError::new(
                    "검토 중인 계획을 먼저 승인하거나 수정 요청해 주세요.",
                ));
            }
        }
        // Stopping a worker session is cancelling its worker, so the worker
        // never continues its parent; and every worker this session started
        // dies with this session's turn. Both only mark records and queue
        // work, so neither waits for a worker's turn, and both happen before
        // anything below can fail.
        self.cancel_own_worker(session_id);
        self.cancel_workers_of(session_id);
        cancel_worker_generation(&worker.cancellation)?;
        let mut engine = worker.engine.lock().await;
        if worker.runtime.write_ticket().is_some() {
            engine.recover_write_failure()?;
        } else {
            engine.phase = Phase::Idle;
            worker
                .runtime
                .emit_activity(crate::write_coordinator::SessionActivity::Idle);
        }
        engine.update_active_session().await;
        Ok(())
    }

    fn create_session(
        &self,
        window: &str,
        first_text: &str,
    ) -> Result<crate::session::SessionRecord, AgentEngineError> {
        let created_at = crate::session::now_unix_seconds();
        let config = self
            .inner
            .dirs
            .load_config()
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let provider_binding =
            crate::provider::default_binding(&config).map_err(AgentEngineError::new)?;
        let handle = self
            .inner
            .registry
            .for_window(window)
            .map_err(AgentEngineError::new)?;
        let open = self
            .open_project_of(&handle)
            .map_err(AgentEngineError::new)?;
        let mut meta = crate::session::SessionMeta::new_for(
            crate::session::SessionKind::Eps,
            auto_session_name(first_text),
            &provider_binding,
            &open.session,
            None,
        );
        meta.created_at = created_at;
        let record = crate::session::SessionRecord {
            meta,
            provider_binding,
            pending_request_ids: Vec::new(),
            context_usage: None,
            panel_log: serde_json::Value::Null,
            context_state: Default::default(),
            task_state: Default::default(),
            autonomous_run: None,
            team_tasks: Vec::new(),
            workers: Vec::new(),
        };
        self.inner
            .sessions
            .save(&record)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        Ok(record)
    }

    async fn open_session(
        &self,
        id: &str,
    ) -> Result<crate::session::SessionRecord, AgentEngineError> {
        let worker = self.worker(id).await?;
        let result = worker.engine.lock().await.hydrate().await;
        result
    }

    async fn delete_session(&self, id: &str) -> Result<(), AgentEngineError> {
        if let Some(refusal) = self.worker_delete_refusal(id) {
            return Err(AgentEngineError::new(refusal));
        }
        // Its pending work lives in its project: a session of
        // a closed project is never deleted from under it.
        let handle = self.session_handle(id)?;
        if let Some(worker) = self.registered_worker(id).await {
            let engine = worker.engine.lock().await;
            if engine.phase != Phase::Idle {
                return Err(AgentEngineError::new(
                    "실행 또는 검토 중인 세션은 삭제할 수 없습니다.",
                ));
            }
        }
        // Every team Map session goes with its EPS parent, but never while the
        // user still has one of its candidates to settle.
        let teams = self
            .inner
            .sessions
            .team_sessions_of(id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        if !teams.is_empty() {
            let record = self
                .inner
                .sessions
                .load(id)
                .map_err(|error| AgentEngineError::new(error.to_string()))?;
            if record.team_tasks.iter().any(|task| task.status.is_active()) {
                return Err(AgentEngineError::new(
                    "맵 창에서 이 세션의 맵 작업 후보를 적용하거나 폐기한 뒤 세션을 삭제해 주세요.",
                ));
            }
            for team in teams {
                Box::pin(self.delete_map_session(&team.id))
                    .await
                    .map_err(AgentEngineError::new)?;
                let service =
                    tauri::Manager::state::<crate::map_agent::MapAgentService>(&self.inner.app);
                if let Err(error) = service.discard_session_candidate(&team.project, &team.id) {
                    eprintln!("eud-agent: team Map session candidate cleanup failed: {error}");
                }
            }
        }
        if let Ok(native) = handle.native() {
            native.revoke_python_dependency_candidates(id);
        }
        self.unregister_worker(id).await;
        self.inner
            .sessions
            .delete(id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        crate::provider_transcript::ProviderTranscriptStore::new(&self.inner.dirs)
            .delete_session(id)
            .map_err(AgentEngineError::new)?;
        self.inner
            .attachments
            .delete_session(id)
            .map_err(AgentEngineError::new)
    }

    async fn session_model_settings(
        &self,
        session_id: &str,
    ) -> Result<crate::provider::SessionModelSettings, AgentEngineError> {
        let record = self
            .inner
            .sessions
            .load(session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let mut models = self
            .inner
            .provider_service
            .catalog(record.provider_binding.provider)
            .await
            .map_err(AgentEngineError::new)?;
        if record.provider_binding.provider == crate::provider::ProviderId::Ollama
            && !models
                .iter()
                .any(|model| model.model == record.provider_binding.model)
        {
            models.push(
                crate::ollama::provider_model(
                    &record.provider_binding.model,
                    Some(&record.provider_binding.model),
                )
                .map_err(AgentEngineError::new)?,
            );
        }
        // A degraded Claude catalog (expired token, offline) must not blank a session that is
        // still bound to a valid catalog model; keep the bound id selectable with its saved level.
        if record.provider_binding.provider == crate::provider::ProviderId::ClaudeCode
            && !models
                .iter()
                .any(|model| model.model == record.provider_binding.model)
        {
            models.push(
                crate::claude_client::bound_model(
                    &record.provider_binding.model,
                    record.provider_binding.reasoning.as_ref(),
                )
                .map_err(AgentEngineError::new)?,
            );
        }
        if !models
            .iter()
            .any(|model| model.model == record.provider_binding.model)
        {
            return Err(AgentEngineError::new("provider_model_unavailable"));
        }
        Ok(crate::provider::SessionModelSettings {
            provider: record.provider_binding.provider,
            models,
            selected_model: record.provider_binding.model,
            selected_reasoning: record.provider_binding.reasoning,
        })
    }

    async fn session_model_settings_save(
        &self,
        session_id: &str,
        model: String,
        reasoning: Option<crate::provider::ReasoningSelection>,
    ) -> Result<crate::provider::SessionModelSettings, AgentEngineError> {
        let record = self
            .inner
            .sessions
            .load(session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        if let Some(worker) = self.registered_worker(session_id).await {
            let engine = worker
                .engine
                .try_lock()
                .map_err(|_| AgentEngineError::new("provider_busy"))?;
            if engine.phase != Phase::Idle || worker.runtime.pending_ask().is_some() {
                return Err(AgentEngineError::new("provider_busy"));
            }
        }
        let model = if record.provider_binding.provider == crate::provider::ProviderId::Ollama {
            crate::ollama::validate_model(&model)
                .map_err(AgentEngineError::new)?
                .to_string()
        } else {
            model
        };
        let models = if record.provider_binding.provider == crate::provider::ProviderId::Ollama {
            vec![crate::ollama::provider_model(&model, Some(&model))
                .map_err(AgentEngineError::new)?]
        } else {
            self.inner
                .provider_service
                .catalog(record.provider_binding.provider)
                .await
                .map_err(AgentEngineError::new)?
        };
        let selected = models
            .iter()
            .find(|candidate| candidate.model == model)
            .ok_or_else(|| AgentEngineError::new("provider_model_unavailable"))?;
        if let Some(reasoning) = reasoning.as_ref() {
            if !selected
                .capabilities
                .reasoning_levels
                .iter()
                .any(|level| level.as_str() == reasoning.level)
            {
                return Err(AgentEngineError::new("provider_capability_unsupported"));
            }
        }
        let record = self
            .inner
            .sessions
            .update_model_settings(
                session_id,
                record.provider_binding.provider,
                model,
                reasoning,
            )
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        self.unregister_worker(session_id).await;
        Ok(crate::provider::SessionModelSettings {
            provider: record.provider_binding.provider,
            models,
            selected_model: record.provider_binding.model,
            selected_reasoning: record.provider_binding.reasoning,
        })
    }
}

// The commands below are thin Tauri wrappers over `*_inner` functions that
// take the manager and plain arguments. The remote dispatcher
// (`remote::dispatch`) calls the same functions, with a remote connection's
// label where a window label is needed.

pub(crate) async fn session_model_settings_inner(
    state: &SessionEngineManager,
    session_id: &str,
) -> Result<crate::provider::SessionModelSettings, String> {
    state
        .session_model_settings(session_id)
        .await
        .map_err(|error| error.message)
}

#[tauri::command]
pub(crate) async fn session_model_settings(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
) -> Result<crate::provider::SessionModelSettings, String> {
    session_model_settings_inner(&state, &session_id).await
}

pub(crate) async fn session_model_settings_save_inner(
    state: &SessionEngineManager,
    session_id: &str,
    model: String,
    reasoning: Option<crate::provider::ReasoningSelection>,
) -> Result<crate::provider::SessionModelSettings, String> {
    state
        .session_model_settings_save(session_id, model, reasoning)
        .await
        .map_err(|error| error.message)
}

#[tauri::command]
pub(crate) async fn session_model_settings_save(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
    model: String,
    reasoning: Option<crate::provider::ReasoningSelection>,
) -> Result<crate::provider::SessionModelSettings, String> {
    session_model_settings_save_inner(&state, &session_id, model, reasoning).await
}

/// The arguments of the `chat` command, named as the panel sends them.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatCommand {
    pub session_id: String,
    pub text: String,
    pub attachments: Vec<String>,
    pub client_turn_id: String,
    pub mentions: Option<Vec<crate::mentions::MentionInstance>>,
    pub execution_mode: Option<crate::autonomous::ExecutionMode>,
    pub autonomous_policy: Option<crate::autonomous::AutonomousRunPolicy>,
    pub clarify: Option<bool>,
    pub skill_selections: Option<std::collections::BTreeMap<String, bool>>,
}

pub(crate) async fn chat_inner(
    state: &SessionEngineManager,
    command: ChatCommand,
) -> Result<(), String> {
    state
        .chat(
            &command.session_id,
            ipc::ChatRequest {
                client_turn_id: command.client_turn_id,
                text: command.text,
                attachments: command.attachments,
                mentions: command.mentions.unwrap_or_default(),
                execution_mode: command.execution_mode.unwrap_or_default(),
                autonomous_policy: command.autonomous_policy,
                clarify: command.clarify.unwrap_or_default(),
                skill_selections: command.skill_selections.unwrap_or_default(),
            },
        )
        .await
        .map_err(|error| error.message)
}

#[allow(
    clippy::too_many_arguments,
    reason = "Tauri exposes each chat request field as a named command argument"
)]
#[tauri::command(rename = "chat")]
pub(crate) async fn engine_chat(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
    text: String,
    attachments: Vec<String>,
    client_turn_id: String,
    mentions: Option<Vec<crate::mentions::MentionInstance>>,
    execution_mode: Option<crate::autonomous::ExecutionMode>,
    autonomous_policy: Option<crate::autonomous::AutonomousRunPolicy>,
    clarify: Option<bool>,
    skill_selections: Option<std::collections::BTreeMap<String, bool>>,
) -> Result<(), String> {
    chat_inner(
        &state,
        ChatCommand {
            session_id,
            text,
            attachments,
            client_turn_id,
            mentions,
            execution_mode,
            autonomous_policy,
            clarify,
            skill_selections,
        },
    )
    .await
}

/// The arguments of the `plan_feedback` command.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanFeedbackCommand {
    pub session_id: String,
    pub text: String,
    pub attachments: Vec<String>,
    pub mentions: Option<Vec<crate::mentions::MentionInstance>>,
    pub client_turn_id: String,
}

pub(crate) async fn plan_feedback_inner(
    state: &SessionEngineManager,
    command: PlanFeedbackCommand,
) -> Result<(), String> {
    state
        .plan_feedback(
            &command.session_id,
            ipc::PlanFeedbackRequest {
                text: command.text,
                client_turn_id: command.client_turn_id,
                attachments: command.attachments,
                mentions: command.mentions.unwrap_or_default(),
            },
        )
        .await
        .map_err(|error| error.message)
}

#[tauri::command(rename = "plan_feedback")]
pub(crate) async fn engine_plan_feedback(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
    text: String,
    attachments: Vec<String>,
    mentions: Option<Vec<crate::mentions::MentionInstance>>,
    client_turn_id: String,
) -> Result<(), String> {
    plan_feedback_inner(
        &state,
        PlanFeedbackCommand {
            session_id,
            text,
            attachments,
            mentions,
            client_turn_id,
        },
    )
    .await
}

pub(crate) async fn plan_approve_inner(
    state: &SessionEngineManager,
    session_id: &str,
) -> Result<(), String> {
    state
        .plan_approve(session_id)
        .await
        .map_err(|error| error.message)
}

#[tauri::command(rename = "plan_approve")]
pub(crate) async fn engine_plan_approve(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
) -> Result<(), String> {
    plan_approve_inner(&state, &session_id).await
}

pub(crate) async fn ask_pending_inner(
    state: &SessionEngineManager,
    session_id: &str,
) -> Result<Option<SessionEvent<ipc::AskEvent>>, String> {
    state
        .pending_ask(session_id)
        .await
        .map_err(|error| error.message)
}

#[tauri::command(rename = "ask_pending")]
pub(crate) async fn engine_ask_pending(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
) -> Result<Option<SessionEvent<ipc::AskEvent>>, String> {
    ask_pending_inner(&state, &session_id).await
}

pub(crate) async fn ask_response_inner(
    state: &SessionEngineManager,
    session_id: &str,
    request_id: String,
    answers: std::collections::BTreeMap<String, ipc::AskAnswer>,
) -> Result<(), String> {
    state
        .answer_ask(
            session_id,
            ipc::AskResponseRequest {
                request_id,
                answers,
            },
        )
        .await
        .map_err(|error| error.message)
}

#[tauri::command(rename = "ask_response")]
pub(crate) async fn engine_ask_response(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
    request_id: String,
    answers: std::collections::BTreeMap<String, ipc::AskAnswer>,
) -> Result<(), String> {
    ask_response_inner(&state, &session_id, request_id, answers).await
}

#[tauri::command(rename = "autonomous_pause")]
pub(crate) async fn engine_autonomous_pause(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
) -> Result<(), String> {
    state
        .autonomous_pause(&session_id)
        .await
        .map_err(|error| error.message)
}

#[tauri::command(rename = "autonomous_resume")]
pub(crate) async fn engine_autonomous_resume(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
) -> Result<(), String> {
    state
        .autonomous_resume(&session_id)
        .await
        .map_err(|error| error.message)
}

#[tauri::command(rename = "autonomous_stop")]
pub(crate) async fn engine_autonomous_stop(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
) -> Result<(), String> {
    state
        .autonomous_stop(&session_id)
        .await
        .map_err(|error| error.message)
}

pub(crate) async fn cancel_inner(
    state: &SessionEngineManager,
    session_id: &str,
) -> Result<(), String> {
    state
        .cancel(session_id)
        .await
        .map_err(|error| error.message)
}

#[tauri::command(rename = "cancel")]
pub(crate) async fn engine_cancel(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
) -> Result<(), String> {
    cancel_inner(&state, &session_id).await
}

pub(crate) async fn compact_inner(
    state: &SessionEngineManager,
    session_id: &str,
) -> Result<(), String> {
    state
        .compact(session_id)
        .await
        .map_err(|error| error.message)
}

#[tauri::command(rename = "compact")]
pub(crate) async fn engine_compact(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
) -> Result<(), String> {
    compact_inner(&state, &session_id).await
}

pub(crate) async fn conversation_rewind_inner(
    state: &SessionEngineManager,
    session_id: &str,
    panel_log: serde_json::Value,
) -> Result<(), String> {
    state
        .rewind(session_id, panel_log)
        .await
        .map_err(|error| error.message)
}

#[tauri::command(rename = "conversation_rewind")]
pub(crate) async fn engine_conversation_rewind(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
    panel_log: serde_json::Value,
) -> Result<(), String> {
    conversation_rewind_inner(&state, &session_id, panel_log).await
}

/// The EPS sessions of the project window (or remote connection) `label`
/// works on.
pub(crate) async fn session_list_inner(
    state: &SessionEngineManager,
    label: &str,
) -> Result<Vec<crate::session::SessionMeta>, String> {
    let manager = state.clone();
    tokio::task::spawn_blocking(move || manager.recover_all_projects())
        .await
        .map_err(|error| error.to_string())??;
    let Ok(handle) = state.inner.registry.for_window(label) else {
        return Ok(Vec::new());
    };
    let sessions = state
        .inner
        .sessions
        .list_kind(crate::session::SessionKind::Eps)
        .map_err(|error| error.to_string())?;
    Ok(sessions_of_project(
        sessions,
        &handle,
        &state.inner.registry,
    ))
}

#[tauri::command(rename = "session_list")]
pub(crate) async fn engine_session_list(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, SessionEngineManager>,
) -> Result<Vec<crate::session::SessionMeta>, String> {
    session_list_inner(&state, window.label()).await
}

pub(crate) fn session_load_inner(
    state: &SessionEngineManager,
    id: &str,
) -> Result<crate::session::SessionRecord, String> {
    state
        .inner
        .sessions
        .load(id)
        .map_err(|error| error.to_string())
}

#[tauri::command(rename = "session_load")]
pub(crate) async fn engine_session_load(
    state: tauri::State<'_, SessionEngineManager>,
    id: String,
) -> Result<crate::session::SessionRecord, String> {
    session_load_inner(&state, &id)
}

pub(crate) fn session_create_inner(
    state: &SessionEngineManager,
    label: &str,
    first_text: &str,
) -> Result<crate::session::SessionRecord, String> {
    state
        .create_session(label, first_text)
        .map_err(|error| error.message)
}

#[tauri::command(rename = "session_create")]
pub(crate) async fn engine_session_create(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, SessionEngineManager>,
    first_text: String,
) -> Result<crate::session::SessionRecord, String> {
    session_create_inner(&state, window.label(), &first_text)
}

pub(crate) fn session_update_log_inner(
    state: &SessionEngineManager,
    id: &str,
    panel_log: serde_json::Value,
) -> Result<(), String> {
    state
        .inner
        .sessions
        .update_panel_log(id, panel_log)
        .map_err(|error| error.to_string())
}

#[tauri::command(rename = "session_update_log")]
pub(crate) async fn engine_session_update_log(
    state: tauri::State<'_, SessionEngineManager>,
    id: String,
    panel_log: serde_json::Value,
) -> Result<(), String> {
    session_update_log_inner(&state, &id, panel_log)
}

pub(crate) async fn session_open_inner(
    state: &SessionEngineManager,
    id: &str,
) -> Result<crate::session::SessionRecord, String> {
    state.open_session(id).await.map_err(|error| error.message)
}

#[tauri::command(rename = "session_open")]
pub(crate) async fn engine_session_open(
    state: tauri::State<'_, SessionEngineManager>,
    id: String,
) -> Result<crate::session::SessionRecord, String> {
    session_open_inner(&state, &id).await
}

pub(crate) fn session_rename_inner(
    state: &SessionEngineManager,
    id: &str,
    name: &str,
) -> Result<(), String> {
    state
        .inner
        .sessions
        .rename(id, name)
        .map_err(|error| error.to_string())
}

#[tauri::command(rename = "session_rename")]
pub(crate) async fn engine_session_rename(
    state: tauri::State<'_, SessionEngineManager>,
    id: String,
    name: String,
) -> Result<(), String> {
    session_rename_inner(&state, &id, &name)
}

pub(crate) async fn session_delete_inner(
    state: &SessionEngineManager,
    id: &str,
) -> Result<(), String> {
    state
        .delete_session(id)
        .await
        .map_err(|error| error.message)
}

#[tauri::command(rename = "session_delete")]
pub(crate) async fn engine_session_delete(
    state: tauri::State<'_, SessionEngineManager>,
    id: String,
) -> Result<(), String> {
    session_delete_inner(&state, &id).await
}

fn approved_plan_execution_instruction(request_id: &str) -> Result<String, AgentEngineError> {
    let plan_path =
        approved_plan_path(request_id).map_err(|error| AgentEngineError::new(error.to_string()))?;
    Ok(format!(
        "The user approved the current plan. Execute it now.\n\
         The app saved the exact approved plan at `{plan_path}`; do not edit, rename, or delete it.\n\
         Read accepted specs as implementation context and update project documentation when the implementation requires it.\n\
         Apply the approved changes, run the authoritative build, report any required runtime verification, and answer immediately. Do not call `propose_plan` again unless implementation cannot proceed."
    ))
}

pub fn build_map_system_prompt(project_state: &str, project_memory: Option<&str>) -> String {
    format!(
        "[role]\n\
         You are Map Agent inside the separate Map Agent Workbench window.\n\
         The connected saved OpenMapName SCX and visible candidate revision are the only map authority.\n\n\
         {error_recovery}\n\n\
         {map_locations}\n\n\
         [project documents]\n\
         - Freely read, create, overwrite and edit project .md/.txt documents using fs_read/fs_write/fs_edit without approval, write registration or build verification. Search with fs_glob/fs_grep. Documents may live anywhere inside the project, including maps/, references/, skill folders and .eud-agent/workspace; .git remains protected. Filesystem document tools cannot write source, manifest, permission settings or map binaries.\n\n\
         [authority]\n\
         - Exact backend-validated MapMentionSnapshot payloads define request constraints; never parse display text into authority.\n\
         - Without a target mention, the entire current candidate is writable for terrain, units, buildings, doodads, sprites, and locations. Never refuse mutation or ask for a region merely because target is absent.\n\
         - A target region narrows coordinate-based writes to its exact cells and explicit layer capabilities. Stored targets omitted from the current request, natural language, reference/anchor regions, palette mentions, and stamp mentions cannot enlarge or narrow that scope.\n\
         - Reference and anchor regions are read/comparison context only. Protect masks always block their cells and layers, including persistent protections omitted from a later prompt.\n\
         - Palette mentions describe a type/style. Saved-selection stamp mentions and importedStamp mentions identify copy sources; none grants destination placement authority. Object and location mentions remain revision-bound exact instances; stale fingerprints must be reported.\n\n\
         [candidate workflow]\n\
         - Modify only the request-owned draft through map_draft_begin, map_draft_patch, map_stamp_place, and map_image_place.\n\
         - Use map_draft_render and map_draft_analyze while iterating, then call map_candidate_finalize once at most.\n\
         - A failed, cancelled, or unfinalized turn must leave the visible candidate unchanged.\n\
         - Follow-up turns start from the visible candidate revision supplied by the backend.\n\
         - Terrain, units, buildings, doodads, sprites, and locations are writable spatial layers. Player slots and forces are writable scenario properties through map_properties_patch.\n\
         - Fog, triggers, briefing, switches, tech/upgrades, and sounds are unsupported and must remain unchanged. For player count, computer owners, teams, or alliance changes, read map_properties_read first, then call map_draft_begin and map_properties_patch. Player and force numbers are 1-based. Force membership is limited to P1..P8; forces are 1..4. Use Unicode force names; the backend encodes them for the map. Finalize the candidate for user review.\n\
         - Semantic ISOM transitions outside the current request scope make finalize fail. Do not clip, hide, or substitute them; ask the user to expand a supplied target only when that target blocks the requested transition.\n\
         - A doodad that changes terrain plus a sprite overlay requires terrain, doodads, and sprites authority.\n\
         - Materially ambiguous owner, count, state, or location bounds require the ask tool.\n\n\
         [gameplay geometry]\n\
         - Preserve the requested route topology independently of visual outline. When a loop is requested, keep a closed traversable cycle for any agreed shape; an open spawn-to-exit route does not satisfy that requirement. Do not cut it to add a gate, access corridor or management platform.\n\
         - For a loop, verify each ordered waypoint segment and the last-to-first segment with actual walkability, elevation and the largest supported unit collision footprint. Closure must follow the agreed route, not backtrack through earlier segments. Verify isolation from protected gameplay areas separately.\n\
         - A distinct summon/management function does not imply an external platform or connecting corridor. Follow the accepted spawn and placement design; incompatible access requirements need resolution rather than a changed route topology.\n\
         - map_draft_analyze validates candidate changes; its valid result alone does not prove route closure, clearance or gameplay separation. Report those checks against the requested geometry before finalizing.\n\n\
         [draft patch operations]\n\
         Every map_draft_patch operation is a flat object with op plus exactly the listed keys; bracketed keys are optional and state{{...}} lists nested object keys. Never invent keys such as tileId.\n\
         {operations_guide}\n\
         - Every value is typed exactly as advertised: brush, after, tiles, and replacementTiles are numeric ids from map_palette_query, never names such as 'Dirt'.\n\
         - terrain.set before must equal the current tile id at (x, y). Read exact ids with map_terrain_read (visible candidate) or map_draft_terrain_read (request draft) — including when replicating an existing pattern — or use terrain.rect / terrain.blit, which need no expected-before value. Never guess before values or probe them through conflict errors.\n\
         - Semantic terrain that must blend with its surroundings (a hill or plateau, a pool, a different ground type) is an ISOM brush job, exactly like SCMDraft's isometric brush: terrain.isom_rect fills a tile rectangle with one brush id from map_palette_query kind=brushes and generates the matching transition tiles (cliff edges, shorelines) itself: the rectangle interior becomes the brush terrain and a ring up to two diamonds wide (8 tiles sideways, 4 tiles up and down) on and outside the rectangle border is regenerated from the map's ISOM data, so where the surroundings were laid with exact tiles the ring shows the ISOM terrain rather than the visible tiles; render or analyze the draft after painting. A high brush (High Dirt, High Jungle, ...) painted on low ground is a hill with cliffs; ramps are separate doodads. Give terrain.isom_rect at least 5x3 tiles plus room for the ring; every map with an ISOM section supports it, and the error names the reason when it does not. terrain.isom_brush is the single-diamond form on ISOM grid coordinates: isomX is a tile x / 2 (0..width/2), isomY is a tile y (0..height), isomX + isomY must be even, one diamond covers tiles [2*isomX-2, 2*isomX+1] x [isomY-1, isomY], and extent N is an NxN diamond block. Never assemble cliff edges from exact tile ids with terrain.set/terrain.rect/terrain.blit unless the user asks for exact tiles, and never conclude a map lacks ISOM data from a rejected isom_brush call.\n\
         - ordinal and beforeFingerprint identify one existing object exactly as map_objects_read returned it for the current revision.\n\
         - tiles and replacementTiles are row-major matrices of exact tile ids whose top-left is (x, y) or the doodad footprint.\n\n\
         [selection stamps]\n\
         - A candidateSelection source is an exact reusable stamp whose content is read from the visible candidate when placed. An imported source is a pinned external-map snapshot authorized only by an importedStamp mention in the current request. Empty layers mean all six supported layers.\n\
         - For exact copy/duplicate/replicate requests, use map_stamp_preview and map_stamp_place. Never reconstruct either source through map_render, tile catalog enumeration, terrain.set probes, terrain.blit matrices, expected-before probes, or semantic ISOM brushes.\n\
         - Imported stamps expose only a compact id and bounded metadata. Filesystem paths, pickers, blob paths, raw CHK, MTXM/TILE matrices, and import management are unavailable and must not be requested.\n\
         - A destination is the top-left of the source selection bounds. If the requested total includes an existing candidate source, place only the additional copies. Stamp destinations in one call must not overlap.\n\
         - Call map_stamp_preview after map_draft_begin and use only imported sources mentioned in the current request. Terrain replacement is inherent and is not an object collision. When object or location collisions exist, obtain the user's explicit merge, replace, or cancel choice unless that choice is already explicit in the current request. Never guess a collision policy.\n\
         - Merge preserves destination objects and adds copied objects/locations. Replace removes only fully contained destination objects/locations in selected layers; boundary-crossing items make replace fail closed. Both modes copy exact MTXM/TILE values and never run ISOM correction.\n\n\
         [palette search]\n\
         - map_palette_query is a bounded search, not a browseable catalog. Supply a non-blank name query or structured filter; it returns a complete result only when at most 256 entries match.\n\
         - map_palette_query kind is a catalog family, not a palette mention kind: use brushes for semanticTerrain, tiles for exactTile, and units/buildings/doodads/sprites for the corresponding object types. For semantic terrain, search brushes by name first. Use the returned terrainType to filter exact tiles by graphicsValid, walkability, height, ramp, view, group, or variant metadata only when exact tiles are necessary.\n\
         - If a palette search is too broad, refine the query/filter. Never enumerate tile ids or catalog pages.\n\n\
         [image terrain]\n\
         - Current-request images are listed as image-1, image-2, and so on under [map image refs] while the same files remain available as localImage vision inputs. imageRef is an input binding, never extra write authority.\n\
         - When the user asks to apply an attached photo as terrain, call map_image_place with only imageRef and integer tile x/y/width/height. Never provide a filesystem path, palette, MTXM id, or tile matrix.\n\
         - When the user asks only to inspect, compare, or analyze an image, do not create a terrain mutation.\n\
         - [image placement requested] means the user turned on 사진 배치: place every listed image with map_image_place (you choose x/y/width/height from map analysis, keeping its aspect ratio) and never imitate a photo with terrain patches, brushes, or doodads. Other requested edits may follow on the same draft.\n\
         - Without a target, choose any in-map placement based on map analysis. With a target, every actually changed terrain cell must remain inside its terrain scope. Protect always blocks actual changes; transparent unchanged cells consume no authority.\n\
         - map_image_place does not seal the draft. Multiple photos and ordinary terrain patches may be applied in either order before one finalize.\n\
         - Report walkability and height changed-cell warnings returned by map_image_place in the final answer.\n\n\
         [trust boundary]\n\
         Original Apply and backup restore are intentionally absent from your tools. Only the user's trusted Map Agent window command can Apply or undo.\n\
         Never request, infer, or expose SCX/candidate filesystem paths; all access uses typed map tools.\n\n\
         [project state]\n{project_state}\n\n{}",
        project_memory.unwrap_or("[project memory]\n(none)"),
        operations_guide = crate::tools::map_draft_patch_operations_guide(),
        error_recovery = prompts::ERROR_RECOVERY_GUIDE.trim(),
        map_locations = prompts::MAP_LOCATION_GUIDE.trim(),
    )
}

/// Compatibility helper for focused prompt tests. Production turns use
/// [`crate::context_state::assemble_context`] and persist its delivery cursor.
pub fn build_system_prompt(
    request_text: &str,
    rag_hits: &[crate::rag::Hit],
    project_state: &str,
    project_memory: Option<&str>,
    wiki_facts: Option<&str>,
) -> String {
    let mut parts = vec![
        static_prompt_baseline(crate::provider::ProviderId::OpencodeGo),
        project_state_section(project_state),
    ];
    if let Some(guidance) =
        selected_task_guidance(&[request_text, project_state, project_memory.unwrap_or("")])
    {
        parts.push(format!("[task guidance]\n{guidance}"));
    }
    if let Some(memory) = project_memory_section(project_memory) {
        parts.push(memory);
    }
    if let Some(wiki) = wiki_facts_section(wiki_facts) {
        parts.push(wiki);
    }
    if !rag_hits.is_empty() {
        parts.push(reference_context_section(rag_hits));
    }
    parts.join("\n\n")
}

fn project_state_section(project_state: &str) -> String {
    let trimmed = project_state.trim();
    if trimmed.is_empty() {
        "[project state]\n(unavailable)".to_string()
    } else {
        trimmed.to_string()
    }
}

fn project_memory_section(project_memory: Option<&str>) -> Option<String> {
    let memory = project_memory?.trim();
    if memory.is_empty() {
        return None;
    }
    if memory.starts_with("[project memory]") {
        Some(memory.to_string())
    } else {
        Some(format!("[project memory]\n{memory}"))
    }
}

/// Normalize the dynamic `[wiki facts]` section: skipped when empty, and given the
/// `[wiki facts]` header if the provider passed a bare body (mirrors
/// [`project_memory_section`]). Placed BEFORE `[reference context]` so the agent's
/// last-applied dat values are reference facts, never a mutation trigger.
fn wiki_facts_section(wiki_facts: Option<&str>) -> Option<String> {
    let wiki = wiki_facts?.trim();
    if wiki.is_empty() {
        return None;
    }
    if wiki.starts_with("[wiki facts]") {
        Some(wiki.to_string())
    } else {
        Some(format!("[wiki facts]\n{wiki}"))
    }
}

fn reference_context_section(rag_hits: &[crate::rag::Hit]) -> String {
    let mut lines = vec!["[reference context]".to_string()];
    if rag_hits.is_empty() {
        lines.push("(no reference context available)".to_string());
    } else {
        for hit in rag_hits {
            lines.push(render_reference_hit(hit));
        }
    }
    lines.join("\n")
}

fn render_reference_hit(hit: &crate::rag::Hit) -> String {
    format!("--- source: {} ---\n{}", hit.source, hit.text)
}

fn next_request_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default();
    let value = nanos ^ COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("req-{value:08x}", value = value as u32)
}

pub(crate) fn next_run_id() -> u64 {
    static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed)
}

/// Paragraph break for the accumulated answer when an item boundary was seen:
/// codex streams each agent message as a separate thread item, so without a
/// break two messages would concatenate into one unbroken paragraph. No break
/// before the first message (empty accumulator).
fn take_chars(s: &str, limit: usize) -> String {
    s.chars().take(limit).collect()
}

/// Auto-derive a session title from the first user message (ChatGPT/codex style:
/// no manual naming). First non-empty line, trimmed and capped; the user can
/// rename later from the list.
fn auto_session_name(first_text: &str) -> String {
    const SESSION_NAME_CHARS: usize = 40;
    let first_line = first_text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if first_line.is_empty() {
        return "새 대화".to_string();
    }
    let name = take_chars(first_line, SESSION_NAME_CHARS);
    if first_line.chars().count() > SESSION_NAME_CHARS {
        format!("{name}…")
    } else {
        name
    }
}

/// Cap on the condensed replay transcript (chars), kept well under prompt limits.
const CONDENSED_TRANSCRIPT_CAP_CHARS: usize = 8000;

/// Run one git mutation off the async runtime, inside the project's write
/// transaction under `scope`.
///
/// The transaction keeps two sessions' commits from racing on the index lock
/// and from staging a file another session's transactional write is still
/// producing; `fs_*` and provider-native writes take no transaction, and the
/// path claims recorded before they run are what keep a commit off them. The
/// wait can be long — `build_run` holds the transaction for the whole
/// euddraft process — so it happens on a blocking thread, never a runtime
/// worker. No caller holds the transaction when it gets here: tool calls
/// release it before they return, and turn boundaries run outside any.
async fn run_git_mutation<T: Send + 'static>(
    writes: crate::write_coordinator::ProjectWriteCoordinator,
    scope: String,
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(move || {
        if scope.trim().is_empty() {
            return operation();
        }
        writes.transaction(&scope, operation)?
    })
    .await
    .map_err(|error| format!("git 작업이 중단되었습니다: {error}"))?
}

/// What one turn boundary recorded, and what the user should be told about it.
struct OwnedCommit {
    record: Option<crate::git::CommitRecord>,
    warning: Option<String>,
}

/// Commit what `request_id` changed and leave what other running turns of the
/// project changed to their own commits.
///
/// A dirty file only another in-flight request claimed stays uncommitted. A
/// file this request and another both claimed is committed here, with a
/// `shared-with:` trailer naming the other sessions and a warning, because
/// its bytes hold both turns' edits and no split is possible. With no other
/// request in flight everything dirty is committed, exactly as before.
#[cfg(test)]
fn commit_owned_paths(
    root: &std::path::Path,
    writes: &crate::write_coordinator::ProjectWriteCoordinator,
    project_id: &str,
    session_id: &str,
    request_id: &str,
    summary: &str,
) -> Result<OwnedCommit, String> {
    commit_owned_paths_with(
        root, writes, project_id, session_id, request_id, summary, "",
    )
}

/// [`commit_owned_paths`] with extra message trailers (a worker's
/// `parent-session:`), each a complete `key: value` line.
fn commit_owned_paths_with(
    root: &std::path::Path,
    writes: &crate::write_coordinator::ProjectWriteCoordinator,
    project_id: &str,
    session_id: &str,
    request_id: &str,
    summary: &str,
    trailer: &str,
) -> Result<OwnedCommit, String> {
    let committed = commit_claimed_paths(
        root, writes, project_id, session_id, request_id, summary, trailer,
    )?;
    // What this commit recorded is no longer in flight. Kept, it would hold
    // the file back from every other session's commit for the rest of the
    // request (an autonomous run commits once per iteration) and pin later
    // unclaimed edits to it on this request.
    if let Ok(dirty) = crate::git::dirty_paths(root) {
        let dirty: HashSet<String> = dirty
            .iter()
            .map(|path| crate::git::path_key(path))
            .collect();
        let done: Vec<String> = writes
            .claimed_by(request_id)
            .into_iter()
            .filter(|path| !dirty.contains(&crate::git::path_key(path)))
            .collect();
        writes.unclaim(request_id, &done);
    }
    Ok(committed)
}

fn commit_claimed_paths(
    root: &std::path::Path,
    writes: &crate::write_coordinator::ProjectWriteCoordinator,
    project_id: &str,
    session_id: &str,
    request_id: &str,
    summary: &str,
    trailer: &str,
) -> Result<OwnedCommit, String> {
    let mut message = crate::git::turn_message(summary, session_id, request_id);
    message.push_str(trailer);
    let others = writes.claimed_by_others(project_id, request_id);
    if others.is_empty() {
        return Ok(OwnedCommit {
            record: crate::git::commit_turn(root, &message)?,
            warning: None,
        });
    }
    let others: HashMap<String, &std::collections::BTreeSet<String>> = others
        .iter()
        .map(|(path, sessions)| (crate::git::path_key(path), sessions))
        .collect();
    let mine: HashSet<String> = writes
        .claimed_by(request_id)
        .iter()
        .map(|path| crate::git::path_key(path))
        .collect();
    let mut selected = Vec::new();
    let mut shared = Vec::new();
    let mut shared_with = std::collections::BTreeSet::new();
    for path in crate::git::dirty_paths(root)? {
        let key = crate::git::path_key(&path);
        match others.get(&key) {
            None => selected.push(path),
            Some(sessions) if mine.contains(&key) => {
                shared_with.extend(sessions.iter().cloned());
                shared.push(path.clone());
                selected.push(path);
            }
            Some(_) => {}
        }
    }
    let warning = (!shared.is_empty()).then(|| {
        message.push_str(&format!(
            "shared-with: {}\n",
            shared_with.into_iter().collect::<Vec<_>>().join(", ")
        ));
        format!(
            "다른 세션과 같은 파일을 함께 변경했습니다: {}",
            shared.join(", ")
        )
    });
    let record = crate::git::commit_paths(root, &message, &selected)?;
    Ok(OwnedCommit {
        warning: record.as_ref().and(warning),
        record,
    })
}

/// Parse the native project name out of a `[project state]` prompt render.
/// Returns `""` when absent or unavailable.
fn project_name_from_state(project_state: &str) -> String {
    let name = project_state
        .lines()
        .find_map(|line| line.trim().strip_prefix("project="))
        .unwrap_or("")
        .trim();
    if name.is_empty() || name == "(no project open)" {
        String::new()
    } else {
        name.to_string()
    }
}

/// Build a condensed prior-conversation transcript from the panel-owned `panelLog`
/// blob for the resume fallback (decision E). Keeps you/agent text and decision
/// lines, drops tool-arg dumps and transient detail, and caps the total well under
/// prompt limits. The `panelLog` is opaque to Rust, so this reads it defensively:
/// any missing/odd shape simply yields fewer lines (never panics).
fn condense_transcript(panel_log: &serde_json::Value) -> String {
    condense_transcript_with_cap(panel_log, CONDENSED_TRANSCRIPT_CAP_CHARS)
}

/// The durable panel log rendered for the model under a character bound.
///
/// The bound drops the OLDEST rows: a turn that continues a conversation needs
/// how it ended, and a head-first cut hands the model the opening of a long
/// session and none of the work it is being asked to continue.
fn condense_transcript_with_cap(panel_log: &serde_json::Value, cap: usize) -> String {
    let entries = panel_log
        .get("log")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut lines = Vec::new();
    for entry in &entries {
        let kind = entry.get("kind").and_then(serde_json::Value::as_str);
        let text = entry
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim();
        // Drop tool-arg dumps and transient/empty rows; keep conversational text
        // (you/agent) and terminal decision rows (ok/error/info).
        let label = match kind {
            Some("you") => "user",
            Some("agent") => "assistant",
            Some("ok") | Some("error") | Some("info") => "note",
            _ => continue,
        };
        if text.is_empty() {
            continue;
        }
        lines.push(format!("{label}: {}", take_chars(text, cap)));
    }
    if lines.is_empty() {
        return String::new();
    }

    let mut kept = std::collections::VecDeque::new();
    let mut used = 0_usize;
    for line in lines.iter().rev() {
        let length = line.chars().count().saturating_add(1);
        if !kept.is_empty() && used.saturating_add(length) > cap {
            break;
        }
        used = used.saturating_add(length);
        kept.push_front(line.as_str());
    }
    let mut out = String::from("[prior conversation]");
    if kept.len() < lines.len() {
        out.push_str("\n(앞부분 생략)");
    }
    for line in kept {
        out.push('\n');
        out.push_str(line);
    }
    out
}

/// The `ids` echoed back to the panel in `rollback_result`. A per-item decision
/// echoes the exact ids it targeted; a bulk (`all`) decision echoes EMPTY, which
/// the panel resolves against its OWN still-undecided item ids (a dat group's ids
/// live on its properties, NOT on a single group id — so the server must not echo
/// group ids here or dat groups would never mark as decided under bulk).
/// Lowercase family slug for the panel's dat type badge.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ProjectMemory;
    use crate::workspace::WorkspaceManager;
    use serde_json::{json, Value};
    use std::collections::VecDeque;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    #[test]
    fn session_event_flattens_payload_with_session_id() {
        let value = serde_json::to_value(SessionEvent {
            session_id: "session-a".to_string(),
            project_id: Some("project-a".to_string()),
            request_id: None,
            candidate_revision: None,
            payload: ipc::AnswerEvent {
                text: "done".to_string(),
            },
        })
        .expect("session event should serialize");
        assert_eq!(value["sessionId"], "session-a");
        // A window drops a session event of another project by this id.
        assert_eq!(value["projectId"], "project-a");
        assert_eq!(value["text"], "done");
    }

    fn sample_hits() -> Vec<crate::rag::Hit> {
        vec![crate::rag::Hit {
            id: 1,
            tier_level: 3,
            match_kind: crate::rag::MatchKind::Semantic,
            text: "RAG chunk about safe epscript practice".to_string(),
            source: "[ECA sample](https://example.test/edac/1)".to_string(),
            score: 0.92,
        }]
    }

    fn assembled_followup(
        user_text: &str,
        delivered_memory: Option<&str>,
        delivered_wiki: Option<&str>,
        current_memory: Option<&str>,
        current_wiki: Option<&str>,
    ) -> String {
        let baseline = static_prompt_baseline(crate::provider::ProviderId::OpencodeGo);
        let mut context = crate::context_state::SessionContextState::default();
        context.adopt_legacy_thread(
            &baseline,
            "thread-test".to_string(),
            delivered_memory,
            delivered_wiki,
            0,
        );
        crate::context_state::assemble_context(
            &context,
            crate::context_state::ContextAssemblyInput {
                static_baseline: &baseline,
                project_state: "[project state]\nproject=Sample",
                project_memory: current_memory,
                wiki_facts: current_wiki,
                project_map: None,
                reference_context: Some("[reference context]\nshould not repeat"),
                task_revision: 0,
                task_snapshot: "[active task state]\n{}",
                task_delta: None,
                replay_transcript: None,
                resolved_mentions: None,
                active_skill: None,
                task_guidance: None,
                user_text,
                provider: crate::provider::ProviderId::Codex,
                current_conversation_key: Some("thread-test"),
                force_full: false,
            },
        )
        .unwrap()
        .text
    }

    fn task_goal_event(
        turn_id: &str,
        request_id: &str,
        base_revision: u64,
        fact_id: &str,
        text: &str,
    ) -> crate::task_state::TaskStateEvent {
        crate::task_state::TaskStateEvent::new(
            Some(turn_id.to_string()),
            Some(request_id.to_string()),
            crate::task_state::TaskStateEventKind::SemanticDelta {
                delta: crate::task_state::TaskStateDelta {
                    base_revision,
                    operations: vec![crate::task_state::TaskStateOperation::Upsert {
                        entity: crate::task_state::TaskStateEntity::Goal {
                            fact: crate::task_state::StateFact {
                                id: fact_id.to_string(),
                                status: crate::task_state::FactStatus::Active,
                                text: text.to_string(),
                                provenance: vec![crate::task_state::Provenance::UserTurn {
                                    client_turn_id: turn_id.to_string(),
                                    exact_quote: text.to_string(),
                                }],
                            },
                        },
                    }],
                },
            },
        )
    }
    type ScriptedCompilerResults = Arc<Mutex<VecDeque<Result<Option<String>, AgentEngineError>>>>;
    type CompilerGate = (
        tokio::sync::mpsc::UnboundedSender<()>,
        Arc<tokio::sync::Notify>,
    );
    type ForegroundHook = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

    #[derive(Clone, Default)]
    pub(super) struct FakeCodexDriver {
        prompts: Arc<Mutex<Vec<String>>>,
        image_paths: Arc<Mutex<Vec<Vec<PathBuf>>>>,
        scripted_turns: Arc<Mutex<VecDeque<AgentTurnResult>>>,
        compiler_prompts: Arc<Mutex<Vec<String>>>,
        compiler_workspaces: Arc<Mutex<Vec<PathBuf>>>,
        scripted_compilers: ScriptedCompilerResults,
        compiler_contracts: Arc<Mutex<Vec<(bool, bool)>>>,
        compiler_delay: Arc<Mutex<Option<std::time::Duration>>>,
        compiler_gate: Arc<Mutex<Option<CompilerGate>>>,
        foreground_error: Arc<Mutex<Option<String>>>,
        reject_seed: Arc<Mutex<bool>>,
        accesses: Arc<Mutex<Vec<WorkspaceAccess>>>,
        native_tool_accesses: Arc<Mutex<Vec<bool>>>,
        /// When set, the next foreground run issues one real `ask` through the
        /// engine tool runtime before returning its scripted result.
        ask_runtime: Arc<Mutex<Option<SessionToolRuntime>>>,
        pub(super) plan_runtime: Option<SessionToolRuntime>,
        acknowledgement_count: Arc<Mutex<usize>>,
        /// The request whose last native run completed but was not yet
        /// acknowledged through `acknowledge_persisted`. The production
        /// runtime refuses to start that request again (its completed receipt
        /// would look like an unrecovered crash), so the fake does too.
        unacknowledged_request: Arc<Mutex<Option<String>>>,
        reset_count: Arc<Mutex<usize>>,
        /// The mock's live thread id; `reset_thread` clears it, `seed_thread_id`
        /// sets it, mirroring the production client's thread_id mutex.
        thread_id: Arc<Mutex<Option<String>>>,
        seeded: Arc<Mutex<Vec<String>>>,
        workspace: Arc<Mutex<Option<PreparedWorkspace>>>,
        /// When set, a foreground run whose identity is not the current
        /// generation ends cancelled before its first step, as
        /// `ProviderRuntime::foreground` does.
        honored_cancellation: Arc<Mutex<Option<tokio::sync::watch::Receiver<u64>>>>,
        /// Runs once when the next foreground run starts (a file the turn writes).
        before_foreground: ForegroundHook,
    }

    impl FakeCodexDriver {
        pub(super) fn scripted(turns: impl IntoIterator<Item = AgentTurnResult>) -> Self {
            Self {
                prompts: Arc::new(Mutex::new(Vec::new())),
                image_paths: Arc::new(Mutex::new(Vec::new())),
                scripted_turns: Arc::new(Mutex::new(turns.into_iter().collect())),
                compiler_prompts: Arc::new(Mutex::new(Vec::new())),
                compiler_workspaces: Arc::new(Mutex::new(Vec::new())),
                scripted_compilers: Arc::new(Mutex::new(VecDeque::new())),
                compiler_delay: Arc::new(Mutex::new(None)),
                compiler_gate: Arc::new(Mutex::new(None)),
                foreground_error: Arc::new(Mutex::new(None)),
                reject_seed: Arc::new(Mutex::new(false)),
                accesses: Arc::new(Mutex::new(Vec::new())),
                native_tool_accesses: Arc::new(Mutex::new(Vec::new())),
                ask_runtime: Arc::new(Mutex::new(None)),
                plan_runtime: None,
                acknowledgement_count: Arc::new(Mutex::new(0)),
                unacknowledged_request: Arc::new(Mutex::new(None)),

                compiler_contracts: Arc::new(Mutex::new(Vec::new())),
                reset_count: Arc::new(Mutex::new(0)),
                thread_id: Arc::new(Mutex::new(None)),
                seeded: Arc::new(Mutex::new(Vec::new())),
                workspace: Arc::new(Mutex::new(None)),
                honored_cancellation: Arc::new(Mutex::new(None)),
                before_foreground: Arc::new(Mutex::new(None)),
            }
        }

        fn honor_cancellation(&self, cancellation: tokio::sync::watch::Receiver<u64>) {
            *self.honored_cancellation.lock().expect("cancellation lock") = Some(cancellation);
        }

        fn before_next_foreground(&self, hook: impl FnOnce() + Send + 'static) {
            *self.before_foreground.lock().expect("hook lock") = Some(Box::new(hook));
        }

        pub(super) fn prompts(&self) -> Vec<String> {
            self.prompts.lock().expect("prompts lock").clone()
        }

        fn image_paths(&self) -> Vec<Vec<PathBuf>> {
            self.image_paths.lock().expect("image paths lock").clone()
        }

        fn seeded_ids(&self) -> Vec<String> {
            self.seeded.lock().expect("seeded lock").clone()
        }

        pub(super) fn set_workspace(&self, workspace: PreparedWorkspace) {
            *self.workspace.lock().expect("workspace lock") = Some(workspace);
        }

        pub(super) fn script_compilers(
            &self,
            outputs: impl IntoIterator<Item = Result<Option<String>, AgentEngineError>>,
        ) {
            self.scripted_compilers
                .lock()
                .expect("compiler queue lock")
                .extend(outputs);
        }

        fn delay_compiler(&self, delay: std::time::Duration) {
            *self.compiler_delay.lock().expect("compiler delay lock") = Some(delay);
        }

        fn gate_compiler(
            &self,
        ) -> (
            tokio::sync::mpsc::UnboundedReceiver<()>,
            Arc<tokio::sync::Notify>,
        ) {
            let (entered_tx, entered_rx) = tokio::sync::mpsc::unbounded_channel();
            let release = Arc::new(tokio::sync::Notify::new());
            *self.compiler_gate.lock().expect("compiler gate lock") =
                Some((entered_tx, Arc::clone(&release)));
            (entered_rx, release)
        }

        fn fail_next_foreground(&self, message: impl Into<String>) {
            *self.foreground_error.lock().expect("foreground error lock") = Some(message.into());
        }

        fn accesses(&self) -> Vec<WorkspaceAccess> {
            self.accesses.lock().expect("accesses lock").clone()
        }

        fn reject_next_seed(&self) {
            *self.reject_seed.lock().expect("reject seed lock") = true;
        }

        pub(super) fn compiler_prompts(&self) -> Vec<String> {
            self.compiler_prompts
                .lock()
                .expect("compiler prompts lock")
                .clone()
        }

        fn compiler_workspaces(&self) -> Vec<PathBuf> {
            self.compiler_workspaces
                .lock()
                .expect("compiler workspace lock")
                .clone()
        }

        fn compiler_contracts(&self) -> Vec<(bool, bool)> {
            self.compiler_contracts
                .lock()
                .expect("compiler contracts lock")
                .clone()
        }

        fn reset_count(&self) -> usize {
            *self.reset_count.lock().expect("reset count lock")
        }

        fn acknowledgement_count(&self) -> usize {
            *self
                .acknowledgement_count
                .lock()
                .expect("acknowledgement count lock")
        }
    }

    impl RuntimeExecutor for FakeCodexDriver {
        fn run_foreground(
            &mut self,
            request: ForegroundRequest,
        ) -> crate::provider_runtime::AdapterFuture<'_, RunOutcome> {
            Box::pin(async move {
                let runtime_session_id = request.identity.session_id.clone();
                let request_id = request.identity.request_id.clone();
                if self
                    .unacknowledged_request
                    .lock()
                    .expect("unacknowledged request lock")
                    .as_deref()
                    == Some(request_id.as_str())
                {
                    // Same refusal as `ProviderRuntime::resolve_native_conversation`.
                    return RunOutcome::Failed(
                        crate::provider_runtime::ProviderRuntimeError::Protocol(
                            "이미 완료된 요청의 저장 복구가 필요합니다. 같은 요청을 자동으로 재실행할 수 없습니다.".into(),
                        ),
                    );
                }
                let hook = self.before_foreground.lock().expect("hook lock").take();
                if let Some(hook) = hook {
                    hook();
                }
                let honored = self
                    .honored_cancellation
                    .lock()
                    .expect("cancellation lock")
                    .clone();
                if honored.is_some_and(|cancellation| {
                    *cancellation.borrow() != request.identity.cancellation_generation
                }) {
                    return RunOutcome::Cancelled;
                }
                let input = request.turn;
                self.native_tool_accesses
                    .lock()
                    .unwrap()
                    .push(input.native_tools_enabled);
                self.accesses
                    .lock()
                    .expect("accesses lock")
                    .push(input.workspace_access);
                self.prompts.lock().expect("prompts lock").push(input.text);
                self.image_paths
                    .lock()
                    .expect("image paths lock")
                    .push(input.image_paths);
                if let Some(message) = self
                    .foreground_error
                    .lock()
                    .expect("foreground error lock")
                    .take()
                {
                    return RunOutcome::Failed(
                        crate::provider_runtime::ProviderRuntimeError::Transport(message),
                    );
                }
                let ask_runtime = self.ask_runtime.lock().expect("ask runtime lock").take();
                if let Some(runtime) = ask_runtime {
                    // The test runtime carries a fixture session id, so the ask
                    // enters through the request-scoped path rather than the
                    // run-identity path; expiry handling is identical.
                    let outcome = runtime
                        .ask(&json!({
                            "questions": [{
                                "id": "mode",
                                "question": "방식을 고르세요.",
                                "options": [{"label": "A"}, {"label": "B"}]
                            }]
                        }))
                        .await
                        .expect("scripted ask must be admitted");
                    assert_eq!(outcome["status"], "unanswered");
                }
                {
                    let mut thread = self.thread_id.lock().expect("thread id lock");
                    if thread.is_none() {
                        *thread = Some("thread-fake".to_string());
                    }
                }
                {
                    let compiler_requested = !self
                        .scripted_compilers
                        .lock()
                        .expect("compiler queue lock")
                        .is_empty();
                    let mut workspace = self.workspace.lock().expect("workspace lock");
                    if workspace.is_none() && compiler_requested {
                        let root = unique_temp_dir("runtime-workspace");
                        let workspace_root = root.join(".eud-agent/workspace");
                        *workspace = Some(PreparedWorkspace {
                            id: "runtime-workspace".to_string(),
                            project: "Sample".to_string(),
                            temp_dir: workspace_root.join(".tmp"),
                            workspace_root,
                            root,
                            session_id: Some(runtime_session_id),
                        });
                    }
                }
                let result = self
                    .scripted_turns
                    .lock()
                    .expect("scripted turns lock")
                    .pop_front()
                    .expect("fake codex driver needs one scripted result per turn");
                if !matches!(result, AgentTurnResult::Cancelled) {
                    // A completed native run persists its receipt; only
                    // `acknowledge_persisted` retires it.
                    *self
                        .unacknowledged_request
                        .lock()
                        .expect("unacknowledged request lock") = Some(request_id);
                }
                match result {
                    AgentTurnResult::Answer { text } => RunOutcome::Completed {
                        text,
                        conversation: self.conversation_state(),
                    },
                    AgentTurnResult::Plan { markdown } => {
                        self.plan_runtime
                            .as_ref()
                            .expect("plan fixture must bind the engine tool runtime")
                            .execute("propose_plan", &json!({"markdown": markdown}))
                            .expect("scripted plan must pass the actual tool authority");
                        RunOutcome::Completed {
                            text: markdown,
                            conversation: self.conversation_state(),
                        }
                    }
                    AgentTurnResult::Cancelled => RunOutcome::Cancelled,
                    AgentTurnResult::WriteTransition => RunOutcome::WriteTransition,
                    AgentTurnResult::IterationBoundary { reason } => {
                        RunOutcome::IterationBoundary {
                            reason,
                            conversation: self.conversation_state(),
                        }
                    }
                }
            })
        }

        fn run_structured(
            &mut self,
            request: StructuredJobRequest,
        ) -> crate::provider_runtime::AdapterFuture<'_, RunOutcome> {
            Box::pin(async move {
                assert!(
                    request.workspace_root.is_dir(),
                    "compiler input cwd must exist"
                );
                let foreground_workspace = self.current_workspace();
                assert_ne!(
                    foreground_workspace
                        .as_ref()
                        .map(|workspace| &workspace.root),
                    Some(&request.workspace_root),
                    "compiler must not run in the foreground workspace"
                );
                assert!(
                    fs::read_dir(&request.workspace_root)
                        .expect("read compiler input cwd")
                        .next()
                        .is_none(),
                    "compiler input cwd must be empty"
                );
                self.compiler_workspaces
                    .lock()
                    .expect("compiler workspace lock")
                    .push(request.workspace_root.clone());
                self.compiler_contracts
                    .lock()
                    .expect("compiler contracts lock")
                    .push((
                        request.output_schema.is_object(),
                        request.policy.max_tool_rounds == 0
                            && !request.binding.conversation.is_started(),
                    ));
                self.compiler_prompts
                    .lock()
                    .expect("compiler prompts lock")
                    .push(request.prompt);
                let compiler_gate = self
                    .compiler_gate
                    .lock()
                    .expect("compiler gate lock")
                    .clone();
                if let Some((entered, release)) = compiler_gate {
                    let _ = entered.send(());
                    release.notified().await;
                }
                let delay = *self.compiler_delay.lock().expect("compiler delay lock");
                if delay.is_some() {
                    return RunOutcome::Failed(
                        crate::provider_runtime::ProviderRuntimeError::TimedOut,
                    );
                }
                match self
                    .scripted_compilers
                    .lock()
                    .expect("compiler queue lock")
                    .pop_front()
                    .unwrap_or(Ok(None))
                {
                    Ok(Some(output)) => match serde_json::from_str(&output) {
                        Ok(value) => RunOutcome::Structured {
                            value,
                            base: request.base,
                        },
                        Err(_) => RunOutcome::Structured {
                            value: serde_json::Value::String(output),
                            base: request.base,
                        },
                    },
                    Ok(None) => RunOutcome::Failed(
                        crate::provider_runtime::ProviderRuntimeError::Transport(
                            "structured result was not scripted".to_string(),
                        ),
                    ),
                    Err(error) => RunOutcome::Failed(
                        crate::provider_runtime::ProviderRuntimeError::Transport(error.to_string()),
                    ),
                }
            })
        }

        fn compact(
            &mut self,
            _request: CompactionRequest,
        ) -> crate::provider_runtime::AdapterFuture<
            '_,
            Result<
                crate::provider::ProviderConversationState,
                crate::provider_runtime::ProviderRuntimeError,
            >,
        > {
            Box::pin(async move {
                let thread_id = self.thread_id.lock().expect("thread id lock").clone();
                thread_id
                    .map(
                        |thread_id| crate::provider::ProviderConversationState::Codex {
                            thread_id: Some(thread_id),
                        },
                    )
                    .ok_or_else(|| {
                        crate::provider_runtime::ProviderRuntimeError::Protocol(
                            "no fake conversation".to_string(),
                        )
                    })
            })
        }

        fn reset(
            &mut self,
        ) -> crate::provider_runtime::AdapterFuture<
            '_,
            Result<(), crate::provider_runtime::ProviderRuntimeError>,
        > {
            Box::pin(async move {
                *self.reset_count.lock().expect("reset count lock") += 1;
                *self.thread_id.lock().expect("thread id lock") = None;
                Ok(())
            })
        }

        fn conversation_state(&self) -> crate::provider::ProviderConversationState {
            crate::provider::ProviderConversationState::Codex {
                thread_id: self.thread_id.lock().expect("thread id lock").clone(),
            }
        }

        fn acknowledge_persisted(
            &mut self,
        ) -> crate::provider_runtime::AdapterFuture<
            '_,
            Result<(), crate::provider_runtime::ProviderRuntimeError>,
        > {
            Box::pin(async move {
                *self
                    .acknowledgement_count
                    .lock()
                    .expect("acknowledgement count lock") += 1;
                *self
                    .unacknowledged_request
                    .lock()
                    .expect("unacknowledged request lock") = None;
                Ok(())
            })
        }

        fn seed(
            &mut self,
            state: crate::provider::ProviderConversationState,
        ) -> crate::provider_runtime::AdapterFuture<
            '_,
            Result<(), crate::provider_runtime::ProviderRuntimeError>,
        > {
            Box::pin(async move {
                if std::mem::take(&mut *self.reject_seed.lock().expect("reject seed lock")) {
                    return Err(crate::provider_runtime::ProviderRuntimeError::ContinuationInvalid);
                }
                let crate::provider::ProviderConversationState::Codex {
                    thread_id: Some(id),
                } = state
                else {
                    return Err(crate::provider_runtime::ProviderRuntimeError::Protocol(
                        "invalid fake conversation state".to_string(),
                    ));
                };
                self.seeded.lock().expect("seeded lock").push(id.clone());
                *self.thread_id.lock().expect("thread id lock") = Some(id);
                Ok(())
            })
        }

        fn current_workspace(&self) -> Option<PreparedWorkspace> {
            self.workspace.lock().expect("workspace lock").clone()
        }
    }

    #[derive(Clone)]
    struct GateCodexDriver {
        label: &'static str,
        entered: tokio::sync::mpsc::UnboundedSender<&'static str>,
        release: Arc<tokio::sync::Notify>,
        wait_once: Arc<std::sync::atomic::AtomicBool>,
        thread_id: Arc<Mutex<Option<String>>>,
    }

    impl GateCodexDriver {
        fn new(
            label: &'static str,
            entered: tokio::sync::mpsc::UnboundedSender<&'static str>,
            release: Arc<tokio::sync::Notify>,
            wait: bool,
        ) -> Self {
            Self {
                label,
                entered,
                release,
                wait_once: Arc::new(std::sync::atomic::AtomicBool::new(wait)),
                thread_id: Arc::new(Mutex::new(None)),
            }
        }
    }

    impl RuntimeExecutor for GateCodexDriver {
        fn run_foreground(
            &mut self,
            _request: ForegroundRequest,
        ) -> crate::provider_runtime::AdapterFuture<'_, RunOutcome> {
            Box::pin(async move {
                if self.entered.send(self.label).is_err() {
                    return RunOutcome::Failed(
                        crate::provider_runtime::ProviderRuntimeError::Transport(
                            "test entry receiver closed".to_string(),
                        ),
                    );
                }
                if self
                    .wait_once
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    self.release.notified().await;
                }
                *self.thread_id.lock().unwrap() = Some(format!("thread-{}", self.label));
                RunOutcome::Completed {
                    text: format!("{} done", self.label),
                    conversation: self.conversation_state(),
                }
            })
        }

        fn run_structured(
            &mut self,
            _request: StructuredJobRequest,
        ) -> crate::provider_runtime::AdapterFuture<'_, RunOutcome> {
            Box::pin(async {
                RunOutcome::Failed(crate::provider_runtime::ProviderRuntimeError::Protocol(
                    "gate runtime has no structured fixture".to_string(),
                ))
            })
        }

        fn compact(
            &mut self,
            _request: CompactionRequest,
        ) -> crate::provider_runtime::AdapterFuture<
            '_,
            Result<
                crate::provider::ProviderConversationState,
                crate::provider_runtime::ProviderRuntimeError,
            >,
        > {
            Box::pin(async move {
                self.thread_id
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|_| self.conversation_state())
                    .ok_or_else(|| {
                        crate::provider_runtime::ProviderRuntimeError::Protocol(
                            "no gate conversation".to_string(),
                        )
                    })
            })
        }

        fn reset(
            &mut self,
        ) -> crate::provider_runtime::AdapterFuture<
            '_,
            Result<(), crate::provider_runtime::ProviderRuntimeError>,
        > {
            Box::pin(async move {
                *self.thread_id.lock().unwrap() = None;
                Ok(())
            })
        }

        fn conversation_state(&self) -> crate::provider::ProviderConversationState {
            crate::provider::ProviderConversationState::Codex {
                thread_id: self.thread_id.lock().unwrap().clone(),
            }
        }

        fn seed(
            &mut self,
            state: crate::provider::ProviderConversationState,
        ) -> crate::provider_runtime::AdapterFuture<
            '_,
            Result<(), crate::provider_runtime::ProviderRuntimeError>,
        > {
            Box::pin(async move {
                let crate::provider::ProviderConversationState::Codex {
                    thread_id: Some(id),
                } = state
                else {
                    return Err(crate::provider_runtime::ProviderRuntimeError::Protocol(
                        "invalid gate conversation state".to_string(),
                    ));
                };
                *self.thread_id.lock().unwrap() = Some(id);
                Ok(())
            })
        }

        fn current_workspace(&self) -> Option<PreparedWorkspace> {
            None
        }
    }

    #[derive(Clone, Default)]
    pub(super) struct CapturingEventSink {
        events: Arc<Mutex<Vec<EngineEvent>>>,
    }

    impl CapturingEventSink {
        pub(super) fn events(&self) -> Vec<EngineEvent> {
            self.events.lock().expect("events lock").clone()
        }
    }

    impl EventSink for CapturingEventSink {
        fn emit(&self, event: EngineEvent) -> Result<(), AgentEngineError> {
            self.events.lock().expect("events lock").push(event);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct StoreMemoryProvider {
        memory: ProjectMemory,
    }

    impl StoreMemoryProvider {
        fn new(memory: ProjectMemory) -> Self {
            Self { memory }
        }
    }

    impl MemoryProvider for StoreMemoryProvider {
        fn render_section(&self) -> String {
            self.memory.render_section(None)
        }
    }

    /// A wiki provider backed by a file-backed [`crate::wiki::WikiStore`], so the
    /// accept-hook's write path is exercised end-to-end (load -> upsert -> save).
    #[derive(Clone)]
    struct StoreWikiProvider {
        wiki_dir: PathBuf,
    }

    impl WikiProvider for StoreWikiProvider {
        fn render_section(&self, query: &str) -> Option<String> {
            crate::wiki::WikiStore::load(Some(self.wiki_dir.clone())).render_section(query)
        }

        fn record_accepted(
            &self,
            entries: Vec<crate::wiki::LedgerEntry>,
        ) -> Option<ipc::WikiResponse> {
            let mut store = crate::wiki::WikiStore::load(Some(self.wiki_dir.clone()));
            if entries.is_empty() {
                return None;
            }
            for entry in entries {
                store.upsert(entry);
            }
            store.save().ok()?;
            Some(ipc::WikiResponse::from(store.ledger()))
        }
    }

    pub(super) fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-engine-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn memory_store(tag: &str) -> (PathBuf, ProjectMemory) {
        let base = unique_temp_dir(tag);
        let memory = ProjectMemory::new(base.join("memory"), "ExampleProject");
        (base, memory)
    }

    pub(super) fn native_workspace_snapshot(
        dirs: &crate::config::DataDirs,
        name: &str,
    ) -> crate::source_snapshot::ProjectSnapshot {
        let root = dirs.app_data().join("test-project");
        fs::create_dir_all(root.join("maps")).unwrap();
        fs::write(root.join("maps/source.scx"), b"map").unwrap();
        let project = crate::native_project::NativeProject::create(
            &root,
            crate::native_project::ProjectManifest {
                schema_version: crate::native_project::PROJECT_SCHEMA_VERSION,
                name: name.to_string(),
                source_map: "maps/source.scx".to_string(),
                output_map: "build/output.scx".to_string(),
                main_file: "src/main.eps".to_string(),
                settings: Default::default(),
                plugins: Vec::new(),
                python_entrypoints: Vec::new(),
                python_dependencies: Vec::new(),
                python_lock: None,
                editor_compatibility: None,
                e2_import: None,
            },
        )
        .unwrap();
        crate::native_runtime::NativeProjectSetup::new(dirs.clone())
            .activate_project(&project)
            .unwrap();
        crate::source_snapshot::ProjectSnapshot {
            project: name.to_string(),
            identity: project.root().to_string_lossy().into_owned(),
        }
    }

    /// `native_workspace_snapshot`, with `runtime` bound to that project the
    /// way a session's runtime is bound to its project's handle.
    pub(super) fn bind_workspace_snapshot(
        runtime: &SessionToolRuntime,
        name: &str,
    ) -> crate::source_snapshot::ProjectSnapshot {
        let snapshot = native_workspace_snapshot(&runtime.data_dirs(), name);
        let project =
            crate::native_project::NativeProject::open(std::path::Path::new(&snapshot.identity))
                .unwrap();
        runtime.bind_for_tests(&project);
        snapshot
    }

    fn config_with_memory(memory: ProjectMemory) -> AgentEngineConfig {
        AgentEngineConfig::for_tests(
            "[project state]\nproject=Sample compiling=false",
            None,
            sample_hits(),
        )
        .with_memory_provider(Arc::new(StoreMemoryProvider::new(memory)))
    }

    /// A SessionStore rooted under `data_dir/sessions` so engine tests that wire a
    /// real on-disk journal also get a real on-disk session store beside it.
    fn session_store_at(data_dir: &std::path::Path) -> crate::session::SessionStore {
        let dirs = crate::config::DataDirs::from_bases(data_dir, data_dir);
        dirs.ensure_dirs().unwrap();
        crate::session::SessionStore::new(&dirs)
    }

    pub(super) fn attachment_store_at(data_dir: &std::path::Path) -> AttachmentStore {
        AttachmentStore::new(data_dir.join("attachments"))
    }

    fn test_session(store: &crate::session::SessionStore) -> crate::session::SessionRecord {
        test_session_of(store, "Sample")
    }

    fn test_session_of(
        store: &crate::session::SessionStore,
        project_id: &str,
    ) -> crate::session::SessionRecord {
        let created_at = crate::session::now_unix_seconds();
        let record = crate::session::SessionRecord {
            meta: crate::session::SessionMeta {
                id: crate::session::new_session_id(),
                name: "test session".to_string(),
                project: "Sample".to_string(),
                kind: crate::session::SessionKind::Eps,
                provider: crate::provider::ProviderId::Codex,
                model: "gpt-test".to_string(),
                created_at,
                last_conversation_at: crate::session::now_unix_millis(),
                team_parent: None,
                worker_parent: None,
                project_id: Some(project_id.to_string()),
                workspace_id: None,
            },
            provider_binding: crate::provider::ProviderBinding::new(
                crate::provider::ProviderId::Codex,
                "gpt-test".to_string(),
                Some(crate::provider::ReasoningSelection {
                    level: "medium".to_string(),
                }),
            )
            .unwrap(),
            pending_request_ids: Vec::new(),
            context_usage: None,
            panel_log: serde_json::Value::Null,
            context_state: Default::default(),
            task_state: Default::default(),
            autonomous_run: None,
            team_tasks: Vec::new(),
            workers: Vec::new(),
        };
        store.save(&record).unwrap();
        record
    }

    fn test_engine_with_memory<R: RuntimeExecutor, S: EventSink>(
        executor: R,
        sink: S,
        memory: ProjectMemory,
        data_dir: &std::path::Path,
    ) -> AgentEngine<R, S> {
        let sessions = session_store_at(data_dir);
        let session = test_session(&sessions);
        let (_cancellation_tx, cancellation_rx) = tokio::sync::watch::channel(0_u64);
        let mut engine = AgentEngine::new(
            executor,
            sink,
            config_with_memory(memory),
            test_tool_runtime(),
            sessions,
            attachment_store_at(data_dir),
            session,
            cancellation_rx,
        );
        engine.journal_store = journal::JournalStore::new(data_dir);
        engine.journal_data_dir = data_dir.to_path_buf();
        engine
    }

    fn record_file_write(
        store: &journal::JournalStore,
        request_id: &str,
        id: &str,
        seq: u64,
        path: &str,
    ) {
        record_file_write_in_memory(store, request_id, id, seq, path);
        store
            .persist(request_id)
            .expect("journal entry should persist");
    }

    pub(super) fn record_file_write_in_memory(
        store: &journal::JournalStore,
        request_id: &str,
        id: &str,
        seq: u64,
        path: &str,
    ) {
        store
            .record(
                request_id,
                journal::JournalEntry {
                    id: id.to_string(),
                    seq,
                    tool: journal::WriteTool::FileWrite,
                    target: journal::JournalTarget::Path {
                        path: path.to_string(),
                    },
                    before: journal::Snapshot::FileContent {
                        content: "old\n".to_string(),
                    },
                    after: journal::Snapshot::FileContent {
                        content: "new\n".to_string(),
                    },
                    ts: 1,
                },
            )
            .expect("journal entry should record");
    }

    /// `target` is the `(dat, objId, property)` tuple of a units/weapons/... dat edit.
    fn record_dat_set_in_memory(
        store: &journal::JournalStore,
        request_id: &str,
        id: &str,
        seq: u64,
        target: (&str, u32, &str),
        new: Value,
    ) {
        let (dat, obj_id, property) = target;
        store
            .record(
                request_id,
                journal::JournalEntry {
                    id: id.to_string(),
                    seq,
                    tool: journal::WriteTool::DatSet,
                    target: journal::JournalTarget::Dat {
                        table: journal::DatTable::Dat,
                        dat: dat.to_string(),
                        obj_id,
                        property: property.to_string(),
                    },
                    before: journal::Snapshot::DatValue {
                        value: Value::Null,
                        was_default: true,
                    },
                    after: journal::Snapshot::DatValue {
                        value: new,
                        was_default: false,
                    },
                    ts: 1_718_000_000 + seq,
                },
            )
            .expect("dat journal entry should record");
    }

    /// Build an engine wired with BOTH a memory provider and a file-backed wiki
    /// provider rooted at `wiki_dir`, sharing the on-disk journal at `data_dir`.
    fn test_engine_with_wiki<R: RuntimeExecutor, S: EventSink>(
        executor: R,
        sink: S,
        memory: ProjectMemory,
        data_dir: &std::path::Path,
        wiki_dir: &std::path::Path,
    ) -> AgentEngine<R, S> {
        let config = config_with_memory(memory).with_wiki_provider(Arc::new(StoreWikiProvider {
            wiki_dir: wiki_dir.to_path_buf(),
        }));
        let sessions = session_store_at(data_dir);
        let session = test_session(&sessions);
        let (_cancellation_tx, cancellation_rx) = tokio::sync::watch::channel(0_u64);
        let mut engine = AgentEngine::new(
            executor,
            sink,
            config,
            test_tool_runtime(),
            sessions,
            attachment_store_at(data_dir),
            session,
            cancellation_rx,
        );
        engine.journal_store = journal::JournalStore::new(data_dir);
        engine.journal_data_dir = data_dir.to_path_buf();
        engine
    }

    /// A session tool runtime with an activated native project, so stage jobs
    /// and approvals can prepare the session workspace as production does.
    fn test_tool_runtime() -> SessionToolRuntime {
        let runtime = SessionToolRuntime::for_tests();
        let dirs = runtime.data_dirs();
        dirs.ensure_dirs().expect("test data dirs");
        // A separate root from `native_workspace_snapshot`, which tests may
        // still call to activate their own fixture project afterwards.
        let root = dirs.app_data().join("base-project");
        fs::create_dir_all(root.join("maps")).unwrap();
        fs::write(root.join("maps/source.scx"), b"map").unwrap();
        let project = crate::native_project::NativeProject::create(
            &root,
            crate::native_project::ProjectManifest {
                schema_version: crate::native_project::PROJECT_SCHEMA_VERSION,
                name: "Sample".to_string(),
                source_map: "maps/source.scx".to_string(),
                output_map: "build/output.scx".to_string(),
                main_file: "src/main.eps".to_string(),
                settings: Default::default(),
                plugins: Vec::new(),
                python_entrypoints: Vec::new(),
                python_dependencies: Vec::new(),
                python_lock: None,
                editor_compatibility: None,
                e2_import: None,
            },
        )
        .unwrap();
        crate::native_runtime::NativeProjectSetup::new(dirs)
            .activate_project(&project)
            .unwrap();
        runtime.bind_for_tests(&project);
        runtime
    }

    pub(super) fn test_engine<R: RuntimeExecutor, S: EventSink>(
        executor: R,
        sink: S,
    ) -> AgentEngine<R, S> {
        test_engine_on(executor, sink, test_tool_runtime())
    }

    fn test_engine_on<R: RuntimeExecutor, S: EventSink>(
        executor: R,
        sink: S,
        runtime: SessionToolRuntime,
    ) -> AgentEngine<R, S> {
        let data_dir = unique_temp_dir("engine-sessions");
        let sessions = session_store_at(&data_dir);
        let session = test_session(&sessions);
        let (_cancellation_tx, cancellation_rx) = tokio::sync::watch::channel(0_u64);
        AgentEngine::new(
            executor,
            sink,
            AgentEngineConfig::for_tests(
                "[project state]\nproject=Sample compiling=false",
                None,
                sample_hits(),
            ),
            runtime,
            sessions,
            attachment_store_at(&unique_temp_dir("engine-attachments")),
            session,
            cancellation_rx,
        )
    }

    #[tokio::test]
    async fn resumed_foreground_failure_does_not_reset_or_replay_completed_work() {
        let driver = FakeCodexDriver::scripted([]);
        driver.fail_next_foreground("resume failed after durable tool completion");
        *driver.thread_id.lock().expect("thread id lock") = Some("thread-saved".to_string());
        let driver_handle = driver.clone();
        let mut engine = test_engine(driver, CapturingEventSink::default());
        engine.thread_active = true;
        engine.pending_resume_transcript = Some("prior transcript".to_string());
        record_file_write_in_memory(
            &engine.journal_store,
            "req-resume-failure",
            "completed-call",
            1,
            "triggers/main.eps",
        );

        let error = engine
            .chat_with_request_id(
                crate::ipc::ChatRequest {
                    client_turn_id: crate::ipc::new_client_turn_id(),
                    text: "continue".to_string(),
                    attachments: Vec::new(),
                    mentions: Vec::new(),
                    execution_mode: Default::default(),
                    autonomous_policy: None,
                    clarify: false,
                    skill_selections: Default::default(),
                },
                Some("req-resume-failure".to_string()),
            )
            .await
            .expect_err("resume transport failure must remain a failure");

        assert!(error.message.contains("resume failed"));
        assert_eq!(driver_handle.prompts().len(), 1);
        assert_eq!(driver_handle.reset_count(), 0);
        assert_eq!(
            engine
                .journal_store
                .changeset("req-resume-failure")
                .expect("completed tool journal remains reviewable")
                .items
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn a_cancelled_turn_is_recorded_apart_from_an_answered_one() {
        // `map_chat` reports `MapTurnEnd` from this record: a cancelled turn
        // is not an error, so without it a stopped team Map request would
        // settle as a failure and continue the EPS conversation.
        let driver = FakeCodexDriver::scripted([
            AgentTurnResult::Cancelled,
            AgentTurnResult::Answer {
                text: "Done.".to_string(),
            },
        ]);
        let mut engine = test_engine(driver, CapturingEventSink::default());
        let request = |text: &str| crate::ipc::ChatRequest {
            client_turn_id: crate::ipc::new_client_turn_id(),
            text: text.to_string(),
            attachments: Vec::new(),
            mentions: Vec::new(),
            execution_mode: Default::default(),
            autonomous_policy: None,
            clarify: false,
            skill_selections: Default::default(),
        };

        engine
            .chat_with_request_id(request("stop me"), Some("req-cancelled".to_string()))
            .await
            .unwrap();
        assert!(engine.last_turn_cancelled);
        engine
            .chat_with_request_id(request("again"), Some("req-answered".to_string()))
            .await
            .unwrap();
        assert!(!engine.last_turn_cancelled);
    }

    #[test]
    fn production_runtime_sink_emits_completed_and_failed_tool_statuses() {
        let data_dir = unique_temp_dir("runtime-tool-status");
        let sessions = session_store_at(&data_dir);
        let record = test_session(&sessions);
        let events = CapturingEventSink::default();
        let sink =
            runtime_events::SessionRuntimeEventSink::new(events.clone(), sessions, record.meta.id);
        for is_error in [false, true] {
            crate::provider_runtime::RuntimeEventSink::emit(
                &sink,
                &AdapterEventKind::Block(NormalizedBlock::ToolResult {
                    response_id: "tool-response".to_string(),
                    batch_id: "tool-batch".to_string(),
                    result: crate::provider_tool_loop::DirectToolResult {
                        id: format!("tool-{is_error}"),
                        name: "read_file".to_string(),
                        result: json!({"ok": !is_error}),
                        is_error,
                    },
                }),
            )
            .expect("publish actual runtime tool result");
        }
        let statuses = events
            .events()
            .into_iter()
            .map(|event| match event {
                EngineEvent::Agent(ipc::AgentEvent {
                    kind,
                    data: Some(data),
                    ..
                }) => {
                    assert_eq!(kind, "tool_result");
                    data.status.expect("tool result status")
                }
                other => panic!("unexpected tool sink event: {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(statuses, ["completed", "failed"]);
        fs::remove_dir_all(data_dir).expect("clean tool sink fixture");
    }

    #[test]
    fn production_runtime_sink_serializes_tool_call_identity() {
        // Given: the production session sink receives an authoritative tool pair.
        let data_dir = unique_temp_dir("runtime-tool-identity");
        let sessions = session_store_at(&data_dir);
        let record = test_session(&sessions);
        let events = CapturingEventSink::default();
        let sink =
            runtime_events::SessionRuntimeEventSink::new(events.clone(), sessions, record.meta.id);
        for event in [
            AdapterEventKind::Block(NormalizedBlock::ToolCall {
                response_id: "tool-response".to_string(),
                batch_id: "tool-batch".to_string(),
                call: crate::provider_tool_loop::DirectToolCall {
                    id: "call-a".to_string(),
                    name: "read_file".to_string(),
                    arguments: json!({"path":"a.eps"}),
                },
                continuation: None,
            }),
            AdapterEventKind::Block(NormalizedBlock::ToolResult {
                response_id: "tool-response".to_string(),
                batch_id: "tool-batch".to_string(),
                result: crate::provider_tool_loop::DirectToolResult {
                    id: "call-a".to_string(),
                    name: "read_file".to_string(),
                    result: json!({"contents":"alpha"}),
                    is_error: false,
                },
            }),
        ] {
            crate::provider_runtime::RuntimeEventSink::emit(&sink, &event)
                .expect("publish actual runtime tool event");
        }

        // When: the actual emitted AgentEvent payloads cross the serde boundary.
        let serialized = events
            .events()
            .into_iter()
            .map(|event| match event {
                EngineEvent::Agent(agent) => {
                    serde_json::to_value(agent).expect("serialize production runtime AgentEvent")
                }
                other => panic!("unexpected tool sink event: {other:?}"),
            })
            .collect::<Vec<_>>();

        // Then: both sides carry the same call identity in IPC camelCase form.
        assert_eq!(serialized[0]["data"]["callId"], "call-a");
        assert_eq!(serialized[1]["data"]["callId"], "call-a");
        fs::remove_dir_all(data_dir).expect("clean tool identity sink fixture");
    }

    #[test]
    fn production_runtime_sink_persists_exact_context_usage_and_emits_typed_event() {
        let data_dir = unique_temp_dir("runtime-context-usage");
        let sessions = session_store_at(&data_dir);
        let mut record = test_session(&sessions);
        record.panel_log = json!({"schemaVersion":2,"logSeq":1,"log":[{"text":"preserved"}]});
        record.pending_request_ids = vec!["review-preserved".to_string()];
        sessions
            .save(&record)
            .expect("save unrelated session state");
        let events = CapturingEventSink::default();
        let sink = runtime_events::SessionRuntimeEventSink::new(
            events.clone(),
            sessions.clone(),
            record.meta.id.clone(),
        );
        crate::provider_runtime::RuntimeEventSink::emit(
            &sink,
            &AdapterEventKind::ResponseStarted {
                response_id: "usage-response".to_string(),
            },
        )
        .expect("start actual runtime response");
        let expected = ipc::ContextUsage {
            last: ipc::TokenUsageBreakdown {
                input_tokens: 100,
                cached_input_tokens: 20,
                cache_write_input_tokens: 5,
                output_tokens: 30,
                reasoning_output_tokens: 7,
                total_tokens: 130,
            },
            total: ipc::TokenUsageBreakdown {
                input_tokens: 700,
                cached_input_tokens: 140,
                cache_write_input_tokens: 35,
                output_tokens: 210,
                reasoning_output_tokens: 49,
                total_tokens: 910,
            },
            model_context_window: Some(2048),
        };
        let event = AdapterEventKind::Usage(crate::provider_runtime::NormalizedUsage {
            input_tokens: Some(100),
            cached_input_tokens: Some(20),
            output_tokens: Some(30),
            total_tokens: Some(130),
            provider_details: None,
            context_usage: Some(expected.clone()),
        });
        crate::provider_runtime::RuntimeEventSink::emit(&sink, &event)
            .expect("publish typed usage");
        let restored = sessions
            .load(&record.meta.id)
            .expect("reload persisted usage");
        assert_eq!(restored.context_usage, Some(expected.clone()));
        assert_eq!(restored.panel_log, record.panel_log);
        assert_eq!(restored.pending_request_ids, record.pending_request_ids);
        assert!(
            matches!(events.events().last(), Some(EngineEvent::ContextUsage(usage))
            if usage.turn_id == "usage-response" && usage.token_usage == expected)
        );
        assert!(!events.events().iter().any(|event| matches!(event,
            EngineEvent::Agent(agent) if agent.kind == "usage")));
        sessions
            .delete(&record.meta.id)
            .expect("simulate stats persistence failure");
        crate::provider_runtime::RuntimeEventSink::emit(&sink, &event)
            .expect("stats persistence failure must not fail foreground response");
        assert!(
            matches!(events.events().last(), Some(EngineEvent::ContextUsage(usage))
            if usage.token_usage == expected)
        );
        fs::remove_dir_all(data_dir).expect("clean usage sink fixture");
    }

    #[test]
    fn production_runtime_sink_maps_builtin_start_and_terminal_statuses() {
        let data_dir = unique_temp_dir("runtime-builtin-observation");
        let sessions = session_store_at(&data_dir);
        let record = test_session(&sessions);
        let events = CapturingEventSink::default();
        let sink =
            runtime_events::SessionRuntimeEventSink::new(events.clone(), sessions, record.meta.id);
        let observation =
            |arguments, result, status: &str| AdapterEventKind::NativeToolObservation {
                call_id: Some("builtin-call".to_string()),
                mcp_server: None,
                name: "web_search".to_string(),
                arguments,
                result,
                status: Some(status.to_string()),
            };
        for event in [
            observation(Some(json!({"query":"first"})), None, "started"),
            observation(None, Some(json!("found")), "completed"),
            observation(Some(json!({"query":"empty-result"})), None, "started"),
            observation(None, None, "completed"),
            observation(Some(json!({"query":"failed"})), None, "started"),
            observation(None, Some(json!("denied")), "failed"),
            observation(Some(json!({"query":"declined"})), None, "started"),
            observation(None, None, "declined"),
        ] {
            crate::provider_runtime::RuntimeEventSink::emit(&sink, &event)
                .expect("publish visible builtin observation");
        }
        assert!(matches!(
            crate::provider_runtime::RuntimeEventSink::emit(
                &sink,
                &observation(
                    Some(json!({"query":"malformed"})),
                    None,
                    "argument_complete"
                )
            ),
            Err(crate::provider_runtime::ProviderRuntimeError::Protocol(_))
        ));
        let rows = events
            .events()
            .into_iter()
            .map(|event| match event {
                EngineEvent::Agent(ipc::AgentEvent {
                    kind,
                    detail,
                    data: Some(data),
                }) => {
                    assert_eq!(detail, "web_search");
                    (kind, data)
                }
                other => panic!("unexpected builtin event: {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rows.iter().map(|row| row.0.as_str()).collect::<Vec<_>>(),
            [
                "tool_call",
                "tool_result",
                "tool_call",
                "tool_result",
                "tool_call",
                "tool_result",
                "tool_call",
                "tool_result"
            ]
        );
        assert_eq!(rows[1].1.status.as_deref(), Some("completed"));
        assert_eq!(rows[3].1.status.as_deref(), Some("completed"));
        assert_eq!(rows[3].1.result, None);
        assert_eq!(rows[5].1.status.as_deref(), Some("failed"));
        assert_eq!(rows[7].1.status.as_deref(), Some("declined"));
        assert_eq!(rows[2].1.status, None);
        assert_eq!(
            rows[2].1.args.as_deref(),
            Some("{\"query\":\"empty-result\"}")
        );
        fs::remove_dir_all(data_dir).expect("clean builtin sink fixture");
    }

    #[tokio::test]
    async fn eps_chat_turn_writes_from_its_first_call_and_settles_itself() {
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "changed".to_string(),
        }]);
        let driver_handle = driver.clone();
        let sink = CapturingEventSink::default();
        let sink_handle = sink.clone();
        let mut engine = test_engine(driver, sink);

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "change the project".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .expect("a write turn answers in one foreground run");

        assert_eq!(driver_handle.accesses(), vec![WorkspaceAccess::Write]);
        assert_eq!(engine.pending_write, None);
        assert!(engine.runtime.write_ticket().is_none());
        assert!(sink_handle
            .events()
            .iter()
            .any(|event| matches!(event, EngineEvent::Answer(_))));
    }

    #[tokio::test]
    async fn investigation_worker_turns_keep_read_access_and_disable_native_tools() {
        for profile in [
            crate::worker::WorkerProfile::Research,
            crate::worker::WorkerProfile::MapResearch,
        ] {
            let driver = FakeCodexDriver::scripted([
                AgentTurnResult::Answer {
                    text: "evidence".into(),
                },
                AgentTurnResult::Answer {
                    text: "more evidence".into(),
                },
            ]);
            let handle = driver.clone();
            let mut engine = test_engine(driver, CapturingEventSink::default());
            engine.worker_origin = Some(("parent".into(), "Research".into()));
            engine.runtime.set_worker_parent(Some("parent".into()));
            engine.runtime.set_worker_profile(profile);
            engine
                .chat(plain_request("Investigate the relevant facts"))
                .await
                .unwrap();
            engine
                .chat(plain_request("Read another relevant fact"))
                .await
                .unwrap();
            assert_eq!(
                handle.accesses(),
                vec![WorkspaceAccess::Read, WorkspaceAccess::Read]
            );
            assert_eq!(
                *handle.native_tool_accesses.lock().unwrap(),
                vec![false, false]
            );
            assert!(engine.runtime.write_ticket().is_none());
        }
    }

    #[tokio::test]
    async fn provider_receipt_acknowledgement_requires_session_persistence() {
        let persisted_driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "saved".to_string(),
        }]);
        let persisted_handle = persisted_driver.clone();
        let mut persisted_engine = test_engine(persisted_driver, CapturingEventSink::default());
        persisted_engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "save this turn".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .expect("foreground turn should persist");
        assert_eq!(persisted_handle.acknowledgement_count(), 1);

        let failed_driver = FakeCodexDriver::scripted([]);
        let failed_handle = failed_driver.clone();
        let mut failed_engine = test_engine(failed_driver, CapturingEventSink::default());
        failed_engine
            .session_store
            .delete(&failed_engine.session_id)
            .expect("session fixture should delete");
        failed_engine.update_active_session().await;
        assert_eq!(failed_handle.acknowledgement_count(), 0);
    }

    #[test]
    fn a_bounded_transcript_keeps_the_newest_rows_and_marks_the_cut() {
        let log = serde_json::json!({
            "schemaVersion": 2,
            "log": (1..=40)
                .map(|index| serde_json::json!({
                    "seq": index,
                    "kind": if index % 2 == 0 { "agent" } else { "you" },
                    "text": format!("message {index:02} 내용")
                }))
                .collect::<Vec<_>>()
        });

        let condensed = super::condense_transcript_with_cap(&log, 120);

        // A turn that continues a conversation needs how it ended. A head-first
        // cut would hand the model the opening of a long session and none of
        // the work it is being asked to continue.
        assert!(condensed.starts_with("[prior conversation]\n(앞부분 생략)"));
        assert!(condensed.contains("message 40"));
        assert!(!condensed.contains("message 01"));
        assert!(condensed.chars().count() <= 240);
    }

    #[test]
    fn an_empty_or_toolonly_panel_log_condenses_to_nothing() {
        assert!(super::condense_transcript(&serde_json::json!({"log": []})).is_empty());
        assert!(super::condense_transcript(&serde_json::json!({
            "log": [{"kind": "tool", "text": "read_file"}, {"kind": "you", "text": "   "}]
        }))
        .is_empty());
    }

    #[tokio::test]
    async fn invalid_persisted_continuation_preserves_review_without_replay() {
        let driver = FakeCodexDriver::scripted([]);
        driver.reject_next_seed();
        let driver_handle = driver.clone();
        let mut engine = test_engine(driver, CapturingEventSink::default());
        let request_id = "req-invalid-continuation";
        let mut record = engine
            .session_store
            .load(&engine.session_id)
            .expect("session fixture should load");
        record.provider_binding.conversation = crate::provider::ProviderConversationState::Codex {
            thread_id: Some("thread-unsafe".to_string()),
        };
        record.pending_request_ids = vec![request_id.to_string()];
        record.panel_log = serde_json::json!({
            "schemaVersion": 2,
            "logSeq": 1,
            "log": [{"seq": 1, "kind": "user", "text": "previous request"}]
        });
        engine
            .session_store
            .save(&record)
            .expect("session fixture should save");
        record_file_write(
            &engine.journal_store,
            request_id,
            "completed-call",
            1,
            "triggers/main.eps",
        );
        engine.journal_store = journal::JournalStore::new(&engine.journal_data_dir);

        let error = engine
            .hydrate()
            .await
            .expect_err("invalid continuation must block automatic replay");

        assert!(error.message.contains("could not be resumed"));
        assert_eq!(driver_handle.reset_count(), 0);
        assert!(driver_handle.prompts().is_empty());
        assert!(engine.pending_resume_transcript.is_none());
        assert!(engine.ensure_provider_conversation_ready().is_err());
        // The completed tool's journal survives the refusal, so the work it did
        // is still on record even though the conversation cannot continue.
        assert_eq!(
            engine
                .journal_store
                .changeset(request_id)
                .expect("a completed tool keeps its journal")
                .items
                .len(),
            1
        );
        assert_eq!(
            engine
                .session_store
                .load(&engine.session_id)
                .expect("saved session remains present")
                .provider_binding
                .conversation,
            record.provider_binding.conversation
        );
    }

    #[tokio::test]
    async fn explicit_conversation_reset_clears_failed_resume_without_replay() {
        let driver = FakeCodexDriver::scripted([]);
        driver.reject_next_seed();
        let driver_handle = driver.clone();
        let mut engine = test_engine(driver, CapturingEventSink::default());
        let mut record = engine
            .session_store
            .load(&engine.session_id)
            .expect("session fixture should load");
        record.provider_binding.conversation = crate::provider::ProviderConversationState::Codex {
            thread_id: Some("thread-unsafe".to_string()),
        };
        record.panel_log = serde_json::json!({
            "schemaVersion": 2,
            "logSeq": 1,
            "log": [{"seq": 1, "kind": "you", "text": "earlier map request"}]
        });
        engine
            .session_store
            .save(&record)
            .expect("session fixture should save");

        engine
            .hydrate()
            .await
            .expect_err("unresumable conversation blocks the session");
        assert!(engine.conversation_resume_error().is_some());

        engine
            .reset_conversation()
            .await
            .expect("explicit reset starts a fresh conversation");

        assert!(engine.conversation_resume_error().is_none());
        assert!(engine.ensure_provider_conversation_ready().is_ok());
        assert_eq!(driver_handle.reset_count(), 1);
        assert!(driver_handle.prompts().is_empty());
        assert!(!engine.thread_active);
        let saved = engine
            .session_store
            .load(&engine.session_id)
            .expect("saved session remains present");
        assert_eq!(
            saved.provider_binding.conversation,
            crate::provider::ProviderConversationState::Codex { thread_id: None }
        );
        assert_eq!(saved.panel_log, record.panel_log);
        assert!(engine
            .pending_resume_transcript
            .as_deref()
            .is_some_and(|transcript| transcript.contains("earlier map request")));
    }

    #[tokio::test]
    async fn session_bound_engines_keep_independent_thread_prompt_state() {
        let driver_a = FakeCodexDriver::scripted([
            AgentTurnResult::Answer {
                text: "First answer.".to_string(),
            },
            AgentTurnResult::Answer {
                text: "Second answer.".to_string(),
            },
        ]);
        let driver_b = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "Fresh answer.".to_string(),
        }]);
        let handle_a = driver_a.clone();
        let handle_b = driver_b.clone();
        let mut engine_a = test_engine(driver_a, CapturingEventSink::default());
        let mut engine_b = test_engine(driver_b, CapturingEventSink::default());

        engine_a
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "first user message".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        engine_a
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "follow-up user message".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        engine_b
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "fresh user message".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();

        let prompts_a = handle_a.prompts();
        let prompts_b = handle_b.prompts();
        assert!(prompts_a[0].contains("[first principles]"));
        assert!(prompts_a[0].lines().any(|line| line == "[user message]"));
        assert!(prompts_a[1].lines().any(|line| line == "[user message]"));
        assert!(!prompts_a[1].contains("[first principles]"));
        assert!(prompts_b[0].contains("[first principles]"));
        assert!(prompts_b[0].lines().any(|line| line == "[user message]"));
    }

    #[tokio::test]
    async fn agentic_engine_routes_answer_only_and_propose_plan_turns_to_v2_events() {
        let driver = FakeCodexDriver::scripted([
            AgentTurnResult::Answer {
                text: "No edits are needed.".to_string(),
            },
            AgentTurnResult::Plan {
                markdown: "- Search docs\n- Apply the change\n- Build".to_string(),
            },
        ]);
        let sink = CapturingEventSink::default();
        let sink_handle = sink.clone();
        let mut engine = test_engine(driver, sink);
        engine.executor.plan_runtime = Some(engine.runtime.clone());

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "Explain the current behavior.".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .expect("answer-only turn should run");
        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "Make a larger change.".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .expect("propose_plan turn should run");

        let events = sink_handle.events();
        assert!(
            matches!(
                events.as_slice(),
                [
                    EngineEvent::Answer(crate::ipc::AnswerEvent { text }),
                    EngineEvent::Plan(crate::ipc::PlanEvent { markdown, revision: 1 }),
                ] if text == "No edits are needed."
                    && markdown == "- Search docs\n- Apply the change\n- Build"
            ),
            "answer-only turns emit answer; propose_plan turns emit plan: {events:?}"
        );
    }

    #[tokio::test]
    async fn approved_plan_completion_never_runs_foreground_document_repairs() {
        let approved_markdown = "- Apply the change\n- Verify the build";
        let driver = FakeCodexDriver::scripted([
            AgentTurnResult::Plan {
                markdown: approved_markdown.to_string(),
            },
            AgentTurnResult::Answer {
                text: "Implementation finished.".to_string(),
            },
        ]);
        let driver_handle = driver.clone();
        let sink = CapturingEventSink::default();
        let mut engine = test_engine(driver, sink);
        engine.executor.plan_runtime = Some(engine.runtime.clone());
        let dirs = engine.runtime.data_dirs();
        dirs.ensure_dirs().unwrap();
        let workspace = WorkspaceManager::new(dirs.clone())
            .prepare_snapshot(&bind_workspace_snapshot(&engine.runtime, "ExampleProject"))
            .unwrap();
        driver_handle.set_workspace(workspace.clone());

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "Make a planned change.".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .expect("plan turn should run");
        let request_id = engine.current_request_id.clone().unwrap();
        engine
            .plan_approve()
            .await
            .expect("plan approval should acquire the test write registration");
        engine
            .continue_pending_write()
            .await
            .expect("implementation answer must complete without document repair turns");

        assert_eq!(
            fs::read_to_string(
                workspace
                    .workspace_root
                    .join(format!("plans/{request_id}.md")),
            )
            .unwrap(),
            approved_markdown
        );
        let prompts = driver_handle.prompts();
        assert_eq!(prompts.len(), 2);

        fs::remove_dir_all(dirs.app_data()).ok();
    }

    #[tokio::test]
    async fn accepted_live_changes_settle_without_post_turn_generation() {
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "Implementation finished.".to_string(),
        }]);
        let driver_handle = driver.clone();
        let sink = CapturingEventSink::default();
        let mut engine = test_engine(driver, sink);
        let dirs = engine.runtime.data_dirs();
        dirs.ensure_dirs().unwrap();
        let workspace = WorkspaceManager::new(dirs.clone())
            .prepare_snapshot(&bind_workspace_snapshot(&engine.runtime, "ExampleProject"))
            .unwrap();
        driver_handle.set_workspace(workspace);
        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "Change live projectile behavior.".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        let request_id = engine.current_request_id.clone().unwrap();
        record_file_write_in_memory(
            &engine.journal_store,
            &request_id,
            "file-write",
            1,
            "survivor_projectiles",
        );

        engine.settle_request().await.unwrap();

        assert_eq!(driver_handle.prompts().len(), 1);
        assert_eq!(engine.last_answer, "Implementation finished.");
        assert!(journal::JournalStore::archived_exists(
            &engine.journal_data_dir,
            &request_id
        ));
        let record = engine.session_store.load(&engine.session_id).unwrap();
        assert!(!record.pending_request_ids.contains(&request_id));
        assert!(record.task_state.events.iter().any(|event| {
            event.request_id.as_deref() == Some(&request_id)
                && matches!(
                    &event.kind,
                    crate::task_state::TaskStateEventKind::RequestAccepted {
                        journal_entry_ids,
                        harness_job_id: None
                    } if journal_entry_ids == &["file-write"]
                )
        }));
        assert!(!dirs.app_data().join("harness_jobs").exists());

        fs::remove_dir_all(dirs.app_data()).ok();
    }

    #[tokio::test]
    async fn agentic_engine_sends_changed_project_memory_as_hash_delta() {
        let (base, memory) = memory_store("memory-refresh");
        assert!(memory.write("resources", "Switch 1 = first value").ok);
        let driver = FakeCodexDriver::scripted([
            AgentTurnResult::Answer {
                text: "First answer.".to_string(),
            },
            AgentTurnResult::Answer {
                text: "Second answer.".to_string(),
            },
        ]);
        let driver_handle = driver.clone();
        let sink = CapturingEventSink::default();
        let mut engine = test_engine_with_memory(driver, sink, memory.clone(), &base.join("data"));

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "first request".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .expect("first chat should run");
        assert!(memory.write("resources", "Switch 2 = refreshed value").ok);
        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "second request".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .expect("second chat should run");

        let prompts = driver_handle.prompts();
        assert_eq!(prompts.len(), 2);
        assert!(prompts[0].contains("[project memory]"));
        assert!(prompts[0].contains("Switch 1 = first value"));
        assert!(
            !prompts[0].contains("Switch 2 = refreshed value"),
            "first prompt must reflect the memory visible at the first turn"
        );
        assert!(prompts[1].contains("[project memory delta"));
        assert!(prompts[1].contains("replaces revision="));
        assert!(prompts[1].contains("Switch 2 = refreshed value"));
        assert!(!prompts[1].contains("Switch 1 = first value"));
        assert!(!prompts[1].contains("[project workspace]"));
        assert!(!prompts[1].contains("[eps project architecture]"));

        fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn compiler_cleanup_failure_preserves_valid_state() {
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "Foreground answer.".into(),
        }]);
        driver.script_compilers([Ok(Some(
            json!({"baseRevision": 0, "operations": []}).to_string(),
        ))]);
        let (mut entered, release) = driver.gate_compiler();
        let handle = driver.clone();
        let mut engine = test_engine(driver, CapturingEventSink::default());
        let obstruct_cleanup = async move {
            entered.recv().await.unwrap();
            let root = handle.compiler_workspaces()[0].clone();
            fs::remove_dir_all(&root).unwrap();
            fs::write(&root, b"cleanup obstruction").unwrap();
            release.notify_one();
            root
        };
        let chat = engine.chat(crate::ipc::ChatRequest {
            client_turn_id: "14141414-1414-4414-8414-141414141414".into(),
            text: "retain the current goal".into(),
            attachments: Vec::new(),
            mentions: Vec::new(),
            execution_mode: Default::default(),
            autonomous_policy: None,
            clarify: false,
            skill_selections: Default::default(),
        });
        let (result, root) = tokio::join!(chat, obstruct_cleanup);
        result.unwrap();
        let state = engine
            .session_store
            .load(&engine.session_id)
            .unwrap()
            .task_state;
        assert!(!state.compilation_stale);
        assert!(state.events.iter().any(|event| matches!(
            event.kind,
            crate::task_state::TaskStateEventKind::SemanticDelta { .. }
        )));
        fs::remove_file(root).unwrap();
    }

    #[tokio::test]
    async fn structured_state_compiler_commits_ten_target_projection_without_tools() {
        let turn_id = "11111111-1111-4111-8111-111111111111";
        let members = (1..=10)
            .map(|index| format!("enemy-{index}"))
            .collect::<Vec<_>>();
        let delta = json!({
            "baseRevision": 0,
            "operations": [{
                "op": "upsert",
                "entity": {
                    "entityType": "target_set",
                    "targetSet": {
                        "id": "enemy-roster",
                        "status": "active",
                        "name": "All enemies",
                        "expectedCount": 10,
                        "members": members,
                        "provenance": [{
                            "kind": "user_turn",
                            "clientTurnId": turn_id,
                            "exactQuote": "all ten enemies"
                        }]
                    }
                }
            }]
        })
        .to_string();
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "Roster retained.".to_string(),
        }]);
        driver.script_compilers([Ok(Some(delta))]);
        let driver_handle = driver.clone();
        let sink = CapturingEventSink::default();
        let mut engine = test_engine(driver, sink);

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: turn_id.to_string(),
                text: "all ten enemies".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();

        let record = engine.session_store.load(&engine.session_id).unwrap();
        assert_eq!(record.task_state.projection.target_sets.len(), 1);
        assert_eq!(
            record.task_state.projection.target_sets[0].expected_count,
            Some(10)
        );
        assert_eq!(
            record.task_state.projection.target_sets[0].members.len(),
            10
        );
        assert_eq!(driver_handle.compiler_contracts(), vec![(true, true)]);
        let compiler_prompts = driver_handle.compiler_prompts();
        assert_eq!(compiler_prompts.len(), 1);
        assert!(!compiler_prompts[0].contains("[tools]"));
        let compiler_workspaces = driver_handle.compiler_workspaces();
        assert_eq!(compiler_workspaces.len(), 1);
        assert!(
            !compiler_workspaces[0].exists(),
            "compiler cwd must be removed after completion"
        );
    }

    #[tokio::test]
    async fn delayed_compiler_rejects_result_after_revision_and_branch_change() {
        let foreground_turn_id = "12121212-1212-4212-8212-121212121212";
        let concurrent_turn_id = "13131313-1313-4313-8313-131313131313";
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "Foreground answer.".to_string(),
        }]);
        driver.script_compilers([Ok(Some(
            json!({
                "baseRevision": 0,
                "operations": [{
                    "op": "upsert",
                    "entity": {
                        "entityType": "goal",
                        "fact": {
                            "id": "compiler-goal",
                            "status": "active",
                            "text": "stale compiler result",
                            "provenance": [{
                                "kind": "user_turn",
                                "clientTurnId": foreground_turn_id,
                                "exactQuote": "foreground goal"
                            }]
                        }
                    }
                }]
            })
            .to_string(),
        ))]);
        let (mut compiler_entered, release_compiler) = driver.gate_compiler();
        let mut engine = test_engine(driver, CapturingEventSink::default());
        let sessions = engine.session_store.clone();
        let session_id = engine.session_id.clone();

        let concurrent_update = async move {
            compiler_entered
                .recv()
                .await
                .expect("compiler should reach the execution gate");
            sessions
                .append_task_event(
                    &session_id,
                    None,
                    task_goal_event(
                        concurrent_turn_id,
                        "req-concurrent",
                        0,
                        "concurrent-goal",
                        "newer branch goal",
                    ),
                )
                .expect("concurrent task update should commit");
            release_compiler.notify_one();
        };
        let chat = engine.chat(crate::ipc::ChatRequest {
            client_turn_id: foreground_turn_id.to_string(),
            text: "foreground goal".to_string(),
            attachments: Vec::new(),
            mentions: Vec::new(),
            execution_mode: Default::default(),
            autonomous_policy: None,
            clarify: false,
            skill_selections: Default::default(),
        });

        let (chat_result, ()) = tokio::join!(chat, concurrent_update);
        chat_result.expect("foreground answer should remain successful");

        let record = engine
            .session_store
            .load(&engine.session_id)
            .expect("session should remain readable");
        assert!(record
            .task_state
            .projection
            .goals
            .iter()
            .any(|goal| goal.id == "concurrent-goal"));
        assert!(!record
            .task_state
            .projection
            .goals
            .iter()
            .any(|goal| goal.id == "compiler-goal"));
        assert!(record.task_state.events.iter().any(|event| matches!(
            &event.kind,
            crate::task_state::TaskStateEventKind::StateCompilationFailed {
                reason_code,
                detail,
            } if reason_code == "append_conflict"
                && detail.as_deref() == Some("task-state base changed while compiler was running")
        )));
    }

    #[tokio::test]
    async fn invalid_state_compiler_output_keeps_foreground_answer_and_marks_stale() {
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "Foreground answer.".to_string(),
        }]);
        driver.script_compilers([Ok(Some("not-json".to_string()))]);
        let sink = CapturingEventSink::default();
        let sink_handle = sink.clone();
        let mut engine = test_engine(driver, sink);
        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: "22222222-2222-4222-8222-222222222222".to_string(),
                text: "retain this goal".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();

        let record = engine.session_store.load(&engine.session_id).unwrap();
        let expected_projection = crate::task_state::ActiveTaskProjection {
            revision: 1,
            ..Default::default()
        };
        assert_eq!(record.task_state.projection, expected_projection);
        assert!(record.task_state.compilation_stale);
        assert!(sink_handle.events().iter().any(|event| matches!(
            event,
            EngineEvent::Answer(ipc::AnswerEvent { text }) if text == "Foreground answer."
        )));
        assert!(sink_handle.events().iter().any(|event| matches!(
            event,
            EngineEvent::Progress(ipc::ProgressEvent {
                stage: ipc::ProgressStage::TaskStateWarning,
                ..
            })
        )));
    }
    #[tokio::test]
    async fn state_compiler_runtime_error_records_exact_diagnostic_detail() {
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "Foreground answer.".to_string(),
        }]);
        let diagnostic =
            "task-state compiler event stream closed; stderr: authentication failed".to_string();
        driver.script_compilers([Err(AgentEngineError::new(diagnostic.clone()))]);
        let sink = CapturingEventSink::default();
        let mut engine = test_engine(driver, sink);
        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: "77777777-7777-4777-8777-777777777777".to_string(),
                text: "driver error fixture".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();

        let record = engine.session_store.load(&engine.session_id).unwrap();
        let failure = record
            .task_state
            .events
            .iter()
            .find_map(|event| match &event.kind {
                crate::task_state::TaskStateEventKind::StateCompilationFailed {
                    reason_code,
                    detail,
                } => Some((reason_code, detail)),
                _ => None,
            })
            .expect("driver failure event");
        assert_eq!(failure.0, "runtime_error");
        assert!(failure
            .1
            .as_deref()
            .is_some_and(|detail| detail.contains(diagnostic.as_str())));
    }

    #[tokio::test]
    async fn state_compiler_timeout_keeps_projection_and_records_reason_code() {
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "Foreground answer.".to_string(),
        }]);
        driver.script_compilers([Ok(Some(
            json!({"baseRevision": 0, "operations": []}).to_string(),
        ))]);
        driver.delay_compiler(std::time::Duration::from_millis(100));
        let sink = CapturingEventSink::default();
        let mut engine = test_engine(driver, sink);
        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: "33333333-3333-4333-8333-333333333333".to_string(),
                text: "timeout fixture".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        let record = engine.session_store.load(&engine.session_id).unwrap();
        assert!(record.task_state.events.iter().any(|event| matches!(
            &event.kind,
            crate::task_state::TaskStateEventKind::StateCompilationFailed {
                reason_code,
                detail,
            } if reason_code == "timeout"
                && detail.as_deref().is_some_and(|value| value.contains("timed out"))
        )));
        assert_eq!(record.task_state.projection.revision, 1);
    }

    #[tokio::test]
    async fn manual_compaction_resets_epoch_and_resends_full_baseline_and_projection() {
        let driver = FakeCodexDriver::scripted([
            AgentTurnResult::Answer {
                text: "First.".to_string(),
            },
            AgentTurnResult::Answer {
                text: "Second.".to_string(),
            },
        ]);
        let driver_handle = driver.clone();
        driver.script_compilers([
            Ok(Some(
                json!({"baseRevision": 0, "operations": []}).to_string(),
            )),
            Ok(Some(
                json!({"baseRevision": 2, "operations": []}).to_string(),
            )),
        ]);
        let sink = CapturingEventSink::default();
        let mut engine = test_engine(driver, sink);
        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: "44444444-4444-4444-8444-444444444444".to_string(),
                text: "first".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        let before = engine.session_store.load(&engine.session_id).unwrap();
        engine.compact().await.unwrap();
        let compacted = engine.session_store.load(&engine.session_id).unwrap();
        assert_eq!(
            compacted.context_state.instruction_epoch,
            before.context_state.instruction_epoch + 1
        );
        assert_eq!(compacted.context_state.delivered.epoch, 0);
        assert!(matches!(
            compacted.task_state.events.last().map(|event| &event.kind),
            Some(crate::task_state::TaskStateEventKind::CompactionBoundary { .. })
        ));

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: "55555555-5555-4555-8555-555555555555".to_string(),
                text: "second".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        let prompts = driver_handle.prompts();
        assert!(prompts[1].contains("[first principles]"));
        assert!(prompts[1].contains("active task state delivery=snapshot"));
        assert!(prompts[1].contains(&format!(
            "instructionEpoch={}",
            compacted.context_state.instruction_epoch
        )));
    }

    #[tokio::test]
    async fn static_prompt_fingerprint_change_starts_fresh_without_losing_task_or_log() {
        let driver = FakeCodexDriver::scripted([
            AgentTurnResult::Answer {
                text: "First.".to_string(),
            },
            AgentTurnResult::Answer {
                text: "Second.".to_string(),
            },
        ]);
        let driver_handle = driver.clone();
        let sink = CapturingEventSink::default();
        let mut engine = test_engine(driver, sink);
        let first_turn = "66666666-6666-4666-8666-666666666666";
        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: first_turn.to_string(),
                text: "stable goal".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        engine
            .session_store
            .append_task_event(
                &engine.session_id,
                None,
                task_goal_event(first_turn, "req-state", 0, "stable-goal", "stable goal"),
            )
            .unwrap();
        let mut saved = engine.session_store.load(&engine.session_id).unwrap();
        saved.context_state.static_prompt_fingerprint = "outdated".to_string();
        saved.panel_log = json!({
            "schemaVersion": 2,
            "logSeq": 1,
            "log": [{
                "id": 1,
                "kind": "you",
                "text": "stable goal",
                "clientTurnId": first_turn
            }]
        });
        engine.session_store.save(&saved).unwrap();

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: "77777777-7777-4777-8777-777777777777".to_string(),
                text: "continue".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        let prompts = driver_handle.prompts();
        assert_eq!(driver_handle.reset_count(), 1);
        assert!(prompts[1].contains("[first principles]"));
        assert!(prompts[1].contains("[prior conversation]"));
        assert!(prompts[1].contains("stable-goal"));
        let loaded = engine.session_store.load(&engine.session_id).unwrap();
        assert_eq!(loaded.task_state.events.len(), 1);
        assert_eq!(loaded.panel_log["log"][0]["text"], "stable goal");
        assert_ne!(loaded.context_state.static_prompt_fingerprint, "outdated");
    }

    #[tokio::test]
    async fn rewind_restores_anchored_branch_and_full_prompt_excludes_abandoned_fact() {
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "Branched.".to_string(),
        }]);
        let driver_handle = driver.clone();
        let sink = CapturingEventSink::default();
        let mut engine = test_engine(driver, sink);
        let first_turn = "88888888-8888-4888-8888-888888888888";
        let second_turn = "99999999-9999-4999-8999-999999999999";
        let first = engine
            .session_store
            .append_task_event(
                &engine.session_id,
                None,
                task_goal_event(first_turn, "req-first", 0, "first-goal", "first goal"),
            )
            .unwrap();
        engine
            .session_store
            .append_task_event(
                &engine.session_id,
                first.leaf_id.as_deref(),
                task_goal_event(
                    second_turn,
                    "req-second",
                    1,
                    "abandoned-goal",
                    "abandoned goal",
                ),
            )
            .unwrap();
        engine
            .rewind(json!({
                "schemaVersion": 2,
                "logSeq": 1,
                "log": [{
                    "id": 1,
                    "kind": "you",
                    "text": "first goal",
                    "clientTurnId": first_turn
                }]
            }))
            .await
            .unwrap();
        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
                text: "new branch".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        let prompt = &driver_handle.prompts()[0];
        assert!(prompt.contains("first-goal"));
        assert!(!prompt.contains("abandoned-goal"));
        assert!(prompt.contains("[first principles]"));
        let loaded = engine.session_store.load(&engine.session_id).unwrap();
        assert_eq!(loaded.task_state.events.len(), 2);
        assert_eq!(loaded.task_state.projection.goals[0].id, "first-goal");
    }

    #[tokio::test]
    async fn accept_records_dat_edits_to_wiki_and_emits_wiki_event() {
        let base = unique_temp_dir("wiki-accept");
        let memory = ProjectMemory::new(base.join("memory"), "ExampleProject");
        let wiki_dir = base.join("wiki");
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Plan {
            markdown: "- Buff the marine".to_string(),
        }]);
        let sink = CapturingEventSink::default();
        let sink_handle = sink.clone();
        let mut engine = test_engine_with_wiki(driver, sink, memory, &base.join("data"), &wiki_dir);
        engine.executor.plan_runtime = Some(engine.runtime.clone());

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "set marine HP to 80".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .expect("chat should run");
        let request_id = engine
            .current_request_id
            .clone()
            .expect("chat should create a request id");
        // A units dat edit (recorded) and a file write (out of wiki scope).
        record_dat_set_in_memory(
            &engine.journal_store,
            &request_id,
            "dat-hp",
            1,
            ("units", 0, "HP"),
            json!(80),
        );
        record_file_write_in_memory(
            &engine.journal_store,
            &request_id,
            "file-write",
            2,
            "scripts/main.eps",
        );

        engine
            .settle_request()
            .await
            .expect("accept-all decision should finalize");

        // The ledger persisted exactly the dat edit (file write is out of scope).
        let store = crate::wiki::WikiStore::load(Some(wiki_dir));
        assert_eq!(
            store.ledger().entries.len(),
            1,
            "only the dat edit recorded"
        );
        let entry = &store.ledger().entries["dat:units:0:HP"];
        assert_eq!(entry.value, json!(80));
        assert_eq!(entry.item_name.as_deref(), Some("Terran Marine"));
        assert!(
            !entry.edited_by_user,
            "accept-hook writes editedByUser=false"
        );

        // A `wiki` event carried the updated ledger to the panel.
        let wiki_event = sink_handle
            .events()
            .into_iter()
            .find_map(|event| match event {
                EngineEvent::Wiki(payload) => Some(payload),
                _ => None,
            })
            .expect("accept should emit a wiki event");
        assert!(wiki_event.entries.contains_key("dat:units:0:HP"));

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn system_prompt_orders_first_principles_before_reference_context() {
        let hits = sample_hits();
        let prompt = build_system_prompt(
            "How do I avoid crash-prone trigger edits?",
            &hits,
            "[project state]\nproject=Sample compiling=false",
            None,
            None,
        );

        let first_principles = prompt
            .find("[first principles]")
            .expect("system prompt must contain [first principles]");
        let reference_context = prompt
            .find("[reference context]")
            .expect("system prompt must contain [reference context]");

        assert!(
            first_principles < reference_context,
            "[first principles] must appear before [reference context]"
        );
    }

    #[test]
    fn system_prompt_advertises_interaction_tools_before_policy() {
        let prompt = build_system_prompt(
            "Explain the design",
            &sample_hits(),
            "[project state]",
            None,
            None,
        );
        assert!(prompt.contains("[interaction]"));
        assert!(tool_catalog_section(crate::provider::ProviderId::Codex)
            .split(|c: char| c == ',' || c.is_whitespace())
            .any(|name| name == "ask"));
        assert!(prompt.find("[tools]").unwrap() < prompt.find("[interaction]").unwrap());
    }

    #[test]
    fn system_prompt_orders_eps_idioms_between_first_principles_and_reference_context() {
        let hits = sample_hits();
        let prompt = build_system_prompt(
            "How do I write a death-counter loop in eps?",
            &hits,
            "[project state]\nproject=Sample compiling=false",
            None,
            None,
        );

        let first_principles = prompt
            .find("[first principles]")
            .expect("system prompt must contain [first principles]");
        let eps_idioms = prompt
            .find("[eps idioms]")
            .expect("system prompt must contain [eps idioms]");
        let reference_context = prompt
            .find("[reference context]")
            .expect("system prompt must contain [reference context]");

        assert!(
            first_principles < eps_idioms,
            "[first principles] must appear before [eps idioms]"
        );
        assert!(
            eps_idioms < reference_context,
            "[eps idioms] must appear before [reference context]"
        );
    }

    #[test]
    fn wiki_facts_render_after_memory_and_before_reference_context_when_present() {
        let hits = sample_hits();
        let wiki = "[wiki facts]\nNOTE: agent-applied last values; may differ from the live map.\n## dat units\n- Terran Marine\n  - HP = 80";
        let prompt = build_system_prompt(
            "buff the marine",
            &hits,
            "[project state]\nproject=Sample compiling=false",
            Some("[project memory]\n## resources\nSwitch 1 = boss"),
            Some(wiki),
        );

        let memory = prompt
            .find("Switch 1 = boss")
            .expect("dynamic memory body present");
        let wiki_facts = prompt
            .find("NOTE: agent-applied last values")
            .expect("dynamic wiki body present");
        let reference = prompt
            .find("RAG chunk about safe epscript practice")
            .expect("dynamic reference body present");
        assert!(memory < wiki_facts, "[project memory] before [wiki facts]");
        assert!(
            wiki_facts < reference,
            "[wiki facts] before [reference context]"
        );
        assert!(prompt.contains("may differ from the live map"));

        // Omitted -> no section, no header.
        let without = build_system_prompt(
            "buff the marine",
            &hits,
            "[project state]\nproject=Sample compiling=false",
            None,
            None,
        );
        assert!(!without.contains("may differ from the live map"));
    }

    #[test]
    fn changed_wiki_follow_up_sends_one_replacement_without_reference_replay() {
        let turn = assembled_followup(
            "what is the marine HP?",
            None,
            Some("[wiki facts]\nold"),
            None,
            Some("[wiki facts]\n## dat units\n- Terran Marine\n  - HP = 80"),
        );
        assert_eq!(turn.matches("[wiki facts delta").count(), 1);
        assert!(!turn.contains("[reference context]"));
        assert!(!turn.contains("[eps project architecture]"));
    }

    #[test]
    fn system_prompt_contains_required_sections() {
        let prompt = build_system_prompt(
            "Explain a safe location workflow",
            &sample_hits(),
            "[project state]",
            None,
            None,
        );
        for section in [
            "[first principles]",
            "[map handoff]",
            "[evidence]",
            "[message format]",
            "[error recovery]",
            "[reference context]",
        ] {
            assert!(prompt.contains(section), "missing section {section}");
        }
        for name in [
            "docs_get",
            "source_search",
            "map_task_request",
            "map_task_status",
            "map_task_apply",
            "map_task_diff",
        ] {
            assert!(prompt.contains(name), "missing tool {name}");
        }
        let map = build_map_system_prompt("[project state]", None);
        assert!(map.contains("[error recovery]"));
        assert!(map.contains("[authority]"));
        assert!(map.contains("[candidate workflow]"));
    }

    #[test]
    fn ordinary_eps_prompt_omits_unrelated_workflow_guides() {
        let prompt = build_system_prompt(
            "Change the mineral reward in src/main.eps",
            &[],
            "[project state]\nproject=Sample",
            None,
            None,
        );

        assert!(!prompt.lines().any(|line| line == "[direct python]"));
        assert!(!prompt.lines().any(|line| line == "[map sounds]"));
    }

    #[test]
    fn context_pressure_continuation_tells_the_next_iteration_to_stop_widening() {
        let previous = AgentTurnInput::text("x");
        let pressured =
            AgentEngine::<FakeCodexDriver, CapturingEventSink>::interactive_continuation_turn(
                &previous,
                crate::provider_runtime::IterationBoundaryReason::ContextPressure,
            );
        assert!(pressured.text.contains("넓은 탐색을 더 벌이지 말고"));
        assert!(pressured.text.contains("직접 다시 읽어야"));
        let other =
            AgentEngine::<FakeCodexDriver, CapturingEventSink>::interactive_continuation_turn(
                &previous,
                crate::provider_runtime::IterationBoundaryReason::ProviderContinuation,
            );
        assert!(!other.text.contains("넓은 탐색"));
    }

    #[test]
    fn continuation_selects_python_guidance_from_existing_sources() {
        let prompt = build_system_prompt(
            "Continue",
            &[],
            "[project state]\nfiles=src/main.eps, src/helper.py",
            None,
            None,
        );
        assert!(prompt.lines().any(|line| line == "[direct python]"));
        assert!(!prompt.lines().any(|line| line == "[map sounds]"));
    }

    #[test]
    fn python_request_selects_only_python_workflow_guidance() {
        let prompt = build_system_prompt("Python dependencies", &[], "[project state]", None, None);
        assert!(prompt.lines().any(|line| line == "[direct python]"));
        assert!(!prompt.lines().any(|line| line == "[map sounds]"));
        assert!(prompt.contains("python_dependencies_prepare"));
        assert!(prompt.contains("python_dependencies_set"));
    }

    #[tokio::test]
    async fn new_workflow_guidance_reaches_the_live_context_once() {
        let (mut engine, driver, _cancel) = generation_engine(
            test_session,
            vec![
                AgentTurnResult::Answer {
                    text: "done".to_string(),
                },
                AgentTurnResult::Answer {
                    text: "done".to_string(),
                },
                AgentTurnResult::Answer {
                    text: "done".to_string(),
                },
            ],
        );
        engine
            .chat(plain_request("Change the mineral reward"))
            .await
            .unwrap();
        engine
            .chat(plain_request("Add music playback"))
            .await
            .unwrap();
        engine
            .chat(plain_request("Adjust music playback"))
            .await
            .unwrap();
        let prompts = driver.prompts();
        assert!(!prompts[0].lines().any(|line| line == "[task guidance]"));
        assert!(prompts[1].contains("[task guidance delta"));
        assert!(prompts[1].lines().any(|line| line == "[map sounds]"));
        assert!(!prompts[2].contains("[task guidance"));
        assert!(!prompts[2].lines().any(|line| line == "[map sounds]"));
    }
    #[test]
    fn system_prompt_keeps_build_authoritative_without_runtime_trace_tools() {
        let prompt = build_system_prompt("Change EPS files", &[], "[project state]", None, None);
        assert!(prompt.contains("[build]"));
        assert!(prompt.contains("build_run"));
        assert!(prompt.contains("build_log_read"));
        for unregistered in [
            "[runtime trace tests]",
            "trace_suite_run",
            "trace_test_run",
            "eudAgentTestSetup",
            "eps_check",
            "build_errors",
        ] {
            assert!(!prompt.contains(unregistered));
        }
    }

    #[test]
    fn every_provider_is_offered_each_registered_tool_once() {
        for provider in crate::provider::ProviderId::ALL {
            let catalog = tool_catalog_section(provider);
            let names: Vec<_> = catalog
                .split(|c: char| c == ',' || c.is_whitespace())
                .collect();
            for spec in crate::tools::tool_registry() {
                assert_eq!(
                    names.iter().filter(|name| **name == spec.name).count(),
                    1,
                    "{provider:?}: {}",
                    spec.name
                );
            }
            assert!(
                catalog.len() < 4000,
                "catalog duplicates tool contracts: {}",
                catalog.len()
            );
        }
    }

    #[test]
    fn cold_start_contains_architecture_but_follow_up_does_not_repeat_it() {
        let hits = sample_hits();
        let project_state = "[project state]\nproject=Sample compiling=false";
        let cold = build_system_prompt(
            "Place a cohesive epScript feature",
            &hits,
            project_state,
            None,
            None,
        );
        let resumed = assembled_followup("Where should this small fix go?", None, None, None, None);

        assert!(cold.contains("[eps project architecture]"));
        let first_principles = cold.find("[first principles]").unwrap();
        let epscript = cold.find("[epscript]").unwrap();
        let architecture = cold.find("[eps project architecture]").unwrap();
        let build = cold.find("[build]").unwrap();
        let reference = cold.find("[reference context]").unwrap();
        assert!(first_principles < epscript);
        assert!(epscript < architecture);
        assert!(architecture < build);
        assert!(build < reference);
        assert!(architecture < reference);

        assert!(!resumed.contains("[eps project architecture]"));
        assert!(!resumed.contains("[eps idioms]"));
        assert!(!resumed.contains("[reference context]"));
        assert!(resumed.contains("[project state]"));
        assert!(resumed.contains("[user message]"));
    }

    #[test]
    fn luck_defense_skill_is_loaded_only_for_creation_requests() {
        let baseline = static_prompt_baseline(crate::provider::ProviderId::Codex);
        assert!(!baseline.contains("# 운빨 디펜스 제작"));
        assert!(selected_luck_defense_skill("[$luck-defense]\n만들어줘").is_some());
        assert!(selected_luck_defense_skill("운빨 디펜스 만들어줘").is_some());
        assert!(selected_luck_defense_skill("운빨 디펜스 스킬 켜줘").is_some());
        assert!(selected_luck_defense_skill("운빨 디펜스 스킬 사용해줘").is_some());
        assert!(selected_luck_defense_skill("기존 운빨 디펜스는 어떻게 구성돼?").is_none());
        assert!(selected_luck_defense_skill("일반 트리거 질문").is_none());
    }

    #[tokio::test]
    async fn luck_defense_skill_reaches_only_the_selected_model_turn() {
        let _shown = crate::skills::show_hidden_skills();
        let (mut engine, driver, _cancel) = generation_engine(
            test_session,
            vec![
                AgentTurnResult::Answer {
                    text: "ordinary done".to_string(),
                },
                AgentTurnResult::Answer {
                    text: "skill done".to_string(),
                },
            ],
        );
        engine
            .chat(plain_request("일반 트리거 질문"))
            .await
            .unwrap();
        assert!(!driver
            .prompts()
            .last()
            .unwrap()
            .contains("[active skill]\n---\nname: luck-defense"));

        engine
            .chat(plain_request("[$luck-defense]\n운빨 디펜스 만들어줘"))
            .await
            .unwrap();
        let prompt = driver.prompts().pop().unwrap();
        assert_eq!(
            prompt
                .lines()
                .filter(|line| *line == "[active skill]")
                .count(),
            1,
            "{prompt}"
        );
        assert_eq!(
            prompt
                .lines()
                .filter(|line| *line == "<!-- luck-defense:section:shared -->")
                .count(),
            1,
            "{prompt}"
        );
        assert_eq!(
            prompt
                .lines()
                .filter(|line| *line == "<!-- luck-defense:section:router -->")
                .count(),
            1,
            "{prompt}"
        );
        for mode_section in [
            "<!-- luck-defense:section:new -->",
            "<!-- luck-defense:section:continue -->",
            "<!-- luck-defense:section:local -->",
        ] {
            assert!(!prompt.lines().any(|line| line == mode_section), "{prompt}");
        }
        assert!(prompt.contains("[user message]\n[$luck-defense]"));
    }

    /// While luck-defense is hidden, neither its marker, the request text, nor an
    /// explicit ON delivers its guidance.
    #[tokio::test]
    async fn hidden_luck_defense_reaches_no_model_turn() {
        assert!(crate::skills::is_hidden(crate::skills::LUCK_DEFENSE));
        let (mut engine, driver, _cancel) = generation_engine(
            test_session,
            vec![AgentTurnResult::Answer {
                text: "done".to_string(),
            }],
        );
        let mut request = plain_request("[$luck-defense]\n운빨 디펜스 만들어줘");
        request.skill_selections =
            std::collections::BTreeMap::from([(crate::skills::LUCK_DEFENSE.to_string(), true)]);
        engine.chat(request).await.unwrap();
        let prompt = driver.prompts().pop().unwrap();
        assert!(!prompt.contains("[skill: luck-defense]"), "{prompt}");
        assert!(!prompt.contains("<!-- luck-defense:"), "{prompt}");
    }

    #[test]
    fn empty_optional_project_fields_do_not_select_workflow_guides() {
        let prompt = build_system_prompt("Build the mineral reward change", &[], "[project state]\n{\"pythonEntrypoints\":[],\"pythonDependencies\":[],\"soundCount\":0}", None, None);
        assert!(!prompt
            .lines()
            .any(|line| line == "[direct python]" || line == "[map sounds]"));
    }

    #[test]
    fn follow_up_delta_labels_only_the_current_user_message() {
        let user_text = "The editor freezes when I test the map.";
        let turn_text = assembled_followup(user_text, None, None, None, None);
        let user_header_line = turn_text
            .lines()
            .position(|line| line == "[user message]")
            .expect("follow-up text must contain a line exactly [user message]");
        let following_line = turn_text
            .lines()
            .nth(user_header_line + 1)
            .expect("[user message] must be followed by the user's text");
        assert_eq!(following_line, user_text);
        assert!(!turn_text.contains("[reference context]"));
        assert!(!turn_text.contains("[project workspace]"));
    }

    #[tokio::test]
    async fn mock_runtime_seed_sets_conversation_state_and_reset_clears_it() {
        let mut executor = FakeCodexDriver::scripted([]);
        assert_eq!(
            executor.conversation_state(),
            crate::provider::ProviderConversationState::Codex { thread_id: None }
        );

        executor
            .seed(crate::provider::ProviderConversationState::Codex {
                thread_id: Some("thread-seeded".to_string()),
            })
            .await
            .expect("seed should succeed");
        assert_eq!(
            executor.conversation_state(),
            crate::provider::ProviderConversationState::Codex {
                thread_id: Some("thread-seeded".to_string())
            }
        );
        assert_eq!(executor.seeded_ids(), vec!["thread-seeded".to_string()]);

        executor.reset().await.expect("reset should succeed");
        assert_eq!(
            executor.conversation_state(),
            crate::provider::ProviderConversationState::Codex { thread_id: None }
        );
    }

    #[tokio::test]
    async fn different_session_read_turns_overlap_and_short_turn_finishes_first() {
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
        let release_a = Arc::new(tokio::sync::Notify::new());
        let release_b = Arc::new(tokio::sync::Notify::new());
        let mut engine_a = test_engine(
            GateCodexDriver::new("a", entered_tx.clone(), Arc::clone(&release_a), true),
            CapturingEventSink::default(),
        );
        let mut engine_b = test_engine(
            GateCodexDriver::new("b", entered_tx, release_b, false),
            CapturingEventSink::default(),
        );

        let long = tokio::spawn(async move {
            engine_a
                .chat(ipc::ChatRequest {
                    client_turn_id: crate::ipc::new_client_turn_id(),
                    text: "long read".to_string(),
                    attachments: Vec::new(),
                    mentions: Vec::new(),
                    execution_mode: Default::default(),
                    autonomous_policy: None,
                    clarify: false,
                    skill_selections: Default::default(),
                })
                .await
        });
        assert_eq!(entered_rx.recv().await, Some("a"));

        let short = tokio::spawn(async move {
            engine_b
                .chat(ipc::ChatRequest {
                    client_turn_id: crate::ipc::new_client_turn_id(),
                    text: "short read".to_string(),
                    attachments: Vec::new(),
                    mentions: Vec::new(),
                    execution_mode: Default::default(),
                    autonomous_policy: None,
                    clarify: false,
                    skill_selections: Default::default(),
                })
                .await
        });
        assert_eq!(entered_rx.recv().await, Some("b"));
        tokio::time::timeout(std::time::Duration::from_secs(1), short)
            .await
            .expect("session B must finish while A is blocked")
            .unwrap()
            .unwrap();
        assert!(!long.is_finished());
        release_a.notify_waiters();
        long.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn commands_in_one_session_remain_serialized() {
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Notify::new());
        let engine = Arc::new(tokio::sync::Mutex::new(test_engine(
            GateCodexDriver::new("same", entered_tx, Arc::clone(&release), true),
            CapturingEventSink::default(),
        )));

        let first_engine = Arc::clone(&engine);
        let first = tokio::spawn(async move {
            first_engine
                .lock()
                .await
                .chat(ipc::ChatRequest {
                    client_turn_id: crate::ipc::new_client_turn_id(),
                    text: "first".to_string(),
                    attachments: Vec::new(),
                    mentions: Vec::new(),
                    execution_mode: Default::default(),
                    autonomous_policy: None,
                    clarify: false,
                    skill_selections: Default::default(),
                })
                .await
        });
        assert_eq!(entered_rx.recv().await, Some("same"));

        let second_engine = Arc::clone(&engine);
        let second = tokio::spawn(async move {
            second_engine
                .lock()
                .await
                .chat(ipc::ChatRequest {
                    client_turn_id: crate::ipc::new_client_turn_id(),
                    text: "second".to_string(),
                    attachments: Vec::new(),
                    mentions: Vec::new(),
                    execution_mode: Default::default(),
                    autonomous_policy: None,
                    clarify: false,
                    skill_selections: Default::default(),
                })
                .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), entered_rx.recv())
                .await
                .is_err(),
            "the second same-session command entered before the first settled"
        );
        release.notify_waiters();
        first.await.unwrap().unwrap();
        assert_eq!(entered_rx.recv().await, Some("same"));
        second.await.unwrap().unwrap();
    }

    #[test]
    fn cancellation_generation_is_scoped_to_one_worker_channel() {
        let (cancel_a, mut watch_a) = tokio::sync::watch::channel(0_u64);
        let (cancel_b, mut watch_b) = tokio::sync::watch::channel(0_u64);

        cancel_worker_generation(&cancel_a).unwrap();

        assert!(watch_a.has_changed().unwrap());
        assert_eq!(*watch_a.borrow_and_update(), 1);
        assert!(!watch_b.has_changed().unwrap());
        assert_eq!(*watch_b.borrow_and_update(), 0);
        drop(cancel_b);
    }

    #[test]
    fn failed_read_recovery_releases_granted_intent_before_retry() {
        let mut engine = test_engine(FakeCodexDriver::scripted([]), CapturingEventSink::default());
        engine
            .runtime
            .begin_request("request-old", "Sample")
            .unwrap();
        engine
            .runtime
            .register_write_request("write after read")
            .unwrap();
        engine.current_request_id = Some("request-old".to_string());
        engine.phase = Phase::Triage;

        engine.recover_read_failure().unwrap();

        assert_eq!(engine.phase, Phase::Idle);
        assert!(!engine.runtime.owns_write_registration());
        engine
            .runtime
            .begin_request("request-new", "Sample")
            .unwrap();
        let next = engine
            .runtime
            .register_write_request("retry write")
            .unwrap();
        assert_eq!(next.state(), crate::write_coordinator::TicketState::Granted);
    }

    #[test]
    fn pending_review_recovery_coexists_with_new_writer_ticket() {
        let base = unique_temp_dir("pending-review-recovery");
        let dirs = crate::config::DataDirs::from_bases(&base, &base);
        dirs.ensure_dirs().unwrap();
        let sessions = crate::session::SessionStore::new(&dirs);
        let mut record = test_session(&sessions);
        let request_id = "req-restored";
        let journal = journal::JournalStore::new(dirs.app_data());
        record_file_write(&journal, request_id, "write-1", 1, "one.eps");
        record.pending_request_ids = vec![request_id.to_string()];
        sessions.save(&record).unwrap();
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();

        restore_pending_review(
            &sessions,
            &dirs,
            &writes,
            &crate::project_id::ProjectId::from_stored("Sample"),
        )
        .unwrap();
        let next = writes
            .request("eps:Sample", "session-next", "req-next")
            .unwrap();

        assert!(writes.owns("eps:Sample", &record.meta.id, request_id));
        assert_eq!(next.state(), crate::write_coordinator::TicketState::Granted);
        fs::remove_dir_all(base).ok();
    }
    #[test]
    fn missing_pending_review_does_not_block_valid_review_recovery() {
        let base = unique_temp_dir("missing-pending-review-isolation");
        let dirs = crate::config::DataDirs::from_bases(&base, &base);
        dirs.ensure_dirs().unwrap();
        let sessions = crate::session::SessionStore::new(&dirs);
        let mut missing_record = test_session(&sessions);
        let missing_request_id = "req-missing";
        missing_record.pending_request_ids = vec![missing_request_id.to_string()];
        sessions.save(&missing_record).unwrap();

        let mut valid_record = test_session(&sessions);
        let valid_request_id = "req-valid";
        let journal = journal::JournalStore::new(dirs.app_data());
        record_file_write(&journal, valid_request_id, "write-valid", 1, "valid.eps");
        valid_record.pending_request_ids = vec![valid_request_id.to_string()];
        sessions.save(&valid_record).unwrap();
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();

        let recovery = restore_pending_review(
            &sessions,
            &dirs,
            &writes,
            &crate::project_id::ProjectId::from_stored("Sample"),
        );

        let session_errors = recovery.expect("one missing journal must not block other sessions");
        assert_eq!(session_errors.len(), 1);
        assert!(session_errors
            .get(&missing_record.meta.id)
            .is_some_and(|error| error.contains(missing_request_id)));
        assert!(writes.owns("eps:Sample", &valid_record.meta.id, valid_request_id));
        assert!(!writes.owns("eps:Sample", &missing_record.meta.id, missing_request_id));
        fs::remove_dir_all(base).ok();
    }
    #[test]
    fn rewind_clears_only_an_unrecoverable_pending_review() {
        let base = unique_temp_dir("rewind-unrecoverable-pending-review");
        let dirs = crate::config::DataDirs::from_bases(&base, &base);
        dirs.ensure_dirs().unwrap();
        let sessions = crate::session::SessionStore::new(&dirs);
        let mut missing_record = test_session(&sessions);
        missing_record.provider_binding.conversation =
            crate::provider::ProviderConversationState::Codex {
                thread_id: Some("thread-missing".to_string()),
            };
        missing_record.pending_request_ids = vec!["req-missing".to_string()];
        missing_record.panel_log = serde_json::json!({"log": ["old"]});
        sessions.save(&missing_record).unwrap();
        let prefix = serde_json::json!({"log": ["prefix"]});

        let recovered_project = rewind_unrecoverable_pending_session(
            &sessions,
            &dirs,
            &missing_record.meta.id,
            "Sample",
            prefix.clone(),
        )
        .unwrap();

        assert_eq!(recovered_project.as_deref(), Some("Sample"));
        let repaired = sessions.load(&missing_record.meta.id).unwrap();
        assert_eq!(
            repaired.provider_binding.conversation,
            crate::provider::ProviderConversationState::Codex { thread_id: None }
        );
        assert!(repaired.pending_request_ids.is_empty());
        assert_eq!(repaired.panel_log, prefix);

        let mut valid_record = test_session(&sessions);
        valid_record.provider_binding.conversation =
            crate::provider::ProviderConversationState::Codex {
                thread_id: Some("thread-valid".to_string()),
            };
        valid_record.pending_request_ids = vec!["req-valid-rewind".to_string()];
        valid_record.panel_log = serde_json::json!({"log": ["valid"]});
        sessions.save(&valid_record).unwrap();
        let journal = journal::JournalStore::new(dirs.app_data());
        record_file_write(
            &journal,
            "req-valid-rewind",
            "write-valid-rewind",
            1,
            "valid.eps",
        );

        let valid_result = rewind_unrecoverable_pending_session(
            &sessions,
            &dirs,
            &valid_record.meta.id,
            "Sample",
            serde_json::json!({"log": []}),
        )
        .unwrap();

        assert!(valid_result.is_none());
        let preserved = sessions.load(&valid_record.meta.id).unwrap();
        assert_eq!(
            preserved.provider_binding.conversation,
            crate::provider::ProviderConversationState::Codex {
                thread_id: Some("thread-valid".to_string())
            }
        );
        assert_eq!(
            preserved.pending_request_ids,
            vec!["req-valid-rewind".to_string()]
        );
        assert_eq!(preserved.panel_log, serde_json::json!({"log": ["valid"]}));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn archived_pending_review_is_removed_from_stale_session_record() {
        let base = unique_temp_dir("archived-pending-review");
        let dirs = crate::config::DataDirs::from_bases(&base, &base);
        dirs.ensure_dirs().unwrap();
        let sessions = crate::session::SessionStore::new(&dirs);
        let mut record = test_session(&sessions);
        let request_id = "req-archived";
        let journal = journal::JournalStore::new(dirs.app_data());
        record_file_write_in_memory(&journal, request_id, "write-1", 1, "one.eps");
        journal.persist(request_id).unwrap();
        journal.archive(request_id).unwrap();
        record.pending_request_ids = vec![request_id.to_string()];
        sessions.save(&record).unwrap();
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();

        restore_pending_review(
            &sessions,
            &dirs,
            &writes,
            &crate::project_id::ProjectId::from_stored("Sample"),
        )
        .unwrap();

        let repaired = sessions.load(&record.meta.id).unwrap();
        assert!(repaired.pending_request_ids.is_empty());
        let next = writes
            .request("eps:Sample", "session-next", "req-next")
            .unwrap();
        assert_eq!(next.state(), crate::write_coordinator::TicketState::Granted);
        fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn chat_injects_text_attachments_and_forwards_images_to_codex() {
        let base = unique_temp_dir("chat-attachments");
        let attachment_store = attachment_store_at(&base);
        let text = attachment_store
            .stage("notes.eps", "text/plain", "const value = 7;".as_bytes())
            .expect("text attachment should stage");
        let image = attachment_store
            .stage("screen.png", "image/png", b"\x89PNG\r\n\x1a\nbody")
            .expect("image attachment should stage");
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "확인했습니다.".to_string(),
        }]);
        let driver_handle = driver.clone();
        let sink = CapturingEventSink::default();
        let sessions = session_store_at(&base);
        let session = test_session(&sessions);
        let (_cancellation_tx, cancellation_rx) = tokio::sync::watch::channel(0_u64);
        let mut engine = AgentEngine::new(
            driver,
            sink,
            AgentEngineConfig::for_tests(
                "[project state]\nproject=Sample compiling=false",
                None,
                sample_hits(),
            ),
            test_tool_runtime(),
            sessions,
            attachment_store.clone(),
            session,
            cancellation_rx,
        );

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "첨부 내용을 검토해 줘".to_string(),
                attachments: vec![text.id.clone(), image.id.clone()],
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .expect("attachment turn should complete");

        let prompts = driver_handle.prompts();
        assert!(prompts[0].contains("[attached file: notes.eps]"));
        assert!(prompts[0].contains("const value = 7;"));
        let image_paths = driver_handle.image_paths();
        assert_eq!(image_paths.len(), 1);
        assert_eq!(image_paths[0].len(), 1);
        assert!(image_paths[0][0].is_file());
        assert!(attachment_store.discard_draft(&text.id).is_err());

        attachment_store
            .delete_session(&engine.session_id)
            .expect("session delete should clean attachments");
        assert!(!image_paths[0][0].exists());
        fs::remove_dir_all(base).ok();
    }
    fn engine_mention_context(
        location: crate::chk::Location,
    ) -> crate::map_context::MapContextSnapshot {
        crate::map_context::MapContextSnapshot {
            revision: crate::map_model::MapRevision {
                project_id: "Sample".to_string(),
                source_path: PathBuf::from("C:/private/source.scx"),
                file_sha256: "a".repeat(64),
                chk_sha256: "b".repeat(64),
                mtime_ns: 1,
                tileset: crate::map_model::Tileset::Jungle,
                width: 64,
                height: 64,
            },
            saved_source_notice: "saved".to_string(),
            source_file_size: 100,
            starcraft_path: PathBuf::from("C:/private/StarCraft"),
            digest: crate::chk::Digest {
                map: crate::chk::MapHeader {
                    width: 64,
                    height: 64,
                    tileset: "Jungle".to_string(),
                    title: String::new(),
                    description: String::new(),
                },
                players: Vec::new(),
                forces: Vec::new(),
                locations: vec![location],
                units: Vec::new(),
                doodads: Vec::new(),
                sprites: Vec::new(),
                start_locations: Vec::new(),
                tiles: Vec::new(),
                switches: Vec::new(),
                switch_usages: Vec::new(),
            },
        }
    }

    fn engine_location_mention() -> (crate::chk::Location, crate::mentions::MentionInstance) {
        let location = crate::chk::Location {
            id: 17,
            name: "회복 지점".to_string(),
            left: 32,
            top: 64,
            right: 160,
            bottom: 192,
            tile_rect: [1, 2, 5, 6],
            elevation_flags: 3,
            inverted: None,
            anywhere: None,
        };
        let mention = crate::mentions::MentionInstance {
            id: "mention-location".to_string(),
            label: location.name.clone(),
            detail: Some("#17".to_string()),
            mention: crate::mentions::MentionSnapshot::MapLocation(
                crate::mentions::MapLocationMentionV1 {
                    version: 1,
                    project_id: "Sample".to_string(),
                    source_file_sha256: "a".repeat(64),
                    location_id: 17,
                    location_fingerprint: crate::mentions::location_fingerprint(&location),
                },
            ),
            stale: false,
        };
        (location, mention)
    }

    fn assert_resolved_before_user(prompt: &str) {
        let resolved = prompt
            .find("[resolved mentions]")
            .expect("resolved mention section");
        let user = prompt.find("[user message]").expect("user message section");
        assert!(resolved < user);
    }

    #[tokio::test]
    async fn valid_mentions_are_ordered_on_cold_resumed_and_plan_feedback_turns() {
        let (location, mention) = engine_location_mention();
        let driver = FakeCodexDriver::scripted([
            AgentTurnResult::Answer {
                text: "cold".to_string(),
            },
            AgentTurnResult::Answer {
                text: "resumed".to_string(),
            },
        ]);
        let handle = driver.clone();
        let mut engine = test_engine(driver, CapturingEventSink::default());
        engine
            .runtime
            .mentions()
            .set_context_for_tests(engine_mention_context(location.clone()));
        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: String::new(),
                attachments: Vec::new(),
                mentions: vec![mention.clone()],
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "후속 요청".to_string(),
                attachments: Vec::new(),
                mentions: vec![mention.clone()],
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        let prompts = handle.prompts();
        assert_resolved_before_user(&prompts[0]);
        assert!(prompts[0].contains("참조한 리소스를 바탕으로 요청을 수행해 주세요."));
        assert_resolved_before_user(&prompts[1]);

        let plan_driver = FakeCodexDriver::scripted([
            AgentTurnResult::Plan {
                markdown: "initial".to_string(),
            },
            AgentTurnResult::Plan {
                markdown: "revised".to_string(),
            },
        ]);
        let plan_handle = plan_driver.clone();
        let mut plan_engine = test_engine(plan_driver, CapturingEventSink::default());
        plan_engine.executor.plan_runtime = Some(plan_engine.runtime.clone());
        plan_engine
            .runtime
            .mentions()
            .set_context_for_tests(engine_mention_context(location));
        plan_engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "계획해 줘".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        plan_engine
            .plan_feedback(crate::ipc::PlanFeedbackRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "이 리소스를 반영해 줘".to_string(),
                attachments: Vec::new(),
                mentions: vec![mention],
            })
            .await
            .unwrap();
        assert_resolved_before_user(&plan_handle.prompts()[1]);
    }

    #[tokio::test]
    async fn stale_mentions_make_zero_codex_calls_and_visible_text_has_no_authority() {
        let (location, mut mention) = engine_location_mention();
        let driver = FakeCodexDriver::scripted([]);
        let handle = driver.clone();
        let mut engine = test_engine(driver, CapturingEventSink::default());
        engine
            .runtime
            .mentions()
            .set_context_for_tests(engine_mention_context(location));
        let crate::mentions::MentionSnapshot::MapLocation(snapshot) = &mut mention.mention else {
            unreachable!()
        };
        snapshot.location_fingerprint = "c".repeat(64);
        let error = engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "@회복 지점에서 치료해 줘".to_string(),
                attachments: Vec::new(),
                mentions: vec![mention],
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap_err();
        assert!(error.message.contains("변경"));
        assert!(handle.prompts().is_empty());

        let plain_driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "plain".to_string(),
        }]);
        let plain_handle = plain_driver.clone();
        let mut plain_engine = test_engine(plain_driver, CapturingEventSink::default());
        plain_engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "@회복 지점에서 치료해 줘".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();
        assert!(!plain_handle.prompts()[0].contains("eud-resolved-mentions/1"));
    }

    #[tokio::test]
    async fn missing_sound_build_keeps_review_and_blocks_accept_without_releasing_lease() {
        let mut engine = test_engine(FakeCodexDriver::scripted([]), CapturingEventSink::default());
        let request_id = "sound-missing-build";
        engine.runtime.begin_request(request_id, "Sample").unwrap();
        engine
            .runtime
            .register_write_request("sound import")
            .unwrap();
        engine.runtime.require_sound_build_for_tests();
        let before_hash = "1".repeat(64);
        let after_hash = "2".repeat(64);
        let normalized_hash = "3".repeat(64);
        let backup_path = PathBuf::from("C:/backups/sound.bak");
        engine
            .runtime
            .journal()
            .record(
                request_id,
                journal::JournalEntry {
                    id: "sound-1".to_string(),
                    seq: 1,
                    tool: journal::WriteTool::MapSound,
                    target: journal::JournalTarget::MapSound {
                        source_map: PathBuf::from("C:/maps/source.scx"),
                        mpq_path: "staredit\\wav\\ea_3333333333333333.ogg".to_string(),
                        normalized_sha256: normalized_hash.clone(),
                    },
                    before: journal::Snapshot::MapBackup {
                        map_path: "C:/maps/source.scx".to_string(),
                        backup_path: backup_path.to_string_lossy().into_owned(),
                    },
                    after: journal::Snapshot::MapSound {
                        source_sha256: "4".repeat(64),
                        source_codec: "flac".to_string(),
                        duration_ms: 1_000,
                        channels: 2,
                        sample_rate: 44_100,
                        normalization_profile: "8.1.2;ogg/vorbis/44100/stereo/q4".to_string(),
                        normalized_sha256: normalized_hash,
                        normalized_bytes: 1_024,
                        mpq_path: "staredit\\wav\\ea_3333333333333333.ogg".to_string(),
                        wav_index: 1,
                        string_id: 2,
                        map_sha256_before: before_hash,
                        map_sha256_after: after_hash,
                        backup_path,
                        native_report_sha256: "5".repeat(64),
                        map_bytes_before: 10,
                        map_bytes_after: 1_034,
                        source_display_name: "theme.flac".to_string(),
                        edit: None,
                    },
                    ts: 1,
                },
            )
            .unwrap();
        engine.current_request_id = Some(request_id.to_string());
        engine.phase = Phase::Executing;

        let error = engine
            .settle_request()
            .await
            .expect_err("a sound import that was never built cannot settle");

        assert!(error.message.contains("build_run"), "{}", error.message);
        assert_eq!(engine.phase, Phase::Executing);
        assert!(engine.runtime.owns_write_registration());
        assert_eq!(
            engine
                .runtime
                .journal()
                .changeset(request_id)
                .unwrap()
                .items
                .len(),
            1
        );
    }

    #[test]
    fn map_system_prompt_pins_candidate_authority_and_user_only_apply() {
        let prompt = build_map_system_prompt("[project state]\nproject=Map", None);
        assert!(prompt.contains("MapMentionSnapshot"));
        assert!(prompt.contains("entire current candidate is writable"));
        assert!(prompt
            .contains("Never refuse mutation or ask for a region merely because target is absent"));
        assert!(prompt.contains("target region narrows coordinate-based writes"));
        assert!(prompt.contains("Protect masks always block"));
        assert!(prompt.contains("map_candidate_finalize once at most"));
        assert!(prompt.contains("Original Apply and backup restore are intentionally absent"));
        assert!(prompt.contains("terrain, units, buildings, doodads, sprites, and locations"));
        assert!(prompt.contains("Semantic ISOM transitions outside the current request scope"));
        assert!(prompt.contains("map_palette_query is a bounded search"));
        assert!(prompt.contains("search brushes by name first"));
        assert!(prompt.contains("use brushes for semanticTerrain"));
        assert!(prompt.contains("Never enumerate tile ids or catalog pages"));
        assert!(prompt.contains("use map_stamp_preview and map_stamp_place"));
        assert!(prompt.contains("Never reconstruct either source"));
        assert!(prompt.contains("Never guess a collision policy"));
        assert!(prompt.contains("never run ISOM correction"));
        assert!(prompt.contains("authorized only by an importedStamp mention"));
        assert!(prompt.contains("use only imported sources mentioned in the current request"));
        assert!(prompt.contains("Filesystem paths, pickers, blob paths, raw CHK"));
        assert!(prompt.contains("imageRef is an input binding, never extra write authority"));
        assert!(prompt.contains("When the user asks only to inspect, compare, or analyze an image"));
        assert!(prompt.contains("Multiple photos and ordinary terrain patches"));
        assert!(prompt.contains("[draft patch operations]"));
        assert!(prompt.contains("Never invent keys such as tileId"));
        assert!(prompt.contains("- terrain.set: x, y, before, after\n"));
        assert!(prompt.contains("terrain.isom_rect fills a tile rectangle"));
        assert!(prompt.contains("isomX + isomY must be even"));
        assert!(prompt.contains("never conclude a map lacks ISOM data"));
        assert!(prompt.contains("- location.delete: locationId\n"));
        assert!(prompt.contains("before must equal the current tile id at (x, y)"));
        assert!(prompt.contains("map_terrain_read (visible candidate)"));
        assert!(prompt.contains("map_draft_terrain_read (request draft)"));
        assert!(prompt.contains("use terrain.rect / terrain.blit"));
        assert!(prompt.contains("Never guess before values or probe them through conflict errors"));
        assert!(prompt.contains("numeric ids from map_palette_query, never names"));
        assert!(
            prompt.contains("Never provide a filesystem path, palette, MTXM id, or tile matrix")
        );
    }
    #[test]
    fn audio_request_selects_only_audio_workflow_guidance() {
        let cold = build_system_prompt("배경음악", &[], "[project state]", None, None);
        assert!(cold.lines().any(|line| line == "[map sounds]"));
        assert!(!cold.lines().any(|line| line == "[direct python]"));
        let resumed = assembled_followup("계속", None, None, None, None);
        assert!(!resumed.contains("[map sounds]"));
        let map = build_map_system_prompt("[project state]", None);
        assert!(!map.contains("map_sound_import"));
        assert!(!map.contains("[map sounds]"));
    }
    #[tokio::test]
    async fn autonomous_mode_continues_typed_boundaries_with_minimal_context() {
        let (base, memory) = memory_store("autonomous-boundary");
        let driver = FakeCodexDriver::scripted([
            AgentTurnResult::IterationBoundary {
                reason: crate::provider_runtime::IterationBoundaryReason::ToolRounds,
            },
            AgentTurnResult::Answer {
                text: "완료".to_string(),
            },
        ]);
        let driver_handle = driver.clone();
        let sink = CapturingEventSink::default();
        let mut engine = test_engine_with_memory(driver, sink, memory, &base.join("data"));
        bind_workspace_snapshot(&engine.runtime, "Sample");

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "여러 반복이 필요한 목표".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: crate::autonomous::ExecutionMode::Autonomous,
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();

        let prompts = driver_handle.prompts();
        assert_eq!(prompts.len(), 2);
        assert!(prompts[0].contains("[autonomous execution]"));
        assert!(prompts[1].contains("[autonomous continuation]"));
        assert!(prompts[1].contains("여러 반복이 필요한 목표"));
        assert!(!prompts[1].contains("[eps project architecture]"));
        let run = engine
            .session_store
            .load(&engine.session_id)
            .unwrap()
            .autonomous_run
            .unwrap();
        assert_eq!(
            run.status,
            crate::autonomous::AutonomousRunStatus::Completed
        );
        assert_eq!(run.iteration, 2);
        assert!(run.last_checkpoint.unwrap().is_started());
        fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn autonomous_turn_after_an_unanswered_ask_pauses_and_resume_carries_the_blocker() {
        let (base, memory) = memory_store("autonomous-unanswered-ask");
        let driver = FakeCodexDriver::scripted([
            AgentTurnResult::Answer {
                text: "어떤 방식을 사용할까요? A 또는 B".to_string(),
            },
            AgentTurnResult::Answer {
                text: "완료".to_string(),
            },
        ]);
        let driver_handle = driver.clone();
        let sink = CapturingEventSink::default();
        let mut engine = test_engine_with_memory(driver, sink, memory, &base.join("data"));
        bind_workspace_snapshot(&engine.runtime, "Sample");
        engine.runtime.set_ask_emitter(|_| Ok(()));
        engine
            .runtime
            .set_ask_wait_timeout(std::time::Duration::from_millis(30));
        *driver_handle.ask_runtime.lock().unwrap() = Some(engine.runtime.clone());

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "사용자 선택이 필요한 목표".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: crate::autonomous::ExecutionMode::Autonomous,
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();

        let run = engine
            .session_store
            .load(&engine.session_id)
            .unwrap()
            .autonomous_run
            .unwrap();
        assert_eq!(run.status, crate::autonomous::AutonomousRunStatus::Paused);
        assert_eq!(
            run.pause_reason,
            Some(crate::autonomous::AutonomousPauseReason::UnansweredAsk)
        );
        assert!(run.active_started_at.is_none());
        let blocker = run
            .blocker
            .clone()
            .expect("pause blocker names the unanswered ask");
        assert!(blocker.contains("같은 질문을 다시"));
        assert!(run.status.can_resume());
        assert!(engine.runtime.pending_ask().is_none());

        engine.autonomous_resume().await.unwrap();

        let prompts = driver_handle.prompts();
        assert_eq!(prompts.len(), 2);
        assert!(prompts[1].contains("[autonomous continuation]"));
        assert!(prompts[1].contains(&blocker));
        let run = engine
            .session_store
            .load(&engine.session_id)
            .unwrap()
            .autonomous_run
            .unwrap();
        assert_eq!(
            run.status,
            crate::autonomous::AutonomousRunStatus::Completed
        );
        assert_eq!(run.pause_reason, None);
        fs::remove_dir_all(base).ok();
    }

    /// A paused autonomous run whose checkpoint names the project (written
    /// before project ids) resumes in that project, and its checkpoint is
    /// rewritten to the id; a checkpoint naming another project stops.
    #[tokio::test]
    async fn a_checkpoint_that_names_the_open_project_resumes_under_its_id() {
        for (recorded, resumes) in [("Sample", true), ("Other", false)] {
            let (base, memory) = memory_store("autonomous-legacy-project");
            let driver = FakeCodexDriver::scripted([
                AgentTurnResult::Answer {
                    text: "어떤 방식을 사용할까요? A 또는 B".to_string(),
                },
                AgentTurnResult::Answer {
                    text: "완료".to_string(),
                },
            ]);
            let driver_handle = driver.clone();
            let mut engine = test_engine_with_memory(
                driver,
                CapturingEventSink::default(),
                memory,
                &base.join("data"),
            );
            bind_workspace_snapshot(&engine.runtime, "Sample");
            let open = engine.runtime.services_native_open().unwrap();
            engine.project_id = crate::project_id::ProjectId::of(&open);
            engine.runtime.set_ask_emitter(|_| Ok(()));
            engine
                .runtime
                .set_ask_wait_timeout(std::time::Duration::from_millis(30));
            *driver_handle.ask_runtime.lock().unwrap() = Some(engine.runtime.clone());
            engine
                .chat(crate::ipc::ChatRequest {
                    client_turn_id: crate::ipc::new_client_turn_id(),
                    text: "사용자 선택이 필요한 목표".to_string(),
                    attachments: Vec::new(),
                    mentions: Vec::new(),
                    execution_mode: crate::autonomous::ExecutionMode::Autonomous,
                    autonomous_policy: None,
                    clarify: false,
                    skill_selections: Default::default(),
                })
                .await
                .unwrap();
            let mut run = engine
                .session_store
                .load(&engine.session_id)
                .unwrap()
                .autonomous_run
                .unwrap();
            assert_eq!(run.status, crate::autonomous::AutonomousRunStatus::Paused);
            assert_eq!(run.project_id, engine.project_id.as_str());
            // The checkpoint an older build wrote named the project.
            run.project_id = recorded.to_string();
            engine
                .session_store
                .set_autonomous_run(&engine.session_id, Some(run))
                .unwrap();

            let result = engine.autonomous_resume().await;

            let run = engine
                .session_store
                .load(&engine.session_id)
                .unwrap()
                .autonomous_run
                .unwrap();
            if resumes {
                result.unwrap();
                assert_eq!(
                    run.status,
                    crate::autonomous::AutonomousRunStatus::Completed
                );
                assert_eq!(run.project_id, engine.project_id.as_str());
            } else {
                assert!(result.is_err());
                assert_eq!(
                    run.status,
                    crate::autonomous::AutonomousRunStatus::SafetyStopped
                );
                assert_eq!(run.project_id, "Other");
            }
            fs::remove_dir_all(base).ok();
        }
    }

    #[test]
    fn ask_wait_copy_matches_the_shared_timeout_constant() {
        let seconds = format!("{} seconds", crate::tools::ASK_WAIT_TIMEOUT.as_secs());
        let baseline = static_prompt_baseline(crate::provider::ProviderId::Codex);
        assert!(baseline.contains(&format!(
            "{}-second",
            crate::tools::ASK_WAIT_TIMEOUT.as_secs()
        )));
        let ask = crate::tools::tool_spec(crate::tools::ASK_TOOL).unwrap();
        assert!(ask.description.contains(&seconds));
    }

    #[tokio::test]
    async fn interactive_mode_continues_at_a_typed_boundary_without_creating_autonomous_state() {
        let (base, memory) = memory_store("interactive-boundary");
        let driver = FakeCodexDriver::scripted([
            AgentTurnResult::IterationBoundary {
                reason: crate::provider_runtime::IterationBoundaryReason::ToolRounds,
            },
            AgentTurnResult::IterationBoundary {
                reason: crate::provider_runtime::IterationBoundaryReason::ToolActions,
            },
            AgentTurnResult::Answer {
                text: "일반 완료".to_string(),
            },
        ]);
        let driver_handle = driver.clone();
        let sink = CapturingEventSink::default();
        let sink_handle = sink.clone();
        let mut engine = test_engine_with_memory(driver, sink, memory, &base.join("data"));

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "일반 실행".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: crate::autonomous::ExecutionMode::Interactive,
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();

        let prompts = driver_handle.prompts();
        assert_eq!(prompts.len(), 3);
        assert!(prompts[0].contains("일반 실행"));
        assert!(!prompts[0].contains("[autonomous execution]"));
        assert!(prompts[1].starts_with("[continuation]"));
        assert!(prompts[1].contains("ToolRounds"));
        assert!(prompts[2].starts_with("[continuation]"));
        assert!(prompts[2].contains("ToolActions"));
        assert!(engine
            .session_store
            .load(&engine.session_id)
            .unwrap()
            .autonomous_run
            .is_none());
        let events = sink_handle.events();
        assert!(events.iter().any(
            |event| matches!(event, EngineEvent::Answer(answer) if answer.text == "일반 완료")
        ));
        assert!(!events.iter().any(
            |event| matches!(event, EngineEvent::Answer(answer) if answer.text.contains("일시 중지"))
        ));
        fs::remove_dir_all(base).ok();
    }
    #[tokio::test]
    async fn autonomous_resume_requires_exact_checkpoint_and_project_revision() {
        for (tag, revision_matches) in [
            ("autonomous-resume-valid", true),
            ("autonomous-resume-stale", false),
        ] {
            let (base, memory) = memory_store(tag);
            let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
                text: "재개 완료".to_string(),
            }]);
            let driver_handle = driver.clone();
            let sink = CapturingEventSink::default();
            let mut engine = test_engine_with_memory(driver, sink, memory, &base.join("data"));
            bind_workspace_snapshot(&engine.runtime, "Sample");
            let checkpoint = engine.executor.conversation_state();
            let current_revision = engine.runtime.current_project_revision().unwrap();
            let mut run = crate::autonomous::AutonomousRunState::new(
                "재개할 목표".to_string(),
                crate::ipc::new_client_turn_id(),
                format!("req-{tag}"),
                engine.project_id.as_str().to_string(),
                if revision_matches {
                    current_revision
                } else {
                    "stale-revision".to_string()
                },
                Default::default(),
                crate::session::now_unix_millis(),
            );
            run.status = crate::autonomous::AutonomousRunStatus::PausedAfterRestart;
            run.pause_reason = Some(crate::autonomous::AutonomousPauseReason::Restart);
            run.active_started_at = None;
            run.last_checkpoint = Some(checkpoint);
            engine
                .session_store
                .set_autonomous_run(&engine.session_id, Some(run))
                .unwrap();

            let result = engine.autonomous_resume().await;
            let persisted = engine
                .session_store
                .load(&engine.session_id)
                .unwrap()
                .autonomous_run
                .unwrap();
            if revision_matches {
                result.unwrap();
                assert_eq!(
                    persisted.status,
                    crate::autonomous::AutonomousRunStatus::Completed
                );
                assert_eq!(driver_handle.prompts().len(), 1);
                assert!(driver_handle.prompts()[0].contains("[autonomous continuation]"));
                assert!(driver_handle.prompts()[0].contains("재개할 목표"));
            } else {
                assert!(result.unwrap_err().message.contains("프로젝트"));
                assert_eq!(
                    persisted.status,
                    crate::autonomous::AutonomousRunStatus::SafetyStopped
                );
                assert!(driver_handle.prompts().is_empty());
            }
            fs::remove_dir_all(base).ok();
        }
    }

    #[test]
    fn autonomous_completion_requires_a_successful_build_for_current_revision() {
        let (base, memory) = memory_store("autonomous-build-gate");
        let driver = FakeCodexDriver::scripted([]);
        let sink = CapturingEventSink::default();
        let mut engine = test_engine_with_memory(driver, sink, memory, &base.join("data"));
        bind_workspace_snapshot(&engine.runtime, "Sample");
        let request_id = "req-autonomous-build";
        engine.current_request_id = Some(request_id.to_string());
        engine
            .runtime
            .begin_request(request_id, engine.project_id.as_str())
            .unwrap();
        record_file_write_in_memory(
            &engine.journal_store,
            request_id,
            "write-build",
            1,
            "src/main.eps",
        );

        assert!(engine
            .autonomous_completion_blocker()
            .unwrap()
            .unwrap()
            .contains("build_run"));

        let revision = engine.runtime.current_project_revision().unwrap();
        engine
            .runtime
            .restore_autonomous_progress(
                request_id,
                &crate::autonomous::AutonomousRunProgress {
                    latest_build: Some(crate::autonomous::AutonomousBuildStatus {
                        input_revision: revision,
                        diagnostics_fingerprint: "success".to_string(),
                        error_count: 0,
                        success: true,
                        consecutive_no_progress: 0,
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(engine.autonomous_completion_blocker().unwrap(), None);
        fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn accepted_autonomous_review_completes_while_paused_review_stays_resumable() {
        for (tag, initial_status, expected_status) in [
            (
                "autonomous-review",
                crate::autonomous::AutonomousRunStatus::Review,
                crate::autonomous::AutonomousRunStatus::Completed,
            ),
            (
                "autonomous-paused-review",
                crate::autonomous::AutonomousRunStatus::Paused,
                crate::autonomous::AutonomousRunStatus::Paused,
            ),
        ] {
            let (base, memory) = memory_store(tag);
            let driver = FakeCodexDriver::scripted([]);
            let sink = CapturingEventSink::default();
            let mut engine = test_engine_with_memory(driver, sink, memory, &base.join("data"));
            bind_workspace_snapshot(&engine.runtime, "Sample");
            let request_id = format!("req-{tag}");
            engine.execution_mode = crate::autonomous::ExecutionMode::Autonomous;
            engine.current_request_id = Some(request_id.clone());
            engine.current_client_turn_id = Some(crate::ipc::new_client_turn_id());
            engine
                .runtime
                .begin_request(&request_id, engine.project_id.as_str())
                .unwrap();
            engine
                .runtime
                .register_write_request("review fixture")
                .unwrap();
            record_file_write(
                &engine.journal_store,
                &request_id,
                "review-write",
                1,
                "src/main.eps",
            );
            let mut run = crate::autonomous::AutonomousRunState::new(
                "검토할 목표".to_string(),
                engine.current_client_turn_id.clone().unwrap(),
                request_id.clone(),
                engine.project_id.as_str().to_string(),
                engine.runtime.current_project_revision().unwrap(),
                Default::default(),
                crate::session::now_unix_millis(),
            );
            run.status = initial_status;
            if initial_status == crate::autonomous::AutonomousRunStatus::Paused {
                run.pause_reason = Some(crate::autonomous::AutonomousPauseReason::User);
            }
            engine
                .session_store
                .set_autonomous_run(&engine.session_id, Some(run))
                .unwrap();

            engine.settle_request().await.unwrap();

            let persisted = engine
                .session_store
                .load(&engine.session_id)
                .unwrap()
                .autonomous_run
                .unwrap();
            assert_eq!(persisted.status, expected_status);
            assert_eq!(
                persisted.project_revision,
                engine.runtime.current_project_revision().unwrap()
            );
            if expected_status == crate::autonomous::AutonomousRunStatus::Paused {
                assert!(persisted.status.can_resume());
                assert_eq!(
                    persisted.pause_reason,
                    Some(crate::autonomous::AutonomousPauseReason::User)
                );
            }
            fs::remove_dir_all(base).ok();
        }
    }

    /// A temp project with the app's own repository, or `None` without git.
    fn owned_commit_repo(tag: &str) -> Option<PathBuf> {
        if !crate::git::available() {
            eprintln!("skipping {tag}: git is not installed on this machine");
            return None;
        }
        let root = std::env::temp_dir().join(format!(
            "eud-agent-owned-commit-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        for file in [
            "src/a.eps",
            "src/b.eps",
            "src/shared.eps",
            "src/external.eps",
        ] {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "const v = 1;").unwrap();
        }
        assert!(crate::git::prepare(&root).auto_commit_allowed());
        for file in [
            "src/a.eps",
            "src/b.eps",
            "src/shared.eps",
            "src/external.eps",
        ] {
            fs::write(root.join(file), "const v = 2;").unwrap();
        }
        Some(root)
    }

    fn committed_files(root: &std::path::Path, sha: &str) -> Vec<String> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["show", "--name-only", "--format=", sha])
            .output()
            .unwrap();
        let mut files: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string)
            .collect();
        files.sort();
        files
    }

    fn commit_body(root: &std::path::Path, sha: &str) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["log", "-1", "--format=%B", sha])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn two_requests_in_flight() -> crate::write_coordinator::ProjectWriteCoordinator {
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        writes.request("project", "session-a", "request-a").unwrap();
        writes.request("project", "session-b", "request-b").unwrap();
        writes
    }

    #[test]
    fn a_turn_commit_leaves_the_files_another_running_request_claimed() {
        let Some(root) = owned_commit_repo("exclusion") else {
            return;
        };
        let writes = two_requests_in_flight();
        writes.claim(
            "project",
            "session-a",
            "request-a",
            ["src/a.eps".to_string()],
        );
        writes.claim(
            "project",
            "session-b",
            "request-b",
            ["src/b.eps".to_string()],
        );

        let committed = commit_owned_paths(
            &root,
            &writes,
            "project",
            "session-a",
            "request-a",
            "A의 변경",
        )
        .unwrap();

        let record = committed.record.expect("a commit");
        assert!(committed.warning.is_none());
        // A's file and the unclaimed edits go in; B's in-flight file does not.
        assert_eq!(
            committed_files(&root, &record.sha),
            vec!["src/a.eps", "src/external.eps", "src/shared.eps"]
        );
        assert_eq!(crate::git::dirty_paths(&root).unwrap(), vec!["src/b.eps"]);

        // B's own boundary then records B's work.
        let b = commit_owned_paths(
            &root,
            &writes,
            "project",
            "session-b",
            "request-b",
            "B의 변경",
        )
        .unwrap()
        .record
        .expect("a commit");
        assert_eq!(committed_files(&root, &b.sha), vec!["src/b.eps"]);
        assert!(crate::git::dirty_paths(&root).unwrap().is_empty());
    }

    #[test]
    fn the_turn_start_external_commit_skips_files_running_turns_claimed() {
        let Some(root) = owned_commit_repo("external") else {
            return;
        };
        let writes = two_requests_in_flight();
        writes.claim(
            "project",
            "session-a",
            "request-a",
            ["src/a.eps".to_string()],
        );
        writes.claim(
            "project",
            "session-b",
            "request-b",
            ["src/b.eps".to_string(), "src/shared.eps".to_string()],
        );

        let external =
            crate::git::commit_external_edits_except(&root, &writes.claimed_in_flight("project"))
                .unwrap()
                .expect("a commit");

        assert_eq!(
            committed_files(&root, &external.sha),
            vec!["src/external.eps"]
        );
        assert!(external.subject.starts_with("외부 편집"));
        let mut dirty = crate::git::dirty_paths(&root).unwrap();
        dirty.sort();
        assert_eq!(dirty, vec!["src/a.eps", "src/b.eps", "src/shared.eps"]);
    }

    #[test]
    fn a_file_two_running_requests_changed_is_committed_once_with_a_shared_trailer() {
        let Some(root) = owned_commit_repo("shared") else {
            return;
        };
        let writes = two_requests_in_flight();
        writes.claim(
            "project",
            "session-a",
            "request-a",
            ["src/a.eps".to_string(), "src/shared.eps".to_string()],
        );
        writes.claim(
            "project",
            "session-b",
            "request-b",
            ["src/b.eps".to_string(), "src/shared.eps".to_string()],
        );

        let committed = commit_owned_paths(
            &root,
            &writes,
            "project",
            "session-a",
            "request-a",
            "A의 변경",
        )
        .unwrap();

        let record = committed.record.expect("a commit");
        assert_eq!(
            committed_files(&root, &record.sha),
            vec!["src/a.eps", "src/external.eps", "src/shared.eps"]
        );
        let body = commit_body(&root, &record.sha);
        assert!(body.contains("session: session-a"), "{body}");
        assert!(body.contains("shared-with: session-b"), "{body}");
        assert_eq!(
            committed.warning.as_deref(),
            Some("다른 세션과 같은 파일을 함께 변경했습니다: src/shared.eps")
        );
        assert_eq!(crate::git::dirty_paths(&root).unwrap(), vec!["src/b.eps"]);
    }

    #[test]
    fn with_no_other_request_in_flight_a_turn_commits_everything_as_before() {
        let Some(root) = owned_commit_repo("alone") else {
            return;
        };
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        writes.request("project", "session-a", "request-a").unwrap();
        writes.claim(
            "project",
            "session-a",
            "request-a",
            ["src/a.eps".to_string()],
        );
        // A request of another project does not hold this one's files back.
        writes.request("other", "session-c", "request-c").unwrap();
        writes.claim("other", "session-c", "request-c", ["src/b.eps".to_string()]);

        let committed = commit_owned_paths(
            &root,
            &writes,
            "project",
            "session-a",
            "request-a",
            "A의 변경",
        )
        .unwrap();

        let record = committed.record.expect("a commit");
        assert!(committed.warning.is_none());
        assert_eq!(record.files, 4);
        assert_eq!(
            committed_files(&root, &record.sha),
            vec![
                "src/a.eps",
                "src/b.eps",
                "src/external.eps",
                "src/shared.eps"
            ]
        );
        let body = commit_body(&root, &record.sha);
        assert!(!body.contains("shared-with"), "{body}");
        assert!(crate::git::dirty_paths(&root).unwrap().is_empty());
    }

    #[test]
    fn a_committed_file_stops_holding_back_other_sessions_commits() {
        let Some(root) = owned_commit_repo("unclaim") else {
            return;
        };
        let writes = two_requests_in_flight();
        writes.claim(
            "project",
            "session-a",
            "request-a",
            ["src/a.eps".to_string()],
        );
        writes.claim(
            "project",
            "session-b",
            "request-b",
            ["src/b.eps".to_string()],
        );

        // A's first autonomous iteration commits its file and keeps running.
        commit_owned_paths(&root, &writes, "project", "session-a", "request-a", "1회차")
            .unwrap()
            .record
            .expect("a commit");
        assert!(writes.claimed_by("request-a").is_empty());
        assert!(writes.owns("project", "session-a", "request-a"));

        // The user then edits A's file outside the app; B's boundary records it
        // without calling it a file both sessions changed.
        fs::write(root.join("src/a.eps"), "const v = 3;").unwrap();
        let b =
            commit_owned_paths(&root, &writes, "project", "session-b", "request-b", "B").unwrap();
        let record = b.record.expect("a commit");
        assert!(b.warning.is_none());
        assert_eq!(
            committed_files(&root, &record.sha),
            vec!["src/a.eps", "src/b.eps"]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_git_commit_waiting_on_a_long_transaction_leaves_the_runtime_free() {
        // Given: another session's build holds the project transaction.
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let (held, held_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = {
            let writes = writes.clone();
            std::thread::spawn(move || {
                writes
                    .transaction("project", || {
                        held.send(()).unwrap();
                        // Released by the test, or after a bound so a hang fails instead.
                        let _ = release_rx.recv_timeout(std::time::Duration::from_secs(10));
                    })
                    .unwrap();
            })
        };
        held_rx.recv().unwrap();

        // When: a turn boundary commits on the only runtime thread.
        let commit = tokio::spawn(run_git_mutation(
            writes.clone(),
            "project".to_string(),
            || Ok(7),
        ));
        let started = std::time::Instant::now();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Then: the runtime kept running while the commit waited, and the
        // commit ran once the transaction was free.
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(!commit.is_finished());
        release.send(()).unwrap();
        assert_eq!(commit.await.unwrap(), Ok(7));
        holder.join().unwrap();
    }

    fn worker_record(id: &str, status: crate::worker::WorkerStatus) -> crate::worker::WorkerRecord {
        let mut worker = crate::worker::WorkerRecord::new(
            format!("child-{id}"),
            "req".to_string(),
            id.to_string(),
            "goal".to_string(),
        );
        worker.id = id.to_string();
        worker.status = status;
        worker
    }

    #[tokio::test]
    async fn a_parent_turn_sees_worker_settlements_once_and_later_ones_stay_unreported() {
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "reviewed".to_string(),
        }]);
        let driver_handle = driver.clone();
        let mut engine = test_engine(driver, CapturingEventSink::default());
        let sessions = engine.session_store.clone();
        let session_id = engine.session_id.clone();
        let mut record = sessions.load(&session_id).unwrap();
        let mut settled = worker_record("w-done", crate::worker::WorkerStatus::Settled);
        settled.summary = Some("메뉴를 정리했습니다.".to_string());
        record.workers = vec![
            settled,
            worker_record("w-live", crate::worker::WorkerStatus::Running),
        ];
        sessions.save(&record).unwrap();

        engine
            .chat(crate::ipc::ChatRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: crate::worker::WORKER_CONTINUE_TEXT.to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
                execution_mode: Default::default(),
                autonomous_policy: None,
                clarify: false,
                skill_selections: Default::default(),
            })
            .await
            .unwrap();

        let prompt = driver_handle.prompts().pop().unwrap();
        assert!(prompt.contains("[workers]\n"), "{prompt}");
        assert!(prompt.contains("- w-done status=settled"));
        assert!(prompt.contains("summary: 메뉴를 정리했습니다."));
        assert!(prompt.contains("- w-live status=running"));
        let record = sessions.load(&session_id).unwrap();
        assert!(record.workers[0].reported, "the settlement was shown");
        assert!(
            !record.workers[1].reported,
            "a running worker is not reported"
        );
        assert_eq!(crate::worker::next_continuation(&record), None);

        // The running worker settles while (or after) the parent turn ran: it
        // is still unreported, so the parent is continued again.
        sessions
            .update_worker(&session_id, "w-live", |worker| {
                crate::worker::settle(worker, Ok(Default::default()));
                true
            })
            .unwrap();
        assert_eq!(
            crate::worker::next_continuation(&sessions.load(&session_id).unwrap()).as_deref(),
            Some("w-live")
        );
    }

    #[tokio::test]
    async fn a_worker_session_commits_under_its_title_and_names_its_parent() {
        if !crate::git::available() {
            eprintln!("skipping worker commit: git is not installed on this machine");
            return;
        }
        let runtime = test_tool_runtime();
        let root = runtime.project_root().unwrap();
        let data_dir = unique_temp_dir("worker-commit");
        let sessions = session_store_at(&data_dir);
        let parent = test_session(&sessions);
        let child = crate::worker::child_session(&parent, "메뉴 정리");
        sessions.save(&child).unwrap();
        let (_cancellation_tx, cancellation_rx) = tokio::sync::watch::channel(0_u64);
        let mut engine = AgentEngine::new(
            FakeCodexDriver::scripted([]),
            CapturingEventSink::default(),
            AgentEngineConfig::for_tests("[project state]\nproject=Sample", None, Vec::new()),
            runtime,
            sessions,
            attachment_store_at(&data_dir),
            child.clone(),
            cancellation_rx,
        );
        assert!(crate::git::prepare(&root).auto_commit_allowed());
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/menu.eps"), "const menu = 2;").unwrap();
        engine.last_answer = "메뉴를 정리했습니다.".to_string();

        let record = engine
            .commit_boundary("the user text", "req-child")
            .await
            .expect("the worker turn is committed");

        assert_eq!(record.subject, "[작업자] 메뉴 정리");
        let body = commit_body(&root, &record.sha);
        assert!(
            body.contains(&format!("session: {}", child.meta.id)),
            "{body}"
        );
        assert!(body.contains("request: req-child"), "{body}");
        assert!(
            body.contains(&format!("parent-session: {}", parent.meta.id)),
            "{body}"
        );
        let outcome = engine.worker_turn_outcome();
        assert_eq!(outcome.answer, "메뉴를 정리했습니다.");
        assert_eq!(outcome.commits, vec![record.sha.clone()]);
        assert!(outcome.changed_files.contains(&"src/menu.eps".to_string()));
        assert!(!outcome.cancelled);
    }

    #[tokio::test]
    async fn a_release_is_tagged_on_the_turn_commit_only_after_its_build_succeeded() {
        if !crate::git::available() {
            eprintln!("skipping release tag: git is not installed on this machine");
            return;
        }
        let runtime = test_tool_runtime();
        let root = runtime.project_root().unwrap();
        let data_dir = unique_temp_dir("release-tag");
        let sessions = session_store_at(&data_dir);
        let session = test_session(&sessions);
        let sink = CapturingEventSink::default();
        let (_cancellation_tx, cancellation_rx) = tokio::sync::watch::channel(0_u64);
        let engine = AgentEngine::new(
            FakeCodexDriver::scripted([]),
            sink.clone(),
            AgentEngineConfig::for_tests("[project state]\nproject=Sample", None, Vec::new()),
            runtime.clone(),
            sessions,
            attachment_store_at(&data_dir),
            session,
            cancellation_rx,
        );
        assert!(crate::git::prepare(&root).auto_commit_allowed());
        runtime
            .begin_request("req-release", engine.project_id.as_str())
            .unwrap();
        runtime.register_write_request("release").unwrap();
        runtime
            .execute(
                crate::tools::RELEASE_PREPARE_TOOL,
                &json!({"title": "데모"}),
            )
            .unwrap();
        fs::write(root.join("CHANGELOG.md"), "# 변경 기록\n\n## v1.0\n").unwrap();
        let tags = || crate::git::tags_matching(&root, "release/*").unwrap();

        // A boundary before any successful build records the work, not the release.
        engine
            .commit_boundary("배포", "req-release")
            .await
            .expect("the release work is committed");
        assert!(tags().is_empty());
        assert_eq!(
            runtime.pending_release("req-release").as_deref(),
            Some("v1.0")
        );
        let warned = sink.events().iter().any(|event| {
            matches!(event, EngineEvent::Git(ipc::GitTurnEvent { warning: Some(warning), .. })
                if warning.contains("release/v1.0"))
        });
        assert!(warned, "the user is told why there is no tag");

        // After a successful build of the current revision the next commit is tagged.
        fs::write(
            root.join("CHANGELOG.md"),
            "# 변경 기록\n\n## v1.0\n\n- 시작\n",
        )
        .unwrap();
        let revision = runtime.current_project_revision().unwrap();
        runtime
            .restore_autonomous_progress(
                "req-release",
                &crate::autonomous::AutonomousRunProgress {
                    latest_build: Some(crate::autonomous::AutonomousBuildStatus {
                        input_revision: revision,
                        diagnostics_fingerprint: "success".to_string(),
                        error_count: 0,
                        success: true,
                        consecutive_no_progress: 0,
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        let tagged = engine
            .commit_boundary("배포", "req-release")
            .await
            .expect("the release commit");
        assert_eq!(tags(), ["release/v1.0"]);
        let newest = crate::git::commits_since(&root, None, 1).unwrap().0;
        assert_eq!(newest[0].sha, tagged.sha);
        assert_eq!(
            crate::git::commits_since(&root, Some("release/v1.0"), 1)
                .unwrap()
                .1,
            0
        );
        assert_eq!(runtime.pending_release("req-release"), None);
    }

    fn plain_request(text: &str) -> crate::ipc::ChatRequest {
        crate::ipc::ChatRequest {
            client_turn_id: crate::ipc::new_client_turn_id(),
            text: text.to_string(),
            attachments: Vec::new(),
            mentions: Vec::new(),
            execution_mode: Default::default(),
            autonomous_policy: None,
            clarify: false,
            skill_selections: Default::default(),
        }
    }

    /// An engine whose fake driver ends a run cancelled when its identity is
    /// not the current generation, as the production runtime does.
    fn generation_engine(
        session: impl FnOnce(&crate::session::SessionStore) -> crate::session::SessionRecord,
        turns: Vec<AgentTurnResult>,
    ) -> (
        AgentEngine<FakeCodexDriver, CapturingEventSink>,
        FakeCodexDriver,
        tokio::sync::watch::Sender<u64>,
    ) {
        let driver = FakeCodexDriver::scripted(turns);
        let handle = driver.clone();
        let data_dir = unique_temp_dir("worker-engine");
        let sessions = session_store_at(&data_dir);
        let record = session(&sessions);
        let (cancel, cancellation) = tokio::sync::watch::channel(0_u64);
        handle.honor_cancellation(cancellation.clone());
        let engine = AgentEngine::new(
            driver,
            CapturingEventSink::default(),
            AgentEngineConfig::for_tests("[project state]\nproject=Sample", None, Vec::new()),
            test_tool_runtime(),
            sessions,
            attachment_store_at(&data_dir),
            record,
            cancellation,
        );
        (engine, handle, cancel)
    }

    #[tokio::test]
    async fn a_cancel_issued_while_a_turn_waited_for_the_engine_stops_it_before_its_first_step() {
        let (mut engine, driver, cancel) = generation_engine(
            test_session,
            vec![AgentTurnResult::Answer {
                text: "ran".to_string(),
            }],
        );
        // The worker turn read generation 0, then waited for the engine while
        // the worker was stopped.
        cancel.send_replace(1);
        engine
            .chat_from_generation(plain_request("work"), 0)
            .await
            .unwrap();
        assert!(engine.last_turn_cancelled);
        assert!(driver.prompts().is_empty(), "no model step ran");

        // A turn that enters after the cancel belongs to the new generation.
        engine.chat(plain_request("again")).await.unwrap();
        assert!(!engine.last_turn_cancelled);
        assert_eq!(driver.prompts().len(), 1);
    }

    #[tokio::test]
    async fn a_cancel_after_a_turn_entered_stops_it_at_its_first_model_step() {
        let (mut engine, driver, cancel) = generation_engine(
            test_session,
            vec![AgentTurnResult::Answer {
                text: "ran".to_string(),
            }],
        );
        // The cancel lands after entry, before the first model step (while
        // the turn prepared its context or waited for a git commit).
        driver.before_next_foreground(move || {
            cancel.send_replace(1);
        });
        engine.chat(plain_request("work")).await.unwrap();
        assert!(engine.last_turn_cancelled);
        assert!(driver.prompts().is_empty());
    }

    #[tokio::test]
    async fn a_failed_parent_turn_leaves_its_worker_settlements_unreported() {
        let (mut engine, driver, _cancel) = generation_engine(
            |sessions| {
                let mut record = test_session(sessions);
                record.workers = vec![worker_record(
                    "w-done",
                    crate::worker::WorkerStatus::Settled,
                )];
                sessions.save(&record).unwrap();
                record
            },
            vec![AgentTurnResult::Answer {
                text: "reviewed".to_string(),
            }],
        );
        let sessions = engine.session_store.clone();
        let session_id = engine.session_id.clone();
        driver.fail_next_foreground("provider down");
        engine
            .chat(plain_request(crate::worker::WORKER_CONTINUE_TEXT))
            .await
            .unwrap_err();
        let record = sessions.load(&session_id).unwrap();
        assert!(
            !record.workers[0].reported,
            "a failed turn reported nothing"
        );
        assert_eq!(
            crate::worker::next_continuation(&record).as_deref(),
            Some("w-done"),
            "the settlement still continues the parent"
        );
        // What the session manager does after a failed command.
        engine.recover_read_failure().unwrap();

        engine
            .chat(plain_request(crate::worker::WORKER_CONTINUE_TEXT))
            .await
            .unwrap();
        assert!(sessions.load(&session_id).unwrap().workers[0].reported);
    }

    #[tokio::test]
    async fn an_autonomous_run_cannot_start_while_workers_are_active() {
        let (mut engine, driver, _cancel) = generation_engine(
            |sessions| {
                let mut record = test_session(sessions);
                record.workers = vec![worker_record(
                    "w-live",
                    crate::worker::WorkerStatus::Running,
                )];
                sessions.save(&record).unwrap();
                record
            },
            vec![AgentTurnResult::Answer {
                text: "ran".to_string(),
            }],
        );
        let mut request = plain_request("long job");
        request.execution_mode = crate::autonomous::ExecutionMode::Autonomous;
        let error = engine.chat(request).await.unwrap_err();
        assert!(
            error.message.contains("작업자가 실행 중인 동안에는"),
            "{}",
            error.message
        );
        assert!(driver.prompts().is_empty());
        assert!(engine
            .session_store
            .load(&engine.session_id)
            .unwrap()
            .autonomous_run
            .is_none());
    }

    #[tokio::test]
    async fn a_failed_worker_turn_commits_its_edits_as_the_workers() {
        if !crate::git::available() {
            eprintln!("skipping failed worker commit: git is not installed on this machine");
            return;
        }
        let parent_id = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let recorded_parent = parent_id.clone();
        let (mut engine, driver, _cancel) = generation_engine(
            move |sessions| {
                let parent = test_session(sessions);
                *recorded_parent.lock().unwrap() = parent.meta.id.clone();
                let child = crate::worker::child_session(&parent, "메뉴 정리");
                sessions.save(&child).unwrap();
                child
            },
            Vec::new(),
        );
        let root = engine.runtime.project_root().unwrap();
        assert!(crate::git::prepare(&root).auto_commit_allowed());
        let written = root.join("src/menu.eps");
        driver.before_next_foreground(move || {
            fs::create_dir_all(written.parent().unwrap()).unwrap();
            fs::write(&written, "const menu = 2;").unwrap();
        });
        driver.fail_next_foreground("provider down");

        engine.chat(plain_request("메뉴 정리")).await.unwrap_err();

        let outcome = engine.worker_turn_outcome();
        assert_eq!(
            outcome.commits.len(),
            1,
            "the partial edit is the worker's commit"
        );
        assert!(outcome.changed_files.contains(&"src/menu.eps".to_string()));
        let body = commit_body(&root, &outcome.commits[0]);
        assert!(body.starts_with("[작업자] 메뉴 정리"), "{body}");
        assert!(
            body.contains(&format!("parent-session: {}", parent_id.lock().unwrap())),
            "{body}"
        );
        let mut failed = worker_record("w", crate::worker::WorkerStatus::Running);
        let mut outcome = outcome;
        outcome.error = Some("provider down".to_string());
        crate::worker::settle(&mut failed, Ok(outcome));
        assert!(matches!(
            failed.status,
            crate::worker::WorkerStatus::Failed { .. }
        ));
        assert_eq!(failed.commits.len(), 1, "worker_revert can undo it");
    }

    #[tokio::test]
    async fn a_stopped_continuation_turn_does_not_start_another_continuation() {
        let (mut engine, driver, cancel) = generation_engine(
            |sessions| {
                let mut record = test_session(sessions);
                record.workers = vec![worker_record(
                    "w-done",
                    crate::worker::WorkerStatus::Settled,
                )];
                sessions.save(&record).unwrap();
                record
            },
            vec![AgentTurnResult::Answer {
                text: "reviewed".to_string(),
            }],
        );
        // The user presses Stop on the engine-started continuation turn.
        driver.before_next_foreground(move || {
            cancel.send_replace(1);
        });
        engine
            .chat(plain_request(crate::worker::WORKER_CONTINUE_TEXT))
            .await
            .unwrap();
        assert!(engine.last_turn_cancelled);
        let record = engine.session_store.load(&engine.session_id).unwrap();
        assert!(
            !record.workers[0].reported,
            "the stopped turn reported nothing"
        );
        // What `worker_continue` and every command exit decide from it.
        assert_eq!(
            crate::worker::continuation_after_command(&record, engine.last_turn_cancelled),
            None,
            "a Stop never starts the next continuation"
        );
    }

    #[tokio::test]
    async fn stopping_a_user_chat_with_an_unreported_settlement_does_not_start_a_continuation() {
        let (mut engine, driver, cancel) = generation_engine(
            |sessions| {
                let mut record = test_session(sessions);
                record.workers = vec![worker_record(
                    "w-live",
                    crate::worker::WorkerStatus::Running,
                )];
                sessions.save(&record).unwrap();
                record
            },
            vec![AgentTurnResult::Answer {
                text: "done".to_string(),
            }],
        );
        let sessions = engine.session_store.clone();
        let session_id = engine.session_id.clone();
        // The worker settles while the user's turn runs, then the user stops it.
        driver.before_next_foreground(move || {
            sessions
                .update_worker(&session_id, "w-live", |worker| {
                    crate::worker::settle(worker, Ok(Default::default()));
                    true
                })
                .unwrap();
            cancel.send_replace(1);
        });
        engine.chat(plain_request("user work")).await.unwrap();
        assert!(engine.last_turn_cancelled);
        let record = engine.session_store.load(&engine.session_id).unwrap();
        assert_eq!(
            crate::worker::next_continuation(&record).as_deref(),
            Some("w-live")
        );
        assert_eq!(
            crate::worker::continuation_after_command(&record, engine.last_turn_cancelled),
            None,
            "the settlement waits for the next command the user starts"
        );
        // The next user turn that completes shows it and reports it.
        engine.chat(plain_request("next")).await.unwrap();
        assert!(!engine.last_turn_cancelled);
        let record = engine.session_store.load(&engine.session_id).unwrap();
        assert!(record.workers[0].reported);
    }

    /// What a command exit sends to the dispatcher for this engine's session,
    /// through the same function the session manager calls.
    fn dispatched_continuation(
        engine: &AgentEngine<FakeCodexDriver, CapturingEventSink>,
    ) -> Option<String> {
        let (dispatcher, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let sent = worker::dispatch_continuation(
            &dispatcher,
            &engine.session_store,
            &engine.session_id,
            engine.last_turn_cancelled,
        );
        match commands.try_recv() {
            Ok(team::TeamCommand::Worker(worker::WorkerCommand::Continue {
                parent_id,
                worker_id,
            })) => {
                assert_eq!(parent_id, engine.session_id);
                assert_eq!(Some(&worker_id), sent.as_ref());
                Some(worker_id)
            }
            Ok(_) => panic!("only a continuation is dispatched"),
            Err(_) => {
                assert!(sent.is_none());
                None
            }
        }
    }

    /// An engine with an unreported settled worker, a plan awaiting approval,
    /// and a driver that honors cancellation.
    async fn engine_with_plan_under_review(
        turns: Vec<AgentTurnResult>,
    ) -> (
        AgentEngine<FakeCodexDriver, CapturingEventSink>,
        FakeCodexDriver,
        tokio::sync::watch::Sender<u64>,
    ) {
        let mut all = vec![AgentTurnResult::Plan {
            markdown: "- change it".to_string(),
        }];
        all.extend(turns);
        let (mut engine, driver, cancel) = generation_engine(test_session, all);
        engine.executor.plan_runtime = Some(engine.runtime.clone());
        let dirs = engine.runtime.data_dirs();
        dirs.ensure_dirs().unwrap();
        let workspace = WorkspaceManager::new(dirs.clone())
            .prepare_snapshot(&bind_workspace_snapshot(&engine.runtime, "ExampleProject"))
            .unwrap();
        driver.set_workspace(workspace);
        engine.chat(plain_request("plan it")).await.unwrap();
        assert!(
            engine.current_plan_markdown.is_some(),
            "a plan is under review"
        );
        // A worker settled while the plan was being written; the plan turn's
        // note did not show it.
        let mut record = engine.session_store.load(&engine.session_id).unwrap();
        record.workers = vec![worker_record(
            "w-done",
            crate::worker::WorkerStatus::Settled,
        )];
        engine.session_store.save(&record).unwrap();
        (engine, driver, cancel)
    }

    #[tokio::test]
    async fn a_stopped_plan_feedback_turn_dispatches_no_continuation() {
        let (mut engine, driver, cancel) =
            engine_with_plan_under_review(vec![AgentTurnResult::Plan {
                markdown: "- revised".to_string(),
            }])
            .await;
        assert_eq!(
            dispatched_continuation(&engine).as_deref(),
            Some("w-done"),
            "the settlement is owed a continuation before the Stop"
        );
        driver.before_next_foreground(move || {
            cancel.send_replace(1);
        });
        engine
            .plan_feedback(crate::ipc::PlanFeedbackRequest {
                client_turn_id: crate::ipc::new_client_turn_id(),
                text: "smaller".to_string(),
                attachments: Vec::new(),
                mentions: Vec::new(),
            })
            .await
            .unwrap();
        assert!(
            engine.last_turn_cancelled,
            "plan feedback records its own cancel"
        );
        assert_eq!(dispatched_continuation(&engine), None);
    }

    #[tokio::test]
    async fn a_stopped_approved_plan_execution_dispatches_no_continuation() {
        let (mut engine, driver, cancel) =
            engine_with_plan_under_review(vec![AgentTurnResult::Answer {
                text: "implemented".to_string(),
            }])
            .await;
        engine.plan_approve().await.unwrap();
        driver.before_next_foreground(move || {
            cancel.send_replace(1);
        });
        let _ = engine.continue_pending_write().await;
        assert!(
            engine.last_turn_cancelled,
            "the execution turn records its own cancel"
        );
        assert_eq!(dispatched_continuation(&engine), None);
    }

    #[tokio::test]
    async fn a_turn_that_succeeds_after_a_cancelled_one_dispatches_the_continuation() {
        let (mut engine, _driver, _cancel) =
            engine_with_plan_under_review(vec![AgentTurnResult::Answer {
                text: "implemented".to_string(),
            }])
            .await;
        // An earlier turn of this session was cancelled.
        engine.last_turn_cancelled = true;
        assert_eq!(dispatched_continuation(&engine), None);
        // The approved plan's execution turn runs to its answer: the flag is
        // its own, so the owed continuation is dispatched.
        engine.plan_approve().await.unwrap();
        engine.continue_pending_write().await.unwrap();
        assert!(!engine.last_turn_cancelled);
        assert_eq!(dispatched_continuation(&engine).as_deref(), Some("w-done"));
    }

    #[tokio::test]
    async fn a_worker_session_keeps_the_goal_as_its_user_message_and_the_rules_as_context() {
        let (mut engine, driver, _cancel) = generation_engine(
            |sessions| {
                let parent = test_session(sessions);
                let child = crate::worker::child_session(&parent, "메뉴 정리");
                sessions.save(&child).unwrap();
                child
            },
            vec![
                AgentTurnResult::Answer {
                    text: "done".to_string(),
                },
                AgentTurnResult::Answer {
                    text: "adjusted".to_string(),
                },
            ],
        );
        let goal = "src/menu.eps의 메뉴를 정리";
        engine.chat(plain_request(goal)).await.unwrap();
        // What the turn records as the user's message is the goal alone.
        assert_eq!(engine.current_user_text, goal);
        let prompt = driver.prompts().pop().unwrap();
        assert!(prompt.contains(goal), "{prompt}");
        assert!(prompt.contains(crate::worker::WORKER_PROMPT), "{prompt}");

        // A later message the user types into the worker's pane keeps the rules.
        engine
            .chat(plain_request("이름만 바꿔 주세요"))
            .await
            .unwrap();
        assert_eq!(engine.current_user_text, "이름만 바꿔 주세요");
        let prompt = driver.prompts().pop().unwrap();
        assert!(prompt.contains(crate::worker::WORKER_PROMPT), "{prompt}");
        assert!(prompt.contains("이름만 바꿔 주세요"), "{prompt}");
    }

    #[tokio::test]
    async fn an_ordinary_session_gets_no_worker_section() {
        let (mut engine, driver, _cancel) = generation_engine(
            test_session,
            vec![AgentTurnResult::Answer {
                text: "done".to_string(),
            }],
        );
        engine.chat(plain_request("work")).await.unwrap();
        assert!(!driver
            .prompts()
            .pop()
            .unwrap()
            .contains("[background worker]"));
    }

    #[tokio::test]
    async fn the_interview_toggle_asks_for_an_interview_on_that_message_only() {
        let (mut engine, driver, _cancel) = generation_engine(
            test_session,
            vec![
                AgentTurnResult::Answer {
                    text: "interviewed".to_string(),
                },
                AgentTurnResult::Answer {
                    text: "done".to_string(),
                },
            ],
        );
        engine
            .chat(crate::ipc::ChatRequest {
                clarify: true,
                ..plain_request("디펜스 맵 만들어줘")
            })
            .await
            .unwrap();
        engine.chat(plain_request("계속")).await.unwrap();

        let prompts = driver.prompts();
        // The interview rules are standing context; the request is per message.
        assert!(prompts[0].contains("[clarify]\n"), "{}", prompts[0]);
        assert!(
            prompts[0].contains(CLARIFY_REQUESTED_NOTE),
            "{}",
            prompts[0]
        );
        assert!(
            !prompts[1].contains(CLARIFY_REQUESTED_NOTE),
            "{}",
            prompts[1]
        );
    }

    #[tokio::test]
    async fn a_turn_that_replays_the_transcript_keeps_the_interview_request() {
        let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
            text: "interviewed".to_string(),
        }]);
        let handle = driver.clone();
        let mut engine = test_engine(driver, CapturingEventSink::default());
        // No live provider thread: the turn starts fresh from the transcript.
        engine.thread_active = false;
        engine.pending_resume_transcript = Some("prior transcript".to_string());

        engine
            .chat(crate::ipc::ChatRequest {
                clarify: true,
                ..plain_request("디펜스 맵 만들어줘")
            })
            .await
            .unwrap();

        let prompt = handle.prompts().pop().unwrap();
        assert!(prompt.contains("prior transcript"), "{prompt}");
        assert!(prompt.contains(CLARIFY_REQUESTED_NOTE), "{prompt}");
    }

    #[tokio::test]
    async fn an_autonomous_run_ignores_the_interview_toggle() {
        let (mut engine, driver, _cancel) = generation_engine(
            test_session,
            vec![AgentTurnResult::Answer {
                text: "done".to_string(),
            }],
        );
        engine
            .chat(crate::ipc::ChatRequest {
                clarify: true,
                execution_mode: crate::autonomous::ExecutionMode::Autonomous,
                ..plain_request("대규모 트리거 작업")
            })
            .await
            .unwrap();
        let prompt = driver.prompts().remove(0);
        assert!(prompt.contains("[autonomous execution]"), "{prompt}");
        assert!(!prompt.contains(CLARIFY_REQUESTED_NOTE), "{prompt}");
    }

    #[tokio::test]
    async fn a_worker_session_ignores_the_interview_toggle() {
        let (mut engine, driver, _cancel) = generation_engine(
            |sessions| {
                let parent = test_session(sessions);
                let child = crate::worker::child_session(&parent, "메뉴 정리");
                sessions.save(&child).unwrap();
                child
            },
            vec![AgentTurnResult::Answer {
                text: "done".to_string(),
            }],
        );
        engine
            .chat(crate::ipc::ChatRequest {
                clarify: true,
                ..plain_request("src/menu.eps의 메뉴를 정리")
            })
            .await
            .unwrap();
        let prompt = driver.prompts().pop().unwrap();
        assert!(prompt.contains(crate::worker::WORKER_PROMPT), "{prompt}");
        assert!(!prompt.contains(CLARIFY_REQUESTED_NOTE), "{prompt}");
    }
}
