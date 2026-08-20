//! Project-scoped FIFO serialization for complete write transactions.
//!
//! Read turns never enter this coordinator. A ticket starts when a session declares
//! write intent and remains the project owner until mutation, build, changeset review,
//! and every accept/reject/rollback step have settled.

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionActivity {
    Idle,
    RunningRead,
    WaitingWrite,
    RunningWrite,
    Review,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActivityEvent {
    pub session_id: String,
    pub activity: SessionActivity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOwner {
    pub project_id: String,
    pub session_id: String,
    pub request_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketState {
    Granted,
    Waiting(usize),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct WriteTicket {
    project_id: String,
    session_id: String,
    request_id: String,
    state: watch::Receiver<TicketState>,
}

impl WriteTicket {
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn state(&self) -> TicketState {
        *self.state.borrow()
    }

    pub async fn wait_for_grant(&mut self) -> Result<(), String> {
        loop {
            match *self.state.borrow_and_update() {
                TicketState::Granted => return Ok(()),
                TicketState::Cancelled => return Err("write ticket was cancelled".to_string()),
                TicketState::Waiting(_) => {}
            }
            self.state
                .changed()
                .await
                .map_err(|_| "write coordinator stopped before granting the ticket".to_string())?;
        }
    }
}

#[derive(Clone)]
pub struct ProjectWriteCoordinator {
    inner: Arc<Mutex<CoordinatorState>>,
    activity: Arc<dyn Fn(SessionActivityEvent) + Send + Sync>,
}

#[derive(Default)]
struct CoordinatorState {
    projects: HashMap<String, ProjectQueue>,
}

#[derive(Default)]
struct ProjectQueue {
    owner: Option<QueueEntry>,
    waiting: VecDeque<QueueEntry>,
}

struct QueueEntry {
    session_id: String,
    request_id: String,
    state: watch::Sender<TicketState>,
}

impl ProjectWriteCoordinator {
    pub fn new(activity: impl Fn(SessionActivityEvent) + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CoordinatorState::default())),
            activity: Arc::new(activity),
        }
    }

    pub fn silent() -> Self {
        Self::new(|_| {})
    }

    pub fn emit_activity(
        &self,
        session_id: impl Into<String>,
        activity: SessionActivity,
        queue_position: Option<usize>,
    ) {
        (self.activity)(SessionActivityEvent {
            session_id: session_id.into(),
            activity,
            queue_position,
            blocking_session_id: None,
        });
    }

    pub fn request(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<WriteTicket, String> {
        validate_key(project_id, "project id")?;
        validate_key(session_id, "session id")?;
        validate_key(request_id, "request id")?;

        let mut events = Vec::new();
        let ticket = {
            let mut state = self.inner.lock();
            if let Some(ticket) = existing_ticket(&state, project_id, request_id) {
                return Ok(ticket);
            }

            let queue = state.projects.entry(project_id.to_string()).or_default();
            if queue.owner.is_none() {
                let (sender, receiver) = watch::channel(TicketState::Granted);
                queue.owner = Some(QueueEntry {
                    session_id: session_id.to_string(),
                    request_id: request_id.to_string(),
                    state: sender,
                });
                events.push(SessionActivityEvent {
                    session_id: session_id.to_string(),
                    activity: SessionActivity::RunningWrite,
                    queue_position: None,
                    blocking_session_id: None,
                });
                WriteTicket {
                    project_id: project_id.to_string(),
                    session_id: session_id.to_string(),
                    request_id: request_id.to_string(),
                    state: receiver,
                }
            } else {
                let blocking_session_id =
                    queue.owner.as_ref().map(|owner| owner.session_id.clone());
                let position = queue.waiting.len() + 1;
                let (sender, receiver) = watch::channel(TicketState::Waiting(position));
                queue.waiting.push_back(QueueEntry {
                    session_id: session_id.to_string(),
                    request_id: request_id.to_string(),
                    state: sender,
                });
                events.push(SessionActivityEvent {
                    session_id: session_id.to_string(),
                    activity: SessionActivity::WaitingWrite,
                    queue_position: Some(position),
                    blocking_session_id,
                });
                WriteTicket {
                    project_id: project_id.to_string(),
                    session_id: session_id.to_string(),
                    request_id: request_id.to_string(),
                    state: receiver,
                }
            }
        };
        self.emit_all(events);
        Ok(ticket)
    }

    pub fn cancel(&self, request_id: &str) -> Result<bool, String> {
        let mut events = Vec::new();
        let removed = {
            let mut state = self.inner.lock();
            let mut removed = None;
            for queue in state.projects.values_mut() {
                if queue
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner.request_id == request_id)
                {
                    return Ok(false);
                }
                if let Some(index) = queue
                    .waiting
                    .iter()
                    .position(|entry| entry.request_id == request_id)
                {
                    let entry = queue.waiting.remove(index).expect("known queue index");
                    entry.state.send_replace(TicketState::Cancelled);
                    events.push(SessionActivityEvent {
                        session_id: entry.session_id.clone(),
                        activity: SessionActivity::Idle,
                        queue_position: None,
                        blocking_session_id: None,
                    });
                    refresh_waiting_positions(queue, &mut events);
                    removed = Some(());
                    break;
                }
            }
            removed.is_some()
        };
        self.emit_all(events);
        Ok(removed)
    }

    pub fn release(&self, request_id: &str) -> Result<bool, String> {
        let mut events = Vec::new();
        let released = {
            let mut state = self.inner.lock();
            let Some((project_id, queue)) = state.projects.iter_mut().find(|(_, queue)| {
                queue
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner.request_id == request_id)
            }) else {
                return Ok(false);
            };
            let previous = queue.owner.take().expect("matched owner");
            previous.state.send_replace(TicketState::Cancelled);
            events.push(SessionActivityEvent {
                session_id: previous.session_id,
                activity: SessionActivity::Idle,
                queue_position: None,
                blocking_session_id: None,
            });

            if let Some(next) = queue.waiting.pop_front() {
                next.state.send_replace(TicketState::Granted);
                events.push(SessionActivityEvent {
                    session_id: next.session_id.clone(),
                    activity: SessionActivity::RunningWrite,
                    queue_position: None,
                    blocking_session_id: None,
                });
                queue.owner = Some(next);
                refresh_waiting_positions(queue, &mut events);
            }
            let project_id = project_id.clone();
            if queue.owner.is_none() && queue.waiting.is_empty() {
                state.projects.remove(&project_id);
            }
            true
        };
        self.emit_all(events);
        Ok(released)
    }

    pub fn restore_review(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<WriteTicket, String> {
        validate_key(project_id, "project id")?;
        validate_key(session_id, "session id")?;
        validate_key(request_id, "request id")?;

        let ticket = {
            let mut state = self.inner.lock();
            if let Some(ticket) = existing_ticket(&state, project_id, request_id) {
                return Ok(ticket);
            }
            let queue = state.projects.entry(project_id.to_string()).or_default();
            if let Some(owner) = &queue.owner {
                return Err(format!(
                    "cannot restore pending review {request_id}; project {project_id} is already owned by {}",
                    owner.request_id
                ));
            }
            if !queue.waiting.is_empty() {
                return Err(format!(
                    "cannot restore pending review {request_id}; project {project_id} already has queued writers"
                ));
            }
            let (sender, receiver) = watch::channel(TicketState::Granted);
            queue.owner = Some(QueueEntry {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                state: sender,
            });
            WriteTicket {
                project_id: project_id.to_string(),
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                state: receiver,
            }
        };
        self.emit_activity(session_id, SessionActivity::Review, None);
        Ok(ticket)
    }

    pub fn owner(&self, project_id: &str) -> Option<WriteOwner> {
        let state = self.inner.lock();
        let owner = state.projects.get(project_id)?.owner.as_ref()?;
        Some(WriteOwner {
            project_id: project_id.to_string(),
            session_id: owner.session_id.clone(),
            request_id: owner.request_id.clone(),
        })
    }

    pub fn owns(&self, project_id: &str, session_id: &str, request_id: &str) -> bool {
        self.owner(project_id)
            .is_some_and(|owner| owner.session_id == session_id && owner.request_id == request_id)
    }

    fn emit_all(&self, events: Vec<SessionActivityEvent>) {
        for event in events {
            (self.activity)(event);
        }
    }
}

fn existing_ticket(
    state: &CoordinatorState,
    project_id: &str,
    request_id: &str,
) -> Option<WriteTicket> {
    let queue = state.projects.get(project_id)?;
    if let Some(owner) = queue
        .owner
        .as_ref()
        .filter(|owner| owner.request_id == request_id)
    {
        return Some(WriteTicket {
            project_id: project_id.to_string(),
            session_id: owner.session_id.clone(),
            request_id: owner.request_id.clone(),
            state: owner.state.subscribe(),
        });
    }
    let (index, waiting) = queue
        .waiting
        .iter()
        .enumerate()
        .find(|(_, waiting)| waiting.request_id == request_id)?;
    let waiting_state = TicketState::Waiting(index + 1);
    if *waiting.state.borrow() != waiting_state {
        waiting.state.send_replace(waiting_state);
    }
    Some(WriteTicket {
        project_id: project_id.to_string(),
        session_id: waiting.session_id.clone(),
        request_id: waiting.request_id.clone(),
        state: waiting.state.subscribe(),
    })
}

fn refresh_waiting_positions(queue: &mut ProjectQueue, events: &mut Vec<SessionActivityEvent>) {
    let blocking_session_id = queue.owner.as_ref().map(|owner| owner.session_id.clone());
    for (index, entry) in queue.waiting.iter().enumerate() {
        let position = index + 1;
        if *entry.state.borrow() != TicketState::Waiting(position) {
            entry.state.send_replace(TicketState::Waiting(position));
            events.push(SessionActivityEvent {
                session_id: entry.session_id.clone(),
                activity: SessionActivity::WaitingWrite,
                queue_position: Some(position),
                blocking_session_id: blocking_session_id.clone(),
            });
        }
    }
}

fn validate_key(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn tickets_are_fifo_by_write_intent_and_review_keeps_the_owner() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        let writes = ProjectWriteCoordinator::new(move |event| {
            captured.lock().push(event);
        });

        let first = writes.request("project", "session-c", "request-c").unwrap();
        let second = writes.request("project", "session-e", "request-e").unwrap();
        let third = writes.request("project", "session-f", "request-f").unwrap();
        assert_eq!(first.state(), TicketState::Granted);
        assert_eq!(second.state(), TicketState::Waiting(1));
        assert_eq!(third.state(), TicketState::Waiting(2));
        assert!(events.lock().iter().any(|event| {
            event.session_id == "session-e"
                && event.activity == SessionActivity::WaitingWrite
                && event.blocking_session_id.as_deref() == Some("session-c")
        }));

        writes.emit_activity("session-c", SessionActivity::Review, None);
        assert_eq!(writes.owner("project").unwrap().request_id, "request-c");
        assert_eq!(second.state(), TicketState::Waiting(1));

        assert!(writes.release("request-c").unwrap());
        assert_eq!(second.state(), TicketState::Granted);
        assert_eq!(third.state(), TicketState::Waiting(1));
        assert_eq!(writes.owner("project").unwrap().request_id, "request-e");
    }

    #[tokio::test]
    async fn queued_cancel_and_grant_are_request_scoped() {
        let writes = ProjectWriteCoordinator::silent();
        let _owner = writes.request("project", "owner", "owner-request").unwrap();
        let mut cancelled = writes
            .request("project", "cancelled", "cancel-request")
            .unwrap();
        let mut next = writes.request("project", "next", "next-request").unwrap();

        assert!(writes.cancel("cancel-request").unwrap());
        assert!(cancelled.wait_for_grant().await.is_err());
        assert_eq!(next.state(), TicketState::Waiting(1));
        assert!(writes.release("owner-request").unwrap());
        next.wait_for_grant().await.unwrap();
        assert_eq!(writes.owner("project").unwrap().session_id, "next");
    }

    #[test]
    fn one_project_never_has_two_owners_under_concurrent_requests() {
        let writes = Arc::new(ProjectWriteCoordinator::silent());
        let start = Arc::new(Barrier::new(3));
        let mut joins = Vec::new();
        for (session, request) in [("a", "request-a"), ("b", "request-b")] {
            let writes = Arc::clone(&writes);
            let start = Arc::clone(&start);
            joins.push(std::thread::spawn(move || {
                start.wait();
                writes.request("project", session, request).unwrap()
            }));
        }
        start.wait();
        let tickets: Vec<_> = joins.into_iter().map(|join| join.join().unwrap()).collect();
        assert_eq!(
            tickets
                .iter()
                .filter(|ticket| ticket.state() == TicketState::Granted)
                .count(),
            1
        );
        assert_eq!(
            tickets
                .iter()
                .filter(|ticket| matches!(ticket.state(), TicketState::Waiting(1)))
                .count(),
            1
        );
    }

    #[test]
    fn restored_review_precedes_new_writers_and_conflicts_are_explicit() {
        let writes = ProjectWriteCoordinator::silent();
        let restored = writes
            .restore_review("project", "review-session", "review-request")
            .unwrap();
        let next = writes
            .request("project", "new-session", "new-request")
            .unwrap();
        assert_eq!(restored.state(), TicketState::Granted);
        assert_eq!(next.state(), TicketState::Waiting(1));
        assert!(writes
            .restore_review("project", "other-session", "other-request")
            .unwrap_err()
            .contains("already owned"));
    }
}
