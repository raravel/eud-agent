//! Staged workflow controller: triage → clarify → research → plan (+critic)
//! → plan review → execute → verify → changeset review.
//!
//! Every stage job is an isolated delegated run over the session's read tools;
//! the engine owns the transitions, persists `WorkflowState` on the session at
//! every transition, and emits the panel projection. The ordinary foreground
//! turn remains the only executing context.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{AgentEngine, AgentEngineError, AgentTurnResult, EngineEvent, EventSink, Phase};
use crate::{
    provider_runtime::{
        AgentTurnInput, DelegatedRunKind, DelegatedRunOutcome, DelegatedRunRequest,
        RuntimeExecutor, WorkspaceAccess,
    },
    provider_tool_loop::DelegatedToolProfile,
    tool_exec::EngineAskOutcome,
    workflow::{
        self, ArtifactRef, PlanArtifact, PlanReviewRecord, StageContext, TriageResult,
        VerifyResult, WorkflowRoute, WorkflowStage, WorkflowState, MAX_CLARIFY_ROUNDS,
        MAX_VERIFY_ATTEMPTS,
    },
    workspace::{PreparedWorkspace, WorkspaceManager},
};

/// How one stage job ended, from the engine's point of view.
pub(super) enum StageOutcome {
    Value(Value),
    Cancelled,
}

/// What the caller of triage should do next.
pub(super) enum TriageDecision {
    /// Continue with the ordinary foreground turn on this route.
    Foreground(WorkflowRoute),
    /// The pipeline ran to plan review (or failed/cancelled and persisted that).
    Handled,
}

const CHANGESET_SUMMARY_BYTES: usize = 96 * 1024;
const EVIDENCE_BYTES: usize = 32 * 1024;
const INLINE_PLAN_BYTES: usize = 48 * 1024;
const INLINE_RESEARCH_BYTES: usize = 32 * 1024;

fn now() -> u64 {
    crate::session::now_unix_millis()
}

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut cut = limit;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n…(truncated)", &text[..cut])
}

impl<R: RuntimeExecutor, S: EventSink> AgentEngine<R, S> {
    // ---- state persistence -------------------------------------------------

    fn persist_workflow(&mut self, mut state: WorkflowState) -> Result<(), AgentEngineError> {
        state.updated_at = now();
        self.session_store
            .set_workflow(&self.session_id, Some(state.clone()))
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let event = workflow::WorkflowEvent::from_state(&state);
        self.workflow = Some(state);
        self.sink.emit(EngineEvent::Workflow(Box::new(event)))
    }

    fn workflow_transition(
        &mut self,
        stage: WorkflowStage,
        update: impl FnOnce(&mut WorkflowState),
    ) -> Result<(), AgentEngineError> {
        let Some(mut state) = self.workflow.clone() else {
            return Ok(());
        };
        state.stage = stage;
        update(&mut state);
        self.persist_workflow(state)
    }

    /// The live workflow of the current request, if any. A workflow left by an
    /// earlier request (interrupted, failed, cancelled, or superseded) never
    /// drives the current one.
    fn current_workflow(&self) -> Option<&WorkflowState> {
        let state = self.workflow.as_ref()?;
        (self.current_request_id.as_deref() == Some(state.request_id.as_str())).then_some(state)
    }

    fn workflow_fail(&mut self, message: &str) -> Result<(), AgentEngineError> {
        let message = message.to_string();
        self.workflow_transition(WorkflowStage::Failed, move |state| {
            state.error = Some(message)
        })
    }

    fn workflow_cancel(&mut self) -> Result<(), AgentEngineError> {
        self.workflow_transition(WorkflowStage::Cancelled, |_| {})
    }

    /// Cancel a non-terminal workflow, whether it belongs to the current
    /// request or was left behind by an earlier one.
    pub(super) fn workflow_cancel_if_active(&mut self) -> Result<(), AgentEngineError> {
        if self
            .workflow
            .as_ref()
            .is_some_and(|state| !state.stage.is_terminal())
        {
            self.workflow_cancel()?;
        }
        Ok(())
    }

    /// Forget a handed-off clarify ask when a non-staged request (autonomous,
    /// Map) takes the session: the next interactive message is then an
    /// ordinary new request, not the reply to those questions.
    pub(super) fn workflow_drop_pending_clarification(&mut self) -> Result<(), AgentEngineError> {
        let Some(stage) = self
            .workflow
            .as_ref()
            .filter(|state| state.pending_clarification.is_some())
            .map(|state| state.stage)
        else {
            return Ok(());
        };
        self.workflow_transition(stage, |state| state.pending_clarification = None)
    }

    pub(super) fn workflow_fail_if_active(
        &mut self,
        message: &str,
    ) -> Result<(), AgentEngineError> {
        if self.current_workflow().is_some_and(|state| {
            !state.stage.is_terminal() && state.stage != WorkflowStage::Interrupted
        }) {
            self.workflow_fail(message)?;
        }
        Ok(())
    }

    /// Called when an ordinary request (answer/direct route) settles so the
    /// strip reflects the end state. Pipeline requests settle through review.
    pub(super) fn workflow_settle_foreground(&mut self) -> Result<(), AgentEngineError> {
        let Some(state) = self.current_workflow() else {
            return Ok(());
        };
        if state.stage.is_terminal() {
            return Ok(());
        }
        let stage = match self.phase {
            Phase::ChangesetReview => WorkflowStage::ChangesetReview,
            Phase::PlanReview => WorkflowStage::PlanReview,
            Phase::Executing => WorkflowStage::Executing,
            Phase::Idle | Phase::Triage | Phase::Answer => WorkflowStage::Done,
        };
        self.workflow_transition(stage, |_| {})
    }

    pub(super) fn workflow_mark_done(&mut self) -> Result<(), AgentEngineError> {
        if self.current_workflow().is_some() {
            self.workflow_transition(WorkflowStage::Done, |_| {})?;
        }
        Ok(())
    }

    pub(super) fn workflow_is_pipeline(&self) -> bool {
        self.current_workflow()
            .is_some_and(|state| state.route == Some(WorkflowRoute::Pipeline))
    }

    /// The rendered verdict of the current request, for the harness job.
    pub(super) fn workflow_verdict_markdown(&self) -> Option<String> {
        let state = self.current_workflow()?;
        let verdict = state.verdict.as_ref()?;
        let workspace = self.executor.current_workspace()?;
        self.read_artifact(&workspace, &verdict.path).ok()
    }

    // ---- stage execution ---------------------------------------------------

    fn deep_planning_setting(&self) -> bool {
        self.runtime
            .data_dirs()
            .load_config()
            .map(|config| config.deep_planning)
            .unwrap_or(false)
    }

    /// The session workspace: the CLI cwd, temp dir, and artifact root. The
    /// foreground prepares it on its first turn; stage jobs and approval may
    /// run before any foreground turn, so they prepare it themselves.
    pub(super) async fn prepared_workspace(&self) -> Result<PreparedWorkspace, AgentEngineError> {
        if let Some(workspace) = self.executor.current_workspace() {
            return Ok(workspace);
        }
        let manager = WorkspaceManager::new(self.runtime.data_dirs());
        let session_id = self.session_id.clone();
        tokio::task::spawn_blocking(move || manager.prepare_session_current(&session_id))
            .await
            .map_err(|error| AgentEngineError::new(error.to_string()))?
            .map_err(AgentEngineError::new)
    }

    /// The static authoring guidance the read-only stages share. Build
    /// guidance describes a tool only the verifier has; Map/audio/mention
    /// guides are foreground-only.
    fn stage_guides() -> String {
        [
            super::first_principles_section(),
            super::EPS_IDIOMS.to_string(),
            super::EPSCRIPT_GUIDE.to_string(),
            super::EPS_PROJECT_ARCHITECTURE_GUIDE.to_string(),
        ]
        .join(
            "

",
        )
    }

    fn verifier_guides() -> String {
        [Self::stage_guides(), super::BUILD_GUIDE.to_string()].join(
            "

",
        )
    }

    fn stage_project_context(&self, query: &str) -> String {
        let mut parts = vec![super::project_state_section(
            &self.config.project_state_for_prompt(),
        )];
        if let Some(map) = self.config.project_map_for_prompt() {
            parts.push(map);
        }
        if let Some(memory) = self
            .config
            .project_memory_for_prompt()
            .and_then(|memory| super::project_memory_section(Some(&memory)))
        {
            parts.push(memory);
        }
        if let Some(wiki) = self
            .config
            .wiki_section_for_prompt(query)
            .and_then(|wiki| super::wiki_facts_section(Some(&wiki)))
        {
            parts.push(wiki);
        }
        parts.join("\n\n")
    }

    async fn run_stage(
        &mut self,
        kind: DelegatedRunKind,
        prompt: String,
    ) -> Result<StageOutcome, AgentEngineError> {
        let request_id = self
            .current_request_id
            .clone()
            .ok_or_else(|| AgentEngineError::new("단계 작업에 연결된 요청이 없습니다."))?;
        let workspace = self.prepared_workspace().await?;
        let (workspace_root, workspace_temp) = (workspace.root, Some(workspace.temp_dir));
        let schema = workflow::stage_schema(kind);
        let profile =
            DelegatedToolProfile::new(workflow::stage_tools(kind).iter().copied(), &schema)
                .map_err(AgentEngineError::new)?;
        let request = DelegatedRunRequest {
            identity: self.run_identity(&request_id),
            parent_run_id: None,
            binding: self.binding_snapshot(true)?,
            kind,
            prompt,
            workspace_root,
            workspace_temp,
            output_schema: schema,
            profile,
            allow_live_write_ticket: kind == DelegatedRunKind::Verifier,
            policy: workflow::stage_policy(kind),
        };
        match self.executor.run_delegated(request).await {
            DelegatedRunOutcome::Result { value, .. } => Ok(StageOutcome::Value(value)),
            DelegatedRunOutcome::Cancelled => Ok(StageOutcome::Cancelled),
            DelegatedRunOutcome::Failed(error) => Err(AgentEngineError::new(format!(
                "{} 단계를 완료하지 못했습니다: {error}",
                stage_label(kind)
            ))),
        }
    }

    /// Run one stage, mapping cancellation and failure onto the workflow
    /// state. `Ok(None)` means the request ended (cancelled or failed).
    async fn stage_value(
        &mut self,
        stage: WorkflowStage,
        kind: DelegatedRunKind,
        prompt: String,
    ) -> Result<Option<Value>, AgentEngineError> {
        self.workflow_transition(stage, |_| {})?;
        match self.run_stage(kind, prompt).await {
            Ok(StageOutcome::Value(value)) => Ok(Some(value)),
            Ok(StageOutcome::Cancelled) => {
                self.workflow_cancel()?;
                self.phase = Phase::Idle;
                Ok(None)
            }
            Err(error) => {
                self.workflow_fail(&error.message)?;
                self.phase = Phase::Idle;
                Err(error)
            }
        }
    }

    fn write_artifact(
        &self,
        workspace: &PreparedWorkspace,
        relative: &str,
        contents: &str,
    ) -> Result<ArtifactRef, AgentEngineError> {
        WorkspaceManager::new(self.runtime.data_dirs())
            .write_stage_artifact(&workspace.id, relative, contents)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        Ok(ArtifactRef {
            path: relative.to_string(),
            sha256: crate::task_state::sha256_bytes(contents.as_bytes()),
            summary: String::new(),
        })
    }

    fn read_artifact(
        &self,
        workspace: &PreparedWorkspace,
        relative: &str,
    ) -> Result<String, AgentEngineError> {
        WorkspaceManager::new(self.runtime.data_dirs())
            .read_stage_artifact(&workspace.id, relative)
            .map_err(|error| AgentEngineError::new(error.to_string()))
    }

    fn stage_context<'a>(
        &self,
        guides: &'a str,
        project: &'a str,
        state: &'a WorkflowState,
    ) -> StageContext<'a> {
        StageContext {
            guides,
            project,
            user_text: &state.user_text,
            clarifications: &state.clarifications,
        }
    }

    fn require_triage(&self) -> Result<TriageResult, AgentEngineError> {
        self.workflow
            .as_ref()
            .and_then(|state| state.triage.clone())
            .ok_or_else(|| {
                AgentEngineError::new("요청 파악 결과가 없어 단계를 진행할 수 없습니다.")
            })
    }

    // ---- triage + clarify --------------------------------------------------

    /// Triage a new request. Returns the route to continue on the ordinary
    /// foreground, or `Handled` when the pipeline (or a cancellation/failure)
    /// consumed the request.
    pub(super) async fn workflow_start(
        &mut self,
        request_id: &str,
        client_turn_id: &str,
        user_text: &str,
    ) -> Result<TriageDecision, AgentEngineError> {
        // Chat requires an open project; the revision is only resume
        // validation, so an unavailable one degrades to an empty token.
        let revision = self.runtime.current_project_revision().unwrap_or_default();
        let mut state = WorkflowState::new(
            request_id.to_string(),
            client_turn_id.to_string(),
            revision,
            user_text.to_string(),
            self.deep_planning_setting(),
            now(),
        );
        // The previous request ended by restating an unanswered clarify ask as
        // text: this message is its reply, so triage continues that request
        // (original text, earlier answers, spent rounds) instead of a fresh one.
        let mut rounds = self
            .workflow
            .take()
            .filter(|previous| previous.pending_clarification.is_some())
            .map_or(0_u8, |previous| {
                state.continue_clarification(&previous, user_text)
            });
        self.persist_workflow(state)?;
        let guides = String::new();
        loop {
            let project = self.stage_project_context(user_text);
            let prompt = {
                let state = self.workflow.as_ref().expect("workflow just persisted");
                workflow::triage_prompt(
                    &self.stage_context(&guides, &project, state),
                    MAX_CLARIFY_ROUNDS.saturating_sub(rounds),
                )
            };
            let Some(value) = self
                .stage_value(WorkflowStage::Triage, DelegatedRunKind::Triage, prompt)
                .await?
            else {
                return Ok(TriageDecision::Handled);
            };
            let (mut triage, questions) = match workflow::parse_triage(&value) {
                Ok(parsed) => parsed,
                Err(error) => {
                    self.workflow_fail(&error)?;
                    self.phase = Phase::Idle;
                    return Err(AgentEngineError::new(error));
                }
            };
            triage.clarify_rounds = rounds;
            if questions.is_empty() || rounds >= MAX_CLARIFY_ROUNDS {
                if !questions.is_empty() {
                    // Out of clarify rounds: answer with what is still missing.
                    triage.route = WorkflowRoute::Answer;
                    triage.rationale = format!(
                        "미해결 질문: {}",
                        questions
                            .iter()
                            .map(|question| question.question.as_str())
                            .collect::<Vec<_>>()
                            .join(" / ")
                    );
                }
                let route = triage.route;
                self.workflow_transition(WorkflowStage::Triage, |state| {
                    state.triage = Some(triage);
                    state.route = Some(route);
                })?;
                return match route {
                    WorkflowRoute::Pipeline => {
                        self.workflow_pipeline().await?;
                        Ok(TriageDecision::Handled)
                    }
                    WorkflowRoute::Direct => {
                        self.workflow_transition(WorkflowStage::Executing, |_| {})?;
                        Ok(TriageDecision::Foreground(WorkflowRoute::Direct))
                    }
                    WorkflowRoute::Answer => Ok(TriageDecision::Foreground(WorkflowRoute::Answer)),
                };
            }
            self.workflow_transition(WorkflowStage::Clarify, |_| {})?;
            let answers = match self
                .runtime
                .ask_for_request(request_id, questions.clone())
                .await
            {
                Ok(EngineAskOutcome::Answered(answers)) => answers,
                Ok(EngineAskOutcome::Unanswered { waited_seconds }) => {
                    // The bounded wait elapsed. As with the `ask` tool, the
                    // turn ends with the questions as its text answer and the
                    // user's next message is the reply; nothing failed.
                    let pending = workflow::PendingClarification {
                        questions: questions.clone(),
                        rounds,
                    };
                    self.workflow_transition(WorkflowStage::Done, |state| {
                        state.pending_clarification = Some(pending);
                    })?;
                    self.handle_turn_result(AgentTurnResult::Answer {
                        text: workflow::clarification_handoff_text(&questions, waited_seconds),
                    })?;
                    return Ok(TriageDecision::Handled);
                }
                Err(error) if error.contains("cancelled") => {
                    self.workflow_cancel()?;
                    self.phase = Phase::Idle;
                    return Ok(TriageDecision::Handled);
                }
                Err(error) => {
                    self.workflow_fail(&error)?;
                    self.phase = Phase::Idle;
                    return Err(AgentEngineError::new(error));
                }
            };
            let clarification = render_clarification(&questions, &answers);
            rounds = rounds.saturating_add(1);
            self.workflow_transition(WorkflowStage::Triage, |state| {
                state.clarifications.push(clarification);
            })?;
        }
    }

    /// The `[route]` note for an answer/direct foreground turn: the decision
    /// plus the triage goal and rationale (which names what is still missing
    /// when clarification rounds ran out).
    pub(super) fn workflow_route_note(&self, route: WorkflowRoute) -> String {
        let (goal, rationale) = self
            .current_workflow()
            .and_then(|state| state.triage.as_ref())
            .map(|triage| (triage.goal.clone(), triage.rationale.clone()))
            .unwrap_or_default();
        let decision = match route {
            WorkflowRoute::Answer => {
                "Triage classified this request as answer-only: reply directly and use no write tools."
            }
            _ => {
                "Triage classified this request as one direct single-site change: apply it, run the authoritative build, and answer."
            }
        };
        format!("[route]\n{decision}\ngoal: {goal}\nrationale: {rationale}")
    }

    /// The clarification answers, for the ordinary foreground routes. When the
    /// request continued a handed-off clarify ask, `user_text` is only the
    /// reply, so the original request is restated first: no provider turn ran
    /// for it.
    pub(super) fn workflow_clarification_text(&self, user_text: &str) -> Option<String> {
        let state = self.current_workflow()?;
        if state.clarifications.is_empty() {
            return None;
        }
        let mut out = String::from("[clarification]\n");
        if state.user_text != user_text {
            out.push_str(&format!("original request: {}\n", state.user_text));
        }
        for (index, answer) in state.clarifications.iter().enumerate() {
            out.push_str(&format!("round {}: {}\n", index + 1, answer));
        }
        Some(out)
    }

    // ---- research + plan ---------------------------------------------------

    async fn workflow_pipeline(&mut self) -> Result<(), AgentEngineError> {
        let triage = self.require_triage()?;
        let guides = Self::stage_guides();
        let prompt = {
            let state = self
                .workflow
                .as_ref()
                .expect("pipeline needs workflow state");
            let project = self.stage_project_context(&state.user_text);
            workflow::research_prompt(&self.stage_context(&guides, &project, state), &triage)
        };
        let Some(value) = self
            .stage_value(WorkflowStage::Research, DelegatedRunKind::Research, prompt)
            .await?
        else {
            return Ok(());
        };
        let request_id = self
            .workflow
            .as_ref()
            .map(|state| state.request_id.clone())
            .unwrap_or_default();
        let workspace = self.prepared_workspace().await?;
        let markdown = workflow::render_research(&request_id, &value);
        let mut artifact =
            self.write_artifact(&workspace, &format!("research/{request_id}.md"), &markdown)?;
        artifact.summary = value
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.workflow_transition(WorkflowStage::Research, |state| {
            state.research = Some(artifact);
        })?;
        self.workflow_plan_round(None).await
    }

    /// One planning round: planner (+ architect/critic per depth) until the
    /// plan is ready for the user. `feedback` re-enters from plan review.
    pub(super) async fn workflow_plan_round(
        &mut self,
        feedback: Option<&str>,
    ) -> Result<(), AgentEngineError> {
        let triage = self.require_triage()?;
        let guides = Self::stage_guides();
        let workspace = self.prepared_workspace().await?;
        let (request_id, deep, research_path, previous_plan) = {
            let state = self
                .workflow
                .as_ref()
                .expect("plan round needs workflow state");
            (
                state.request_id.clone(),
                state.deep_planning,
                state
                    .research
                    .as_ref()
                    .map(|artifact| artifact.path.clone()),
                state.plan.as_ref().map(|plan| plan.path.clone()),
            )
        };
        // A missing artifact degrades the prompt rather than discarding the
        // request; the plan revision still carries the user's feedback.
        let research_markdown = match research_path {
            Some(path) => self
                .read_artifact(&workspace, &path)
                .unwrap_or_else(|error| format!("(research artifact unavailable: {error})")),
            None => "(no research artifact)".to_string(),
        };
        let mut previous_markdown = match previous_plan {
            Some(path) => self.read_artifact(&workspace, &path).ok(),
            None => None,
        };
        let mut critique: Option<String> = None;
        let mut iterations = 0_u8;
        let mut critic_runs = 0_u8;
        let mut reviews: Vec<PlanReviewRecord> = Vec::new();
        let plan_relative = format!("plans/{request_id}.md");
        let review_after_feedback = feedback.is_none() || deep;
        // The project is read-only throughout planning: one context serves
        // every planner and reviewer call of this round.
        let project = {
            let state = self.workflow.as_ref().expect("workflow state");
            self.stage_project_context(&state.user_text)
        };
        loop {
            iterations = iterations.saturating_add(1);
            let prompt = {
                let state = self.workflow.as_ref().expect("workflow state");
                workflow::planner_prompt(
                    &self.stage_context(&guides, &project, state),
                    &triage,
                    &research_markdown,
                    previous_markdown.as_deref(),
                    (iterations == 1).then_some(feedback).flatten(),
                    critique.as_deref(),
                )
            };
            let Some(plan_value) = self
                .stage_value(WorkflowStage::Planning, DelegatedRunKind::Planner, prompt)
                .await?
            else {
                return Ok(());
            };
            let revision = self
                .plan_revision
                .checked_add(1)
                .ok_or_else(|| AgentEngineError::new("plan revision overflow"))?;
            let markdown = workflow::render_plan(&request_id, revision, &plan_value);
            let artifact = self.write_artifact(&workspace, &plan_relative, &markdown)?;
            self.plan_revision = revision;
            self.current_plan_markdown = Some(markdown.clone());
            let plan = PlanArtifact {
                path: artifact.path,
                revision,
                sha256: artifact.sha256,
                approved_sha256: None,
                title: plan_value
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                acceptance_criteria: plan_value
                    .get("acceptanceCriteria")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
                // A revision that is not re-reviewed shows no verdict; the
                // review history below keeps every earlier verdict.
                critic_verdict: None,
                critic_summary: None,
                deep,
                iterations,
                reviews: reviews.clone(),
            };
            self.workflow_transition(WorkflowStage::Planning, |state| {
                state.plan = Some(plan);
                state.critique_rounds = critic_runs;
            })?;
            previous_markdown = Some(markdown.clone());

            // Default depth reviews once and accepts the revised plan as final;
            // a feedback revision without deep planning is not re-reviewed.
            let review_now = review_after_feedback && (deep || iterations == 1);
            if !review_now {
                break;
            }
            let architect = if deep {
                let prompt = {
                    let state = self.workflow.as_ref().expect("workflow state");
                    workflow::architect_prompt(
                        &self.stage_context(&guides, &project, state),
                        &triage,
                        &research_markdown,
                        &markdown,
                    )
                };
                match self
                    .stage_value(WorkflowStage::Critique, DelegatedRunKind::Architect, prompt)
                    .await?
                {
                    Some(value) => Some(value),
                    None => return Ok(()),
                }
            } else {
                None
            };
            let critic_prompt = {
                let state = self.workflow.as_ref().expect("workflow state");
                workflow::critic_prompt(
                    &self.stage_context(&guides, &project, state),
                    &triage,
                    &research_markdown,
                    &markdown,
                )
            };
            let Some(critic) = self
                .stage_value(
                    WorkflowStage::Critique,
                    DelegatedRunKind::Critic,
                    critic_prompt,
                )
                .await?
            else {
                return Ok(());
            };
            let revise = critic.get("verdict").and_then(Value::as_str) == Some("revise")
                || architect
                    .as_ref()
                    .and_then(|value| value.get("verdict"))
                    .and_then(Value::as_str)
                    == Some("revise");
            critic_runs = critic_runs.saturating_add(1);
            let architect_verdict = architect.as_ref().map(|value| {
                value
                    .get("verdict")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            });
            let verdict = critic
                .get("verdict")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let summary = critic
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let rendered = workflow::render_critique(iterations, architect.as_ref(), &critic);
            let review_artifact = self.write_artifact(
                &workspace,
                &format!("verify/{request_id}.plan.{iterations}.md"),
                &rendered,
            )?;
            reviews.push(PlanReviewRecord {
                iteration: iterations,
                revision,
                architect_verdict,
                critic_verdict: verdict.clone(),
                summary: summary.clone(),
                path: review_artifact.path,
            });
            let reviews_snapshot = reviews.clone();
            self.workflow_transition(WorkflowStage::Critique, |state| {
                if let Some(plan) = state.plan.as_mut() {
                    plan.critic_verdict = Some(verdict);
                    plan.critic_summary = Some(summary);
                    plan.reviews = reviews_snapshot;
                }
                state.critique_rounds = critic_runs;
            })?;
            let max_rounds = self
                .workflow
                .as_ref()
                .map(WorkflowState::max_critique_rounds)
                .unwrap_or(1);
            if !revise || (deep && iterations >= max_rounds) {
                break;
            }
            critique = Some(rendered);
        }
        // Plan review: the same plan card and approval path as before.
        let markdown = self
            .current_plan_markdown
            .clone()
            .ok_or_else(|| AgentEngineError::new("계획 단계가 계획을 만들지 못했습니다."))?;
        self.phase = Phase::PlanReview;
        self.sink.emit(EngineEvent::Plan(crate::ipc::PlanEvent {
            markdown,
            revision: self.plan_revision,
        }))?;
        self.workflow_transition(WorkflowStage::PlanReview, |_| {})
    }

    pub(super) fn workflow_record_approval(
        &mut self,
        sha256: &str,
    ) -> Result<(), AgentEngineError> {
        if self.current_workflow().is_none() {
            return Ok(());
        }
        let sha256 = sha256.to_string();
        self.workflow_transition(WorkflowStage::Executing, move |state| {
            if let Some(plan) = state.plan.as_mut() {
                plan.approved_sha256 = Some(sha256);
            }
        })
    }

    /// The execution instruction for an approved staged plan. The plan and
    /// research were produced in isolated contexts the executing thread has
    /// never seen, and direct providers cannot read workspace files, so both
    /// are inlined (bounded) in addition to their paths.
    pub(super) fn workflow_execution_instruction(
        &self,
        request_id: &str,
        workspace: &PreparedWorkspace,
    ) -> Option<String> {
        let state = self.current_workflow()?;
        if state.route != Some(WorkflowRoute::Pipeline) {
            return None;
        }
        let plan_path = state.plan.as_ref().map(|plan| plan.path.clone())?;
        let plan_markdown = self
            .current_plan_markdown
            .as_deref()
            .map(|markdown| truncate(markdown, INLINE_PLAN_BYTES))
            .unwrap_or_else(|| "(plan text unavailable)".to_string());
        let research = state
            .research
            .as_ref()
            .map(|artifact| format!(" and the research notes at `{}`", artifact.path))
            .unwrap_or_default();
        let research_markdown = state
            .research
            .as_ref()
            .and_then(|artifact| self.read_artifact(workspace, &artifact.path).ok())
            .map(|markdown| truncate(&markdown, INLINE_RESEARCH_BYTES))
            .unwrap_or_else(|| "(research notes unavailable)".to_string());
        let criteria = state
            .plan
            .as_ref()
            .map(|plan| plan.acceptance_criteria.clone())
            .unwrap_or_default();
        let criteria = if criteria.is_empty() {
            String::new()
        } else {
            format!(
                "\nAcceptance criteria the verifier will check:\n{}\n",
                criteria
                    .iter()
                    .map(|item| format!("- {item}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        Some(format!(
            "The user approved the plan for request `{request_id}`. Execute it now.\n\
The approved plan is saved at `{plan_path}`{research}; do not edit, rename, or delete them.\n\
Implement the steps in dependency order through the eud-tools file/DAT tools. After source, DAT, plugin, or Python changes, run `build_run`. Repair every compiler error before answering.\n\
The foreground workspace is read-only: do not edit specs, decisions, worklogs, plans, or project memory. Do not call `propose_plan`.\n\
{criteria}\
Answer with a per-step status list (step id — done/partial/skipped and why). A separate verifier will judge the result; the backend creates the post-acceptance harness job after the user accepts the changes.\n\n\
[approved plan]\n{plan_markdown}\n\n[research]\n{research_markdown}"
        ))
    }

    // ---- verify ------------------------------------------------------------

    /// After an executing turn: verify against the plan and re-enter the
    /// executing turn on failure, bounded. Returns when the request may
    /// proceed to changeset review.
    pub(super) async fn workflow_verify_loop(&mut self) -> Result<(), AgentEngineError> {
        if !self.workflow_is_pipeline() {
            return Ok(());
        }
        let request_id = self
            .current_request_id
            .clone()
            .ok_or_else(|| AgentEngineError::new("검증할 요청이 없습니다."))?;
        let has_changes = self
            .journal_store
            .changeset(&request_id)
            .map(|changeset| !changeset.items.is_empty())
            .unwrap_or(false);
        if !has_changes {
            return Ok(());
        }
        let mut triage = self.require_triage()?;
        // The approved plan's criteria are what the executor worked to; the
        // triage criteria remain only when the plan listed none.
        if let Some(criteria) = self
            .current_workflow()
            .and_then(|state| state.plan.as_ref())
            .map(|plan| plan.acceptance_criteria.clone())
            .filter(|criteria| !criteria.is_empty())
        {
            triage.acceptance_criteria = criteria;
        }
        let guides = Self::verifier_guides();
        let workspace = self.prepared_workspace().await?;
        let project = {
            let state = self.workflow.as_ref().expect("workflow state");
            self.stage_project_context(&state.user_text)
        };
        loop {
            let attempt = self
                .workflow
                .as_ref()
                .map(|state| state.verify_attempts)
                .unwrap_or(0)
                .saturating_add(1);
            let plan_markdown = match self.workflow.as_ref().and_then(|state| state.plan.as_ref()) {
                Some(plan) => self
                    .read_artifact(&workspace, &plan.path)
                    .unwrap_or_else(|error| format!("(plan artifact unavailable: {error})")),
                None => "(no plan artifact)".to_string(),
            };
            let changeset_summary = self.changeset_summary(&request_id);
            let build_result = self
                .runtime
                .last_build_result()
                .map(|value| truncate(&value.to_string(), EVIDENCE_BYTES))
                .unwrap_or_else(|| "(no build_run in this request)".to_string());
            let revision = self.runtime.current_project_revision().unwrap_or_default();
            let prompt = {
                let state = self.workflow.as_ref().expect("workflow state");
                workflow::verifier_prompt(
                    &self.stage_context(&guides, &project, state),
                    &triage,
                    &workflow::VerifierInputs {
                        plan_markdown: &plan_markdown,
                        changeset_summary: &changeset_summary,
                        build_result: &build_result,
                        executor_answer: &self.last_answer,
                        revision: &revision,
                    },
                    attempt,
                )
            };
            self.workflow_transition(WorkflowStage::Verifying, |state| {
                state.verify_attempts = attempt;
            })?;
            let value = match self.run_stage(DelegatedRunKind::Verifier, prompt).await {
                Ok(StageOutcome::Value(value)) => value,
                Ok(StageOutcome::Cancelled) => {
                    // The changes stay reviewable; only verification stops.
                    self.workflow_transition(WorkflowStage::ChangesetReview, |_| {})?;
                    return Ok(());
                }
                Err(error) => {
                    self.workflow_transition(WorkflowStage::ChangesetReview, |state| {
                        state.error = Some(error.message.clone());
                    })?;
                    return Ok(());
                }
            };
            let markdown = workflow::render_verdict(&request_id, attempt, &value);
            let artifact = self.write_artifact(
                &workspace,
                &format!("verify/{request_id}.{attempt}.md"),
                &markdown,
            )?;
            let verdict = value
                .get("verdict")
                .and_then(Value::as_str)
                .unwrap_or("fail")
                .to_string();
            let unmet = workflow::verdict_unmet(&value);
            let result = VerifyResult {
                verdict: verdict.clone(),
                summary: value
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                path: artifact.path,
                sha256: artifact.sha256,
                unmet: unmet.clone(),
            };
            self.workflow_transition(WorkflowStage::Verifying, |state| {
                state.verdict = Some(result);
            })?;
            // A failing verdict with nothing an executing turn can fix (only
            // unverifiable criteria) goes to review as it is.
            if verdict == "pass" || attempt >= MAX_VERIFY_ATTEMPTS || unmet.is_empty() {
                self.workflow_transition(WorkflowStage::ChangesetReview, |_| {})?;
                return Ok(());
            }
            // Fix turn on the same request and write ticket.
            self.workflow_transition(WorkflowStage::Executing, |_| {})?;
            let instruction = format!(
                "[verification]\nThe verifier judged attempt {attempt} as fail. Resolve every item below, run `build_run` after runtime changes, and answer again with the per-step status list:\n{}",
                unmet
                    .iter()
                    .map(|item| format!("- {item}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            let turn_text = self
                .prepare_eps_context(&instruction, None, None, false)
                .await?;
            let result = self
                .run_foreground(AgentTurnInput::text(turn_text).with_access(WorkspaceAccess::Write))
                .await?;
            self.commit_context_delivery(&result).await;
            self.thread_active = true;
            let result = self.reinterpret_plan(result);
            if matches!(result, AgentTurnResult::Cancelled) {
                self.workflow_transition(WorkflowStage::ChangesetReview, |_| {})?;
                return Ok(());
            }
            self.handle_turn_result(result)?;
        }
    }

    fn changeset_summary(&self, request_id: &str) -> String {
        let Ok(changeset) = self.journal_store.changeset(request_id) else {
            return "(no changeset)".to_string();
        };
        let mut out = String::new();
        for item in changeset.items {
            let kind = format!("{:?}", item.kind);
            match (&item.path, &item.dat_ref) {
                (Some(path), _) => out.push_str(&format!("### {kind} `{path}`\n")),
                (None, Some(dat)) => out.push_str(&format!(
                    "### {kind} {:?}/{}#{}\n",
                    dat.table, dat.dat, dat.obj_id
                )),
                (None, None) => out.push_str(&format!("### {kind}\n")),
            }
            for property in &item.properties {
                out.push_str(&format!(
                    "- {}: {} → {}\n",
                    property.property, property.old, property.new
                ));
            }
            if let Some(diff) = &item.diff {
                out.push_str("```diff\n");
                out.push_str(diff);
                out.push_str("\n```\n");
            }
        }
        if out.is_empty() {
            "(empty changeset)".to_string()
        } else {
            truncate(&out, CHANGESET_SUMMARY_BYTES)
        }
    }

    // ---- restore / resume --------------------------------------------------

    /// Re-emit the persisted workflow on hydrate and restore plan review.
    pub(super) async fn workflow_hydrate(
        &mut self,
        record: &crate::session::SessionRecord,
    ) -> Result<(), AgentEngineError> {
        let Some(mut state) = record.workflow.clone() else {
            return Ok(());
        };
        if state.stage == WorkflowStage::PlanReview {
            let markdown = match state.plan.as_ref() {
                Some(plan) => match self.prepared_workspace().await {
                    Ok(workspace) => self.read_artifact(&workspace, &plan.path).ok(),
                    Err(_) => None,
                },
                None => None,
            };
            match markdown {
                Some(markdown) => self.restore_plan_review(&state, markdown)?,
                None => {
                    // The card cannot be restored without its file: surface an
                    // explicit interruption instead of a review with no plan.
                    state.interrupted_stage = Some(WorkflowStage::Planning);
                    state.stage = WorkflowStage::Interrupted;
                    state.error = Some(
                        "저장된 계획 파일을 읽을 수 없어 계획 검토를 복원하지 못했습니다."
                            .to_string(),
                    );
                    self.session_store
                        .set_workflow(&self.session_id, Some(state.clone()))
                        .map_err(|error| AgentEngineError::new(error.to_string()))?;
                }
            }
        }
        self.workflow = Some(state.clone());
        self.sink.emit(EngineEvent::Workflow(Box::new(
            workflow::WorkflowEvent::from_state(&state),
        )))
    }

    fn restore_plan_review(
        &mut self,
        state: &WorkflowState,
        markdown: String,
    ) -> Result<(), AgentEngineError> {
        let revision = state.plan.as_ref().map(|plan| plan.revision).unwrap_or(1);
        self.plan_revision = revision;
        self.current_plan_markdown = Some(markdown.clone());
        self.current_request_id = Some(state.request_id.clone());
        self.current_client_turn_id = Some(state.client_turn_id.clone());
        self.current_user_text = state.user_text.clone();
        if self.runtime.current_request_id().as_deref() != Some(state.request_id.as_str()) {
            self.runtime
                .begin_request(&state.request_id, &self.project_id)
                .map_err(AgentEngineError::new)?;
        }
        self.phase = Phase::PlanReview;
        self.sink.emit(EngineEvent::Plan(crate::ipc::PlanEvent {
            markdown,
            revision,
        }))
    }

    /// Resume an interrupted stage from its persisted inputs. Completed stages
    /// are never replayed: research resumes at research, planning at planning,
    /// verifying at verifying (with the changes still pending review).
    pub(super) async fn workflow_resume(&mut self) -> Result<(), AgentEngineError> {
        self.ensure_provider_conversation_ready()?;
        let Some(state) = self.workflow.clone() else {
            return Err(AgentEngineError::new("이어서 진행할 단계 작업이 없습니다."));
        };
        if state.stage != WorkflowStage::Interrupted {
            return Err(AgentEngineError::new(
                "중단된 단계가 아니어서 이어서 진행할 수 없습니다.",
            ));
        }
        let revision = self
            .runtime
            .current_project_revision()
            .map_err(AgentEngineError::new)?;
        let interrupted = state.interrupted_stage.unwrap_or(WorkflowStage::Triage);
        if !matches!(
            interrupted,
            WorkflowStage::Verifying | WorkflowStage::Executing
        ) && revision != state.project_revision_at_start
        {
            return Err(AgentEngineError::new(
                "프로젝트가 중단 시점 이후 변경되어 이어서 진행할 수 없습니다. 처음부터 다시 시작해 주세요.",
            ));
        }
        self.current_request_id = Some(state.request_id.clone());
        self.current_client_turn_id = Some(state.client_turn_id.clone());
        self.current_user_text = state.user_text.clone();
        // Hydrate may already have reopened this request (pending changeset).
        if self.runtime.current_request_id().as_deref() != Some(state.request_id.as_str()) {
            self.runtime
                .begin_request(&state.request_id, &self.project_id)
                .map_err(AgentEngineError::new)?;
        }
        self.phase = Phase::Triage;
        match interrupted {
            WorkflowStage::Triage | WorkflowStage::Clarify => {
                self.workflow = None;
                self.session_store
                    .set_workflow(&self.session_id, None)
                    .map_err(|error| AgentEngineError::new(error.to_string()))?;
                self.phase = Phase::Idle;
                Err(AgentEngineError::new(
                    "파악 단계는 이어서 진행할 수 없습니다. 같은 요청을 다시 보내 주세요.",
                ))
            }
            WorkflowStage::Research => {
                self.workflow_transition(WorkflowStage::Research, |state| {
                    state.interrupted_stage = None
                })?;
                self.workflow_pipeline().await?;
                self.update_active_session().await;
                Ok(())
            }
            WorkflowStage::Planning | WorkflowStage::Critique => {
                self.workflow_transition(WorkflowStage::Planning, |state| {
                    state.interrupted_stage = None;
                    state.plan = None;
                })?;
                self.workflow_plan_round(None).await?;
                self.update_active_session().await;
                Ok(())
            }
            WorkflowStage::Verifying | WorkflowStage::Executing => {
                // The executing turn already settled (or died); whatever it
                // journaled is pending review. Verification is not repeated
                // without the write ticket: surface the changeset for the
                // user's decision, or finish when nothing was changed.
                self.workflow_transition(WorkflowStage::ChangesetReview, |state| {
                    state.interrupted_stage = None
                })?;
                if !self.emit_current_changeset_if_any()? {
                    self.workflow_mark_done()?;
                    self.phase = Phase::Idle;
                }
                self.update_active_session().await;
                Ok(())
            }
            other => Err(AgentEngineError::new(format!(
                "{other:?} 단계는 이어서 진행할 수 없습니다."
            ))),
        }
    }

    /// Discard the interrupted request's stage state and clear it; the panel
    /// resends the same user text as a fresh request.
    pub(super) fn workflow_restart(&mut self) -> Result<String, AgentEngineError> {
        let Some(state) = self.workflow.clone() else {
            return Err(AgentEngineError::new("다시 시작할 단계 작업이 없습니다."));
        };
        if !matches!(
            state.stage,
            WorkflowStage::Interrupted | WorkflowStage::Failed | WorkflowStage::Cancelled
        ) {
            return Err(AgentEngineError::new(
                "진행 중인 요청은 다시 시작할 수 없습니다. 먼저 취소해 주세요.",
            ));
        }
        if matches!(
            self.phase,
            Phase::PlanReview | Phase::Executing | Phase::ChangesetReview
        ) || self.runtime.write_ticket().is_some()
        {
            return Err(AgentEngineError::new(
                "검토 중인 변경이 있어 다시 시작할 수 없습니다. 변경을 먼저 수락하거나 되돌려 주세요.",
            ));
        }
        self.workflow = None;
        self.session_store
            .set_workflow(&self.session_id, None)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        self.phase = Phase::Idle;
        Ok(state.user_text)
    }
}

fn stage_label(kind: DelegatedRunKind) -> &'static str {
    match kind {
        DelegatedRunKind::Triage => "파악",
        DelegatedRunKind::Research | DelegatedRunKind::Read => "조사",
        DelegatedRunKind::Planner => "계획",
        DelegatedRunKind::Architect => "구조 검토",
        DelegatedRunKind::Critic => "계획 검토",
        DelegatedRunKind::Verifier => "검증",
    }
}

fn render_clarification(
    questions: &[crate::ipc::AskQuestion],
    answers: &BTreeMap<String, crate::ipc::AskAnswer>,
) -> String {
    questions
        .iter()
        .map(|question| {
            let answer = answers
                .get(&question.id)
                .map(|answer| answer.answers.join(", "))
                .unwrap_or_default();
            format!("{} → {}", question.question, answer)
        })
        .collect::<Vec<_>>()
        .join("; ")
}
