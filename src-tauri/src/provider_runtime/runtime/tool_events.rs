use std::{sync::Arc, time::Duration};

use crate::{
    provider_runtime::{AdapterEventKind, NormalizedBlock, ProviderRuntimeError},
    provider_tool_loop::{GateEvent, GateEventKind, RunGate},
};

use super::RuntimeEventSink;

pub(crate) struct RunToolEvents {
    gate: RunGate,
    receiver: tokio::sync::mpsc::UnboundedReceiver<GateEvent>,
    fatal: tokio::sync::watch::Receiver<Option<String>>,
    sink: Arc<dyn RuntimeEventSink>,
    response_id: String,
    batch_id: String,
}

impl RunToolEvents {
    pub(super) fn new(
        gate: &RunGate,
        sink: Arc<dyn RuntimeEventSink>,
    ) -> Result<Self, ProviderRuntimeError> {
        Ok(Self {
            gate: gate.clone(),
            receiver: gate.take_events().map_err(ProviderRuntimeError::Protocol)?,
            fatal: gate.subscribe_fatal(),
            sink,
            response_id: gate.identity().request_id.clone(),
            batch_id: gate.identity().run_id.get().to_string(),
        })
    }

    pub(super) fn set_batch(&mut self, response_id: &str, batch_id: &str) {
        self.response_id = response_id.to_string();
        self.batch_id = batch_id.to_string();
    }

    pub(super) fn close_for_native_completion(&self) -> usize {
        self.gate.close_for_native_completion()
    }

    /// Record the resumable native session of the run in progress, so an
    /// interruption that never reaches a terminal boundary still leaves one.
    pub(super) fn mark_native_session_started(&self, session_id: &str) -> Result<(), String> {
        self.gate.mark_native_run_interrupted(session_id)
    }

    pub(super) fn check_fatal(&self) -> Result<(), ProviderRuntimeError> {
        match self.fatal.borrow().as_ref() {
            Some(error) => Err(ProviderRuntimeError::Protocol(error.clone())),
            None => Ok(()),
        }
    }

    pub(super) async fn next(&mut self) -> Result<(), ProviderRuntimeError> {
        self.check_fatal()?;
        loop {
            tokio::select! {
                biased;
                event = self.receiver.recv() => return self.publish_received(event),
                changed = self.fatal.changed() => {
                    changed.map_err(|_| ProviderRuntimeError::Protocol("tool gate failure observer closed".into()))?;
                    self.check_fatal()?;
                }
            }
        }
    }

    pub(super) fn drain_ready(&mut self) -> Result<(), ProviderRuntimeError> {
        while let Ok(event) = self.receiver.try_recv() {
            self.publish_received(Some(event))?;
        }
        Ok(())
    }

    pub(super) async fn drain(&mut self, within: Duration) -> Result<(), ProviderRuntimeError> {
        let gate = self.gate.clone();
        let drained = gate.drain(within);
        tokio::pin!(drained);
        loop {
            tokio::select! {
                event = self.receiver.recv() => self.publish_received(event)?,
                result = &mut drained => {
                    let result = result.map_err(ProviderRuntimeError::Transport);
                    let published = self.drain_ready();
                    return result.and(published);
                }
            }
        }
    }

    fn publish_received(&self, event: Option<GateEvent>) -> Result<(), ProviderRuntimeError> {
        let event = event.ok_or_else(|| {
            ProviderRuntimeError::Protocol("tool gate event stream closed".into())
        })?;
        if &event.identity != self.gate.identity() {
            return Err(ProviderRuntimeError::StaleEvent);
        }
        let block = match event.kind {
            GateEventKind::Started(call) => NormalizedBlock::ToolCall {
                response_id: self.response_id.clone(),
                batch_id: self.batch_id.clone(),
                call,
                continuation: None,
            },
            GateEventKind::Completed(result) => NormalizedBlock::ToolResult {
                response_id: self.response_id.clone(),
                batch_id: self.batch_id.clone(),
                result,
            },
        };
        self.sink.emit(&AdapterEventKind::Block(block))
    }
}
