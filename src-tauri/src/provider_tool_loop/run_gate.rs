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
    receipt::RunReceiptStore,
    validation::{validate_call_shape, CallAdmission},
    DirectDispatchBatch, DirectToolCall, DirectToolResult, DurableToolCompletion,
};

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
            checkpoint_writer,
            receipt_store,
        )
    }

    fn build(
        identity: RunIdentity,
        runtime: SessionToolRuntime,
        workspace_access: WorkspaceAccess,
        checkpoint_writer: Option<Arc<RunCheckpointWriter>>,
        receipt_store: Option<RunReceiptStore>,
    ) -> Self {
        let (event_sender, events) = mpsc::unbounded_channel();
        Self {
            inner: Arc::new(RunGateInner {
                identity,
                workspace_access,
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

    /// Every descriptor a call may validate against.
    pub fn descriptors(&self) -> Vec<Value> {
        self.inner.runtime.tool_descriptors()
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

    /// Record the resumable native session of a run that was cancelled or
    /// failed after that session was observed.
    pub fn mark_native_run_interrupted(&self, candidate_native_id: &str) -> Result<(), String> {
        self.native_receipt_store()?
            .mark_interrupted(candidate_native_id)
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
                    CallAdmission::Valid => None,
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
