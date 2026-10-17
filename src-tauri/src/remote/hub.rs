//! Project events for remote clients.
//!
//! Desktop windows receive a project's events through `emit_to`; a phone has
//! no window, so [`crate::project_events::emit_project`] also publishes into
//! this hub (as does `session_activity`, scoped to the session's project),
//! and each remote event socket forwards what belongs to the project it is
//! attached to.
//! Publishing costs nothing while no socket listens: the payload is not even
//! serialized.

use std::sync::Arc;

use serde::Serialize;
use tokio::sync::broadcast;

use crate::project_id::ProjectId;

/// How many events a slow socket may fall behind before it must resync.
const CAPACITY: usize = 1024;

/// One event as a remote client receives it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteEvent {
    /// The project it belongs to; `None` for a process-wide event.
    pub project_id: Option<ProjectId>,
    pub event: String,
    pub payload: serde_json::Value,
}

impl RemoteEvent {
    /// Whether a socket attached to `attached` should receive this event: a
    /// project's events go only to sockets attached to that project, and a
    /// process-wide event goes to every socket.
    pub fn is_for(&self, attached: Option<&ProjectId>) -> bool {
        match &self.project_id {
            None => true,
            Some(project) => attached == Some(project),
        }
    }
}

#[derive(Clone)]
pub struct RemoteHub {
    sender: broadcast::Sender<Arc<RemoteEvent>>,
}

impl Default for RemoteHub {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteHub {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(CAPACITY);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<RemoteEvent>> {
        self.sender.subscribe()
    }

    /// Whether any remote socket listens.
    pub fn has_listeners(&self) -> bool {
        self.sender.receiver_count() > 0
    }

    /// Publish `event` of `project`.
    pub fn publish_project<S: Serialize>(&self, project: &ProjectId, event: &str, payload: &S) {
        self.publish(Some(project), event, payload);
    }

    /// Publish a process-wide `event`.
    pub fn publish_global<S: Serialize>(&self, event: &str, payload: &S) {
        self.publish(None, event, payload);
    }

    fn publish<S: Serialize>(&self, project: Option<&ProjectId>, event: &str, payload: &S) {
        if self.sender.receiver_count() == 0 {
            return;
        }
        let payload = match serde_json::to_value(payload) {
            Ok(payload) => payload,
            Err(error) => {
                eprintln!("eud-agent: remote event {event} could not be serialized: {error}");
                return;
            }
        };
        // A send fails only when every receiver went away meanwhile.
        let _ = self.sender.send(Arc::new(RemoteEvent {
            project_id: project.cloned(),
            event: event.to_string(),
            payload,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_events_reach_only_sockets_attached_to_that_project() {
        let a = ProjectId::from_stored("a");
        let b = ProjectId::from_stored("b");
        let hub = RemoteHub::new();
        let mut receiver = hub.subscribe();
        hub.publish_project(&a, "answer", &serde_json::json!({"text": "hi"}));
        hub.publish_global("session_activity", &serde_json::json!({"sessionId": "s"}));

        let first = receiver.try_recv().unwrap();
        assert_eq!(first.event, "answer");
        assert_eq!(first.payload["text"], "hi");
        assert!(first.is_for(Some(&a)));
        assert!(!first.is_for(Some(&b)));
        assert!(!first.is_for(None));

        let second = receiver.try_recv().unwrap();
        assert_eq!(second.project_id, None);
        assert!(second.is_for(Some(&a)));
        assert!(second.is_for(Some(&b)));
        assert!(second.is_for(None));
    }

    #[test]
    fn publishing_without_listeners_is_a_no_op() {
        struct Panics;
        impl Serialize for Panics {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                panic!("serialized with no listener");
            }
        }
        let hub = RemoteHub::new();
        hub.publish_global("session_activity", &Panics);
        hub.publish_project(&ProjectId::from_stored("a"), "answer", &Panics);
    }

    #[test]
    fn a_lagging_receiver_is_told_so() {
        let hub = RemoteHub::new();
        let mut receiver = hub.subscribe();
        for index in 0..CAPACITY + 1 {
            hub.publish_global("tick", &index);
        }
        assert!(matches!(
            receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
    }
}
