use std::{collections::HashSet, sync::Arc};

use parking_lot::Mutex;
use tokio::sync::{mpsc, watch, Notify};

use crate::{
    provider_runtime::{IterationBoundaryReason, RunIdentity, WorkspaceAccess},
    provider_transcript::RunCheckpointWriter,
    tool_exec::SessionToolRuntime,
};

use super::{
    completion::durable_completion, gate_events::GateEvent, receipt::RunReceiptStore, DirectToolCall, DirectToolResult, DurableToolCompletion,
};

pub(super) struct GateState {
    pub(super) accepting: bool,
    pub(super) seen_call_ids: HashSet<String>,
    pub(super) in_flight: usize,
    pub(super) next_sequence: u64,
    pub(super) completed: Vec<DurableToolCompletion>,
    pub(super) write_transition_requested: bool,
    pub(super) iteration_boundary_requested: Option<IterationBoundaryReason>,
}

pub(super) struct RunGateInner {
    pub(super) identity: RunIdentity,
    pub(super) workspace_access: WorkspaceAccess,
    pub(super) runtime: SessionToolRuntime,
    pub(super) checkpoint_writer: Option<Arc<RunCheckpointWriter>>,
    pub(super) receipt_store: Option<RunReceiptStore>,
    pub(super) state: Mutex<GateState>,
    pub(super) recording: Mutex<()>,
    pub(super) event_sender: mpsc::UnboundedSender<GateEvent>,
    pub(super) events: Mutex<Option<mpsc::UnboundedReceiver<GateEvent>>>,
    pub(super) fatal: watch::Sender<Option<String>>,
    pub(super) closed: watch::Sender<bool>,
    pub(super) admitted: Notify,
    pub(super) drained: Notify,
}

pub(super) struct Admission {
    pub(super) inner: Arc<RunGateInner>,
}

impl Drop for Admission {
    fn drop(&mut self) {
        let drained = {
            let mut state = self.inner.state.lock();
            state.in_flight = state.in_flight.saturating_sub(1);
            state.in_flight == 0
        };
        if drained {
            self.inner.drained.notify_waiters();
        }
    }
}

pub(super) fn record_completion(
    inner: &RunGateInner,
    call: &DirectToolCall,
    result: &DirectToolResult,
) -> Result<(), String> {
    let _recording = inner.recording.lock();
    let journal = match inner.runtime.journal().persist(&inner.identity.request_id) {
        Ok(()) | Err(crate::journal::JournalError::MissingJournal { .. }) => Ok(()),
        Err(error) => Err(format!(
            "provider tool journal cannot be persisted: {error}"
        )),
    };
    let checkpoint = inner
        .checkpoint_writer
        .as_ref()
        .map(|writer| writer.commit_provisional_tool_result(call, result))
        .transpose();
    let completed = {
        let mut state = inner.state.lock();
        state.next_sequence = state
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| "provider tool completion sequence overflow".to_string())?;
        let sequence = state.next_sequence;
        state
            .completed
            .push(durable_completion(sequence, &inner.identity, call, result));
        if state.iteration_boundary_requested.is_none() {
            state.iteration_boundary_requested = if inner.runtime.autonomous_pause_requested() {
                Some(IterationBoundaryReason::ProviderContinuation)
            } else if inner
                .runtime
                .iteration_action_boundary_reached(&inner.identity.request_id)
            {
                Some(IterationBoundaryReason::ToolActions)
            } else {
                None
            };
        }
        state.completed.clone()
    };
    let receipt = inner
        .receipt_store
        .as_ref()
        .map(|store| store.persist(&completed))
        .transpose();
    journal.and(checkpoint).and(receipt).map(|_| ())
}
