use tokio::sync::{mpsc, watch};

use crate::provider_runtime::RunIdentity;

use super::{DirectToolCall, DirectToolResult, RunGate};

#[derive(Debug, Clone, PartialEq)]
pub struct GateEvent {
    pub identity: RunIdentity,
    pub kind: GateEventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GateEventKind {
    Started(DirectToolCall),
    Completed(DirectToolResult),
}

impl RunGate {
    pub fn take_events(&self) -> Result<mpsc::UnboundedReceiver<GateEvent>, String> {
        self.inner
            .events
            .lock()
            .take()
            .ok_or_else(|| "provider tool gate events already have a consumer".to_string())
    }

    pub fn subscribe_fatal(&self) -> watch::Receiver<Option<String>> {
        self.inner.fatal.subscribe()
    }

    pub fn fatal_admission_error(&self) -> Option<String> {
        self.inner.fatal.borrow().clone()
    }

    pub(super) fn publish_started(&self, call: &DirectToolCall) -> Result<(), String> {
        self.publish(GateEventKind::Started(call.clone()))
    }

    pub(super) fn publish_completed(&self, result: &DirectToolResult) -> Result<(), String> {
        self.publish(GateEventKind::Completed(result.clone()))
    }

    pub(super) fn record_fatal(&self, error: &str) {
        let recorded = self.inner.fatal.send_if_modified(|current| {
            if current.is_some() {
                false
            } else {
                *current = Some(error.to_string());
                true
            }
        });
        if recorded {
            self.close();
        }
    }

    fn publish(&self, kind: GateEventKind) -> Result<(), String> {
        self.inner
            .event_sender
            .send(GateEvent {
                identity: self.inner.identity.clone(),
                kind,
            })
            .map_err(|_| "provider tool event consumer closed".to_string())
    }
}
