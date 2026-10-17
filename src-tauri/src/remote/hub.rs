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

/// Forward session activity to remote sockets of the session's project
/// only. `project_of` names the open project a session belongs to (it reads
/// the session index, so it runs off the async runtime); a session of no
/// open project reaches no socket. Events keep their order.
pub fn session_activity_router(
    hub: RemoteHub,
    project_of: impl Fn(&str) -> Option<ProjectId> + Send + Sync + 'static,
) -> tokio::sync::mpsc::UnboundedSender<crate::write_coordinator::SessionActivityEvent> {
    let (sender, mut events) =
        tokio::sync::mpsc::unbounded_channel::<crate::write_coordinator::SessionActivityEvent>();
    let project_of = Arc::new(project_of);
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            let lookup = project_of.clone();
            let session = event.session_id.clone();
            let project = tauri::async_runtime::spawn_blocking(move || lookup(&session))
                .await
                .ok()
                .flatten();
            if let Some(project) = project {
                hub.publish_project(&project, "session_activity", &event);
            }
        }
    });
    sender
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_activity_reaches_only_its_projects_sockets() {
        let a = ProjectId::from_stored("a");
        let b = ProjectId::from_stored("b");
        let hub = RemoteHub::new();
        let mut receiver = hub.subscribe();
        let owner = a.clone();
        let router = session_activity_router(hub.clone(), move |session| {
            (session == "s-a").then(|| owner.clone())
        });
        for session in ["s-closed", "s-a"] {
            router
                .send(crate::write_coordinator::SessionActivityEvent {
                    session_id: session.to_string(),
                    activity: crate::write_coordinator::SessionActivity::RunningRead,
                })
                .unwrap();
        }
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.event, "session_activity");
        assert_eq!(event.payload["sessionId"], "s-a");
        assert!(event.is_for(Some(&a)));
        assert!(!event.is_for(Some(&b)));
        assert!(!event.is_for(None));
        // The session of no open project was published nowhere.
        assert!(receiver.try_recv().is_err());
    }

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
