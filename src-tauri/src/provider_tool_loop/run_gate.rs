use std::{collections::HashSet, sync::Arc, time::Duration};

use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::{mpsc, watch, Notify};

use crate::{
    provider_runtime::{IterationBoundaryReason, RunIdentity, WorkspaceAccess},
    provider_transcript::RunCheckpointWriter,
    tool_exec::SessionToolRuntime,
};

use super::{
    gate_state::{Admission, GateState, RunGateInner},
    profile::{DelegatedToolProfile, ToolProfile, SUBMIT_RESULT_TOOL},
    receipt::RunReceiptStore,
    validation::{validate_call_shape, CallAdmission},
    DirectDispatchBatch, DirectToolCall, DirectToolResult, DurableToolCompletion,
};

/// Completion text for any call a delegated run makes after its submission.
const ENDED_MESSAGE: &str =
    "delegated run already submitted its result; this call was not executed";

/// Completion text for a profile read a delegated run makes on its final
/// tool round, when only `submit_result` is still accepted.
pub const SUBMISSION_ONLY_MESSAGE: &str = "delegated run has used its tool rounds; only submit_result is accepted now and this call was not executed";

#[derive(Clone)]
pub struct RunGate {
    pub(super) inner: Arc<RunGateInner>,
}

impl RunGate {
    pub fn new(
        identity: RunIdentity,
        runtime: SessionToolRuntime,
        workspace_access: WorkspaceAccess,
        checkpoint_writer: Option<Arc<RunCheckpointWriter>>,
    ) -> Self {
        let receipt_store = checkpoint_writer
            .is_none()
            .then(|| RunReceiptStore::new(runtime.data_dirs().journal_dir(), &identity));
        Self::build(
            identity,
            runtime,
            workspace_access,
            ToolProfile::Foreground,
            checkpoint_writer,
            receipt_store,
        )
    }

    /// A gate for an isolated read-only run. It advertises only the profile's
    /// tools plus `submit_result`, never registers write intent, keeps no
    /// transcript checkpoint, and persists no native recovery receipt: a
    /// delegated run is never resumed, so its completions are not recoverable
    /// state.
    pub fn delegated(
        identity: RunIdentity,
        runtime: SessionToolRuntime,
        profile: DelegatedToolProfile,
    ) -> Self {
        Self::build(
            identity,
            runtime,
            WorkspaceAccess::Read,
            ToolProfile::Delegated(profile),
            None,
            None,
        )
    }

    fn build(
        identity: RunIdentity,
        runtime: SessionToolRuntime,
        workspace_access: WorkspaceAccess,
        profile: ToolProfile,
        checkpoint_writer: Option<Arc<RunCheckpointWriter>>,
        receipt_store: Option<RunReceiptStore>,
    ) -> Self {
        let (event_sender, events) = mpsc::unbounded_channel();
        Self {
            inner: Arc::new(RunGateInner {
                identity,
                workspace_access,
                profile,
                runtime,
                checkpoint_writer,
                receipt_store,
                state: Mutex::new(GateState {
                    accepting: true,
                    seen_call_ids: HashSet::new(),
                    in_flight: 0,
                    next_sequence: 0,
                    completed: Vec::new(),
                    write_transition_requested: false,
                    iteration_boundary_requested: None,
                    delegated_result: None,
                    submission_only: false,
                }),
                recording: Mutex::new(()),
                event_sender,
                events: Mutex::new(Some(events)),
                fatal: watch::channel(None).0,
                closed: watch::channel(false).0,
                admitted: Notify::new(),
                drained: Notify::new(),
            }),
        }
    }

    pub fn identity(&self) -> &RunIdentity {
        &self.inner.identity
    }

    /// Every descriptor a call may validate against: the full profile, so a
    /// read on the final round is still a known tool that completes with
    /// [`SUBMISSION_ONLY_MESSAGE`] rather than a fatal unknown-tool error.
    pub fn descriptors(&self) -> Vec<Value> {
        let registry = self.inner.runtime.tool_descriptors();
        match &self.inner.profile {
            ToolProfile::Foreground => registry,
            ToolProfile::Delegated(profile) => profile.descriptors(registry),
        }
    }

    /// The descriptors the model is shown this round: the profile, or
    /// `submit_result` alone once the delegated run is submission-only.
    pub fn advertised_descriptors(&self) -> Vec<Value> {
        match &self.inner.profile {
            ToolProfile::Delegated(profile) if self.submission_only() => {
                profile.submission_descriptors()
            }
            _ => self.descriptors(),
        }
    }

    /// Enter the final tool round of a delegated run: from now on the model
    /// sees only `submit_result`, and any profile read completes with a usage
    /// error instead of executing. Foreground gates ignore it.
    pub fn enter_submission_only(&self) {
        if self.delegated_profile().is_some() {
            self.inner.state.lock().submission_only = true;
        }
    }

    pub fn submission_only(&self) -> bool {
        self.inner.state.lock().submission_only
    }

    /// The usage error for a shape-valid call that the submission-only round
    /// refuses to execute.
    fn submission_only_violation(&self, call: &DirectToolCall) -> Option<String> {
        (self.submission_only() && !self.is_submission(call))
            .then(|| SUBMISSION_ONLY_MESSAGE.to_string())
    }

    fn delegated_profile(&self) -> Option<&DelegatedToolProfile> {
        match &self.inner.profile {
            ToolProfile::Foreground => None,
            ToolProfile::Delegated(profile) => Some(profile),
        }
    }

    /// The accepted `submit_result` payload of a delegated run, if the model
    /// has submitted one. Foreground gates never hold a result.
    pub fn delegated_result(&self) -> Option<Value> {
        self.inner.state.lock().delegated_result.clone()
    }

    pub fn completed(&self) -> Vec<DurableToolCompletion> {
        self.inner.state.lock().completed.clone()
    }

    pub fn receipt_path(&self) -> Option<std::path::PathBuf> {
        self.inner.receipt_store.as_ref().map(RunReceiptStore::path)
    }

    pub fn begin_native_run(
        &self,
        provider: crate::provider::ProviderId,
        prior_native_id: Option<&str>,
    ) -> Result<(), String> {
        self.native_receipt_store()?
            .begin_native(provider, prior_native_id)
    }

    pub fn mark_native_run_unknown(&self) -> Result<(), String> {
        self.native_receipt_store()?.mark_unknown()
    }

    pub fn mark_native_run_completed(
        &self,
        candidate_native_id: Option<&str>,
    ) -> Result<(), String> {
        self.native_receipt_store()?
            .mark_completed(candidate_native_id)
    }

    fn native_receipt_store(&self) -> Result<&RunReceiptStore, String> {
        self.inner
            .receipt_store
            .as_ref()
            .ok_or_else(|| "this gate has no native run receipt".to_string())
    }

    pub fn acknowledge_receipts(&self) -> Result<(), String> {
        self.inner
            .receipt_store
            .as_ref()
            .map(RunReceiptStore::acknowledge)
            .transpose()
            .map(|_| ())
    }

    pub fn close(&self) {
        self.close_and_snapshot_in_flight();
    }

    pub(crate) fn close_for_native_completion(&self) -> usize {
        self.close_and_snapshot_in_flight()
    }

    fn close_and_snapshot_in_flight(&self) -> usize {
        let in_flight = {
            let mut state = self.inner.state.lock();
            state.accepting = false;
            state.in_flight
        };
        self.inner.closed.send_replace(true);
        if self
            .inner
            .runtime
            .matches_request_scope(&self.inner.identity)
        {
            self.inner.runtime.cancel_pending_ask();
        }
        in_flight
    }

    pub fn cancel(&self) {
        self.close();
    }

    pub async fn cancel_and_drain(&self, within: Duration) -> Result<(), String> {
        self.cancel();
        self.drain(within).await
    }

    pub async fn drain(&self, within: Duration) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let notified = self.inner.drained.notified();
            if self.inner.state.lock().in_flight == 0 {
                return Ok(());
            }
            tokio::time::timeout_at(deadline, notified)
                .await
                .map_err(|_| "provider tool drain timed out".to_string())?;
        }
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_admission(&self) {
        loop {
            let notified = self.inner.admitted.notified();
            if self.inner.state.lock().in_flight > 0 {
                return;
            }
            notified.await;
        }
    }

    pub async fn dispatch_batch(
        &self,
        calls: Vec<DirectToolCall>,
        tools_disabled: bool,
    ) -> Result<DirectDispatchBatch, String> {
        let violations = self.reserve_batch(&calls, tools_disabled)?;
        let mut results = Vec::with_capacity(calls.len());
        let mut stop_for_write_transition = false;
        for (call, violation) in calls.into_iter().zip(violations) {
            let result = match violation {
                Some(message) => self.complete_usage_error(&call, message).await?,
                None if self.delegated_result().is_some() => {
                    self.complete_usage_error(&call, ENDED_MESSAGE.to_string())
                        .await?
                }
                None if self.is_submission(&call) => self.complete_submission(&call).await?,
                None => self.execute_call(call).await?,
            };
            results.push(result);
            if self.write_transition_requested() {
                stop_for_write_transition = true;
                break;
            }
        }
        Ok(DirectDispatchBatch {
            results,
            stop_for_write_transition,
        })
    }

    pub async fn dispatch_native(
        &self,
        call_id: Option<String>,
        name: String,
        arguments: Value,
    ) -> Result<DirectToolResult, String> {
        let result = async {
            let call = DirectToolCall {
                id: call_id.clone().unwrap_or_default(),
                name,
                arguments,
            };
            let admission = validate_call_shape(
                &self.descriptors(),
                call_id.as_deref(),
                &call.name,
                &call.arguments,
            )?;
            self.reserve_ids(call_id.iter().map(String::as_str))?;
            match admission {
                CallAdmission::Valid if self.delegated_result().is_some() => {
                    self.complete_usage_error(&call, ENDED_MESSAGE.to_string())
                        .await
                }
                CallAdmission::Valid if self.submission_only_violation(&call).is_some() => {
                    self.complete_usage_error(&call, SUBMISSION_ONLY_MESSAGE.to_string())
                        .await
                }
                CallAdmission::Valid if self.is_submission(&call) => {
                    self.complete_submission(&call).await
                }
                CallAdmission::Valid => self.execute_call(call).await,
                CallAdmission::SchemaViolation(message) => {
                    self.complete_usage_error(&call, message).await
                }
            }
        }
        .await;
        if let Err(error) = &result {
            self.record_fatal(error);
        }
        result
    }

    fn is_submission(&self, call: &DirectToolCall) -> bool {
        self.delegated_profile().is_some() && call.name == SUBMIT_RESULT_TOOL
    }

    /// Capture a schema-valid `submit_result` as the delegated run's result.
    /// Later calls in the same run complete with [`ENDED_MESSAGE`] instead of
    /// executing; the first submission stays the result. The completion is
    /// published like any tool completion so transcripts stay paired, but
    /// nothing is dispatched to the tool runtime.
    async fn complete_submission(&self, call: &DirectToolCall) -> Result<DirectToolResult, String> {
        let admission = self.admit()?;
        self.publish_started(call)?;
        // Set before the completion is recorded so a concurrent native call
        // already sees the run as ended. A failed record is fatal for the gate
        // and the executor never accepts the value, so nothing leaks.
        self.inner.state.lock().delegated_result = Some(call.arguments.clone());
        let completion_inner = Arc::clone(&self.inner);
        let completion_gate = self.clone();
        let call = call.clone();
        let result = tokio::task::spawn_blocking(move || {
            let result =
                super::completion::tool_result(&call, Ok(serde_json::json!({"accepted": true})));
            let recorded = super::gate_state::record_completion(&completion_inner, &call, &result);
            let published = recorded.and_then(|()| completion_gate.publish_completed(&result));
            drop(admission);
            published.map(|()| result)
        })
        .await
        .map_err(|error| format!("provider result submission task failed: {error}"))?;
        result
    }

    /// Fatal-shape the whole batch before admitting anything, then return one
    /// optional schema-violation message per call for recoverable completion.
    fn reserve_batch(
        &self,
        calls: &[DirectToolCall],
        tools_disabled: bool,
    ) -> Result<Vec<Option<String>>, String> {
        if tools_disabled && !calls.is_empty() {
            return Err("provider_structured_output_invalid".to_string());
        }
        let descriptors = self.descriptors();
        let mut batch_ids = HashSet::with_capacity(calls.len());
        let mut violations = Vec::with_capacity(calls.len());
        for call in calls {
            violations.push(
                match validate_call_shape(
                    &descriptors,
                    Some(&call.id),
                    &call.name,
                    &call.arguments,
                )? {
                    CallAdmission::Valid => self.submission_only_violation(call),
                    CallAdmission::SchemaViolation(message) => Some(message),
                },
            );
            if !batch_ids.insert(call.id.as_str()) {
                return Err("provider returned a duplicate tool-call id".to_string());
            }
        }
        self.reserve_ids(calls.iter().map(|call| call.id.as_str()))?;
        Ok(violations)
    }

    fn reserve_ids<'a>(&self, ids: impl Iterator<Item = &'a str>) -> Result<(), String> {
        self.ensure_scope()?;
        let ids = ids.collect::<Vec<_>>();
        let mut state = self.inner.state.lock();
        if !state.accepting {
            return Err("provider tool gate is closed".to_string());
        }
        if ids.iter().any(|id| state.seen_call_ids.contains(*id)) {
            return Err("provider returned a duplicate tool-call id".to_string());
        }
        state
            .seen_call_ids
            .extend(ids.into_iter().map(str::to_string));
        Ok(())
    }

    pub(super) fn admit(&self) -> Result<Admission, String> {
        self.ensure_scope()?;
        let mut state = self.inner.state.lock();
        if !state.accepting {
            return Err("provider tool gate is closed".to_string());
        }
        state.in_flight = state
            .in_flight
            .checked_add(1)
            .ok_or_else(|| "provider tool in-flight count overflow".to_string())?;
        self.inner.admitted.notify_waiters();
        Ok(Admission {
            inner: Arc::clone(&self.inner),
        })
    }

    fn ensure_scope(&self) -> Result<(), String> {
        if self.inner.runtime.matches_run_scope(&self.inner.identity) {
            Ok(())
        } else {
            Err("stale provider run cannot admit tools".to_string())
        }
    }

    pub(super) fn execute_outcome(
        &self,
        call: &DirectToolCall,
        execute: impl FnOnce() -> Result<Value, String>,
    ) -> Result<Value, String> {
        if let Some(profile) = self.delegated_profile() {
            // Descriptor filtering already makes an unlisted name a fatal
            // unknown-tool admission error; this guard keeps a delegated run
            // from ever registering write intent even if a listed name were
            // reclassified. Iteration boundaries belong to the foreground: the
            // delegated executor observes them itself and stops the whole run.
            if !profile.allows(&call.name) || crate::tools::requires_write_workspace(&call.name) {
                let error = format!(
                    "delegated run cannot execute '{}': read-only profile",
                    call.name
                );
                self.record_fatal(&error);
                return Err(error);
            }
            return execute();
        }
        if let Some(reason) = self.iteration_boundary_reason() {
            return Err(match reason {
                IterationBoundaryReason::ToolActions => {
                    "IterationBoundary: 이번 반복에서 300개 도구 작업을 완료했습니다. 이 호출은 실행하지 않았습니다."
                        .to_string()
                }
                _ => {
                    "IterationBoundary: 안전한 재개 지점이 요청되어 이 호출은 실행하지 않았습니다."
                        .to_string()
                }
            });
        }
        if self.inner.workspace_access == WorkspaceAccess::Read
            && self.inner.identity.session_kind == crate::session::SessionKind::Eps
            && crate::tools::requires_write_workspace(&call.name)
        {
            self.inner
                .runtime
                .register_write_request(format!("automatic transition for {}", call.name))?;
            self.inner.state.lock().write_transition_requested = true;
            return Err(
                "WriteWorkspaceTransition: mutation was not executed because this foreground run is read-only. The runtime will resume the same thread in its isolated write context; re-read the target and retry the mutation."
                    .to_string(),
            );
        }
        execute()
    }

    pub(crate) fn has_requested_write_transition(&self) -> bool {
        self.write_transition_requested()
    }

    fn write_transition_requested(&self) -> bool {
        self.inner.state.lock().write_transition_requested
    }

    pub(crate) fn iteration_boundary_reason(&self) -> Option<IterationBoundaryReason> {
        let mut state = self.inner.state.lock();
        if state.iteration_boundary_requested.is_none() {
            state.iteration_boundary_requested = if self.inner.runtime.autonomous_pause_requested()
            {
                Some(IterationBoundaryReason::ProviderContinuation)
            } else if self
                .inner
                .runtime
                .iteration_action_boundary_reached(&self.inner.identity.request_id)
            {
                Some(IterationBoundaryReason::ToolActions)
            } else {
                None
            };
        }
        state.iteration_boundary_requested
    }
}
