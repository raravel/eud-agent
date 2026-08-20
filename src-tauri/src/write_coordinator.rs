//! Concurrent session write registration with short project transactions.
//!
//! Each session edits its isolated workspace without waiting for another session's
//! review. Only an operation that touches shared project state enters `transaction`;
//! the critical section ends as soon as that operation has atomically settled.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionActivity {
    Idle,
    RunningRead,
    RunningWrite,
    Review,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActivityEvent {
    pub session_id: String,
    pub activity: SessionActivity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketState {
    Granted,
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
}

#[derive(Clone)]
pub struct ProjectWriteCoordinator {
    active: Arc<Mutex<HashMap<String, ActiveWrite>>>,
    transactions: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    activity: Arc<dyn Fn(SessionActivityEvent) + Send + Sync>,
}

struct ActiveWrite {
    project_id: String,
    session_id: String,
    state: watch::Sender<TicketState>,
}

impl ProjectWriteCoordinator {
    pub fn new(activity: impl Fn(SessionActivityEvent) + Send + Sync + 'static) -> Self {
        Self {
            active: Arc::new(Mutex::new(HashMap::new())),
            transactions: Arc::new(Mutex::new(HashMap::new())),
            activity: Arc::new(activity),
        }
    }

    pub fn silent() -> Self {
        Self::new(|_| {})
    }

    pub fn emit_activity(&self, session_id: impl Into<String>, activity: SessionActivity) {
        (self.activity)(SessionActivityEvent {
            session_id: session_id.into(),
            activity,
        });
    }

    pub fn request(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<WriteTicket, String> {
        self.register(
            project_id,
            session_id,
            request_id,
            SessionActivity::RunningWrite,
        )
    }

    pub fn release(&self, request_id: &str) -> Result<bool, String> {
        validate_key(request_id, "request id")?;
        let removed = self.active.lock().remove(request_id);
        let Some(entry) = removed else {
            return Ok(false);
        };
        entry.state.send_replace(TicketState::Cancelled);
        self.emit_activity(entry.session_id, SessionActivity::Idle);
        Ok(true)
    }

    pub fn restore_review(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<WriteTicket, String> {
        self.register(project_id, session_id, request_id, SessionActivity::Review)
    }

    pub fn owns(&self, project_id: &str, session_id: &str, request_id: &str) -> bool {
        self.active
            .lock()
            .get(request_id)
            .is_some_and(|entry| entry.project_id == project_id && entry.session_id == session_id)
    }

    /// Serialize one operation that reads or mutates shared canonical project state.
    ///
    /// Session workspace editing and review stay outside this lock. Callers should
    /// perform all validation before entering and return only after rollback metadata
    /// and shared bytes have settled.
    pub fn transaction<T>(
        &self,
        project_id: &str,
        operation: impl FnOnce() -> T,
    ) -> Result<T, String> {
        validate_key(project_id, "project id")?;
        let project_lock = {
            let mut transactions = self.transactions.lock();
            Arc::clone(
                transactions
                    .entry(project_id.to_string())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _transaction = project_lock.lock();
        Ok(operation())
    }

    fn register(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
        activity: SessionActivity,
    ) -> Result<WriteTicket, String> {
        validate_key(project_id, "project id")?;
        validate_key(session_id, "session id")?;
        validate_key(request_id, "request id")?;

        let ticket = {
            let mut active = self.active.lock();
            if let Some(entry) = active.get(request_id) {
                if entry.project_id != project_id || entry.session_id != session_id {
                    return Err(format!(
                        "write request `{request_id}` is already registered for another session or project"
                    ));
                }
                return Ok(WriteTicket {
                    project_id: entry.project_id.clone(),
                    session_id: entry.session_id.clone(),
                    request_id: request_id.to_string(),
                    state: entry.state.subscribe(),
                });
            }

            let (sender, receiver) = watch::channel(TicketState::Granted);
            active.insert(
                request_id.to_string(),
                ActiveWrite {
                    project_id: project_id.to_string(),
                    session_id: session_id.to_string(),
                    state: sender,
                },
            );
            WriteTicket {
                project_id: project_id.to_string(),
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                state: receiver,
            }
        };
        self.emit_activity(session_id, activity);
        Ok(ticket)
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;
    use std::time::Duration;

    #[test]
    fn tickets_for_the_same_project_are_granted_concurrently() {
        let writes = ProjectWriteCoordinator::silent();

        let first = writes.request("project", "session-a", "request-a").unwrap();
        let second = writes.request("project", "session-b", "request-b").unwrap();

        assert_eq!(first.state(), TicketState::Granted);
        assert_eq!(second.state(), TicketState::Granted);
        assert!(writes.owns("project", "session-a", "request-a"));
        assert!(writes.owns("project", "session-b", "request-b"));
    }

    #[test]
    fn restored_reviews_coexist_with_new_writers() {
        let writes = ProjectWriteCoordinator::silent();

        let restored = writes
            .restore_review("project", "review-session", "review-request")
            .unwrap();
        let next = writes
            .request("project", "new-session", "new-request")
            .unwrap();

        assert_eq!(restored.state(), TicketState::Granted);
        assert_eq!(next.state(), TicketState::Granted);
        assert!(writes.owns("project", "review-session", "review-request"));
        assert!(writes.owns("project", "new-session", "new-request"));
    }

    #[test]
    fn project_transactions_serialize_only_the_apply_step() {
        let writes = Arc::new(ProjectWriteCoordinator::silent());
        let start = Arc::new(Barrier::new(3));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut joins = Vec::new();

        for _ in 0..2 {
            let writes = Arc::clone(&writes);
            let start = Arc::clone(&start);
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            joins.push(std::thread::spawn(move || {
                start.wait();
                writes
                    .transaction("project", || {
                        let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(current, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(20));
                        active.fetch_sub(1, Ordering::SeqCst);
                    })
                    .unwrap();
            }));
        }

        start.wait();
        for join in joins {
            join.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }
}
