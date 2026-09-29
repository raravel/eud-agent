//! Concurrent session write registration with short project transactions.
//!
//! Each session edits its isolated workspace without waiting for another session's
//! review. Only an operation that touches shared project state enters `transaction`;
//! the critical section ends as soon as that operation has atomically settled.

use parking_lot::Mutex;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::project_id::ProjectId;
use crate::session::SessionKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionActivity {
    Idle,
    RunningRead,
    WaitingInput,
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

/// The coordinator keys of one project, namespaced by what they serialize.
///
/// `eps:<id>` serializes EPS tool transactions, git mutations, path claims,
/// memory/wiki saves, and harness accepts; `map:<id>` serializes Map session
/// tool transactions with the Map window's apply, undo, and properties save.
/// The two NEVER merge: `map_task_apply` runs the Map apply (`map:`) inside
/// an EPS tool transaction (`eps:`), and one non-reentrant key for both would
/// deadlock it.
pub struct TxnKey;

impl TxnKey {
    pub fn eps(project: &ProjectId) -> String {
        format!("eps:{project}")
    }

    pub fn map(project: &ProjectId) -> String {
        format!("map:{project}")
    }

    /// Git mutations and path claims share the EPS key, exactly as they have
    /// always shared one key with EPS tool transactions.
    pub fn git(project: &ProjectId) -> String {
        Self::eps(project)
    }

    /// The transaction and registration key of a session of `kind`.
    pub fn for_session(kind: SessionKind, project: &ProjectId) -> String {
        match kind {
            SessionKind::Eps => Self::eps(project),
            SessionKind::Map => Self::map(project),
        }
    }
}

#[derive(Clone)]
pub struct ProjectWriteCoordinator {
    active: Arc<Mutex<HashMap<String, ActiveWrite>>>,
    transactions: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    claims: Arc<Mutex<HashMap<String, PathClaim>>>,
    activity: Arc<dyn Fn(SessionActivityEvent) + Send + Sync>,
    /// Every key a transaction entered, for tests that pin which lock a path
    /// takes.
    #[cfg(test)]
    entered: Arc<Mutex<Vec<String>>>,
}

/// The project files one in-flight request changed. A turn commit leaves
/// another request's files to that request's own commit.
struct PathClaim {
    project_id: String,
    session_id: String,
    paths: BTreeSet<String>,
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
            claims: Arc::new(Mutex::new(HashMap::new())),
            activity: Arc::new(activity),
            #[cfg(test)]
            entered: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The keys transactions entered so far, oldest first.
    #[cfg(test)]
    pub(crate) fn entered_transaction_keys(&self) -> Vec<String> {
        self.entered.lock().clone()
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
        self.claims.lock().remove(request_id);
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

    /// Record project-relative paths (`/`-separated) that `request_id` changed.
    ///
    /// Claims live in memory only and last as long as the request: they end
    /// with its registration (`release`), when its turn ends (`clear_claims`),
    /// or when its session opens its next request (`clear_session_claims`),
    /// whichever comes first. A request needs no write registration to claim,
    /// because a read-first turn still writes through `fs_*` and a provider's
    /// own edit tools.
    pub fn claim(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
        paths: impl IntoIterator<Item = String>,
    ) {
        let paths: Vec<String> = paths
            .into_iter()
            .filter_map(|path| normalize_claim_path(&path))
            .collect();
        if paths.is_empty() {
            return;
        }
        let mut claims = self.claims.lock();
        let claim = claims
            .entry(request_id.to_string())
            .or_insert_with(|| PathClaim {
                project_id: project_id.to_string(),
                session_id: session_id.to_string(),
                paths: BTreeSet::new(),
            });
        claim.paths.extend(paths);
    }

    /// Drop `paths` from `request_id`'s claims once its commit recorded them,
    /// so a later commit of another session is not held back by them.
    pub fn unclaim<'a>(&self, request_id: &str, paths: impl IntoIterator<Item = &'a String>) {
        let mut claims = self.claims.lock();
        let Some(claim) = claims.get_mut(request_id) else {
            return;
        };
        for path in paths {
            if let Some(path) = normalize_claim_path(path) {
                claim.paths.remove(&path);
            }
        }
        if claim.paths.is_empty() {
            claims.remove(request_id);
        }
    }

    /// End every claim of `request_id`.
    pub fn clear_claims(&self, request_id: &str) {
        self.claims.lock().remove(request_id);
    }

    /// End every claim `session_id` still holds. A session runs one request
    /// at a time, so opening its next request retires whatever the previous
    /// one left behind on an exit path that did not clear it.
    pub fn clear_session_claims(&self, session_id: &str) {
        self.claims
            .lock()
            .retain(|_, claim| claim.session_id != session_id);
    }

    /// Paths other in-flight requests of `project_id` claimed, each with the
    /// sessions that claimed it.
    pub fn claimed_by_others(
        &self,
        project_id: &str,
        request_id: &str,
    ) -> BTreeMap<String, BTreeSet<String>> {
        let mut claimed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (id, claim) in self.claims.lock().iter() {
            if id == request_id || claim.project_id != project_id {
                continue;
            }
            for path in &claim.paths {
                claimed
                    .entry(path.clone())
                    .or_default()
                    .insert(claim.session_id.clone());
            }
        }
        claimed
    }

    /// The paths `request_id` itself claimed.
    pub fn claimed_by(&self, request_id: &str) -> BTreeSet<String> {
        self.claims
            .lock()
            .get(request_id)
            .map(|claim| claim.paths.clone())
            .unwrap_or_default()
    }

    /// Every path any in-flight request of `project_id` claimed.
    pub fn claimed_in_flight(&self, project_id: &str) -> BTreeSet<String> {
        self.claims
            .lock()
            .values()
            .filter(|claim| claim.project_id == project_id)
            .flat_map(|claim| claim.paths.iter().cloned())
            .collect()
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
        #[cfg(test)]
        self.entered.lock().push(project_id.to_string());
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

/// A claimed path in the form `git status` reports it relative to the project
/// root: `/`-separated, no leading `./` or `/`, no empty or `.` components.
/// A path that climbs out of the root is not a project path and is dropped.
pub fn normalize_claim_path(path: &str) -> Option<String> {
    let path = path.trim().replace('\\', "/");
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            part => parts.push(part),
        }
    }
    (!parts.is_empty() && !path.starts_with('/') && !path.contains(':')).then(|| parts.join("/"))
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

    #[test]
    fn path_claims_are_per_request_and_end_with_the_registration() {
        let writes = ProjectWriteCoordinator::silent();
        writes.request("project", "session-a", "request-a").unwrap();
        writes.request("project", "session-b", "request-b").unwrap();
        writes.request("other", "session-c", "request-c").unwrap();

        writes.claim(
            "project",
            "session-a",
            "request-a",
            [
                "src/main.eps".to_string(),
                "./dat\\standard.json".to_string(),
                // Paths outside the root are not project paths.
                "../escape.eps".to_string(),
            ],
        );
        writes.claim(
            "project",
            "session-b",
            "request-b",
            ["src/main.eps".to_string(), "src/b.eps".to_string()],
        );
        writes.claim("other", "session-c", "request-c", ["src/c.eps".to_string()]);

        assert_eq!(
            writes.claimed_by("request-a"),
            BTreeSet::from(["dat/standard.json".to_string(), "src/main.eps".to_string()])
        );
        assert_eq!(
            writes.claimed_by_others("project", "request-a"),
            BTreeMap::from([
                (
                    "src/b.eps".to_string(),
                    BTreeSet::from(["session-b".to_string()])
                ),
                (
                    "src/main.eps".to_string(),
                    BTreeSet::from(["session-b".to_string()])
                ),
            ])
        );
        assert_eq!(
            writes.claimed_in_flight("project"),
            BTreeSet::from([
                "dat/standard.json".to_string(),
                "src/b.eps".to_string(),
                "src/main.eps".to_string(),
            ])
        );

        assert!(writes.release("request-b").unwrap());
        assert!(writes.claimed_by_others("project", "request-a").is_empty());
        assert_eq!(
            writes.claimed_in_flight("project"),
            BTreeSet::from(["dat/standard.json".to_string(), "src/main.eps".to_string()])
        );
        assert!(writes.release("request-a").unwrap());
        assert!(writes.claimed_in_flight("project").is_empty());
        assert_eq!(
            writes.claimed_in_flight("other"),
            BTreeSet::from(["src/c.eps".to_string()])
        );
    }

    #[test]
    fn a_request_without_a_registration_claims_until_its_turn_or_session_moves_on() {
        let writes = ProjectWriteCoordinator::silent();

        // A read-first turn writes with fs_* before it holds any registration.
        writes.claim(
            "project",
            "session-a",
            "request-a",
            ["notes/a.txt".to_string()],
        );
        writes.claim(
            "project",
            "session-b",
            "request-b",
            ["notes/b.txt".to_string()],
        );
        assert_eq!(
            writes.claimed_in_flight("project"),
            BTreeSet::from(["notes/a.txt".to_string(), "notes/b.txt".to_string()])
        );

        // The end of the turn ends its claims, registered or not.
        writes.clear_claims("request-a");
        assert_eq!(
            writes.claimed_in_flight("project"),
            BTreeSet::from(["notes/b.txt".to_string()])
        );

        // A request that left without clearing is retired by its session's next one.
        writes.clear_session_claims("session-b");
        assert!(writes.claimed_in_flight("project").is_empty());
    }

    #[test]
    fn committed_paths_are_no_longer_claimed() {
        let writes = ProjectWriteCoordinator::silent();
        writes.request("project", "session-a", "request-a").unwrap();
        writes.claim(
            "project",
            "session-a",
            "request-a",
            ["src/a.eps".to_string(), "src/b.eps".to_string()],
        );

        writes.unclaim("request-a", &["src/a.eps".to_string()]);
        assert_eq!(
            writes.claimed_by("request-a"),
            BTreeSet::from(["src/b.eps".to_string()])
        );
        writes.unclaim("request-a", &["src/b.eps".to_string()]);
        assert!(writes.claimed_in_flight("project").is_empty());
        // The registration itself is untouched.
        assert!(writes.owns("project", "session-a", "request-a"));
    }
}
