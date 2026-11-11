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
    instruction_writes: Arc<Mutex<InstructionWrites>>,
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

/// What each agent-instruction path of a git scope settled on, newest
/// first wins: the content an allowed `fs_write`/`fs_edit` left, what a
/// turn-end sweep put back (a deletion is a tombstone), or what a history
/// revert left. A sweep whose baseline is older than an entry keeps that
/// entry, and restores it over any later unapproved change, instead of its
/// own baseline, so overlapping turns of two sessions never undo each
/// other. `held` keeps, per scope, the paths a sweep left in place with
/// what they were then (`instruction_guard::held_paths`), and `loaded` the
/// scopes whose held paths were read back from the project's state file.
/// `guards` keeps each running turn's baseline sequence (`since`) per scope:
/// an entry no running guard can still ask for (its sequence is at or below
/// every running guard's `since`) is dropped, and a scope with no running
/// guard keeps nothing once its last guard ends. `closing` keeps the scopes
/// whose project closed while a guard of theirs was still registered (a
/// cancelled turn's final sweep, a dropped command's sweep): their records
/// stay until that last guard ends, so its sweep still sees what the
/// project settled on. `closed` keeps every scope whose project closed until
/// a guard of it opens again, outliving the guards `closing` waits for: a
/// report that comes after the close goes to the project's next turn.
#[derive(Default)]
struct InstructionWrites {
    seq: u64,
    by_scope: HashMap<String, HashMap<String, (u64, crate::instruction_guard::Settled)>>,
    held: HashMap<String, BTreeMap<String, (String, String)>>,
    loaded: std::collections::HashSet<String>,
    guards: HashMap<String, BTreeMap<u64, u64>>,
    closing: std::collections::HashSet<String>,
    closed: std::collections::HashSet<String>,
    next_guard: u64,
}

impl InstructionWrites {
    /// Forget everything kept for `scope`.
    fn forget_scope(&mut self, scope: &str) {
        self.by_scope.remove(scope);
        self.held.remove(scope);
        self.loaded.remove(scope);
        self.closing.remove(scope);
    }

    /// Drop the settled entries of `scope` that no running guard reads.
    fn prune(&mut self, scope: &str) {
        let Some(oldest) = self
            .guards
            .get(scope)
            .and_then(|guards| guards.values().min().copied())
        else {
            return;
        };
        if let Some(entries) = self.by_scope.get_mut(scope) {
            entries.retain(|_, (seq, _)| *seq > oldest);
        }
    }
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
            instruction_writes: Arc::new(Mutex::new(InstructionWrites::default())),
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

    /// End every registration and path claim held under `keys` (a closing
    /// project's `eps:` and `map:` keys). Returns how many registrations
    /// ended.
    pub fn release_project(&self, keys: &[String]) -> usize {
        let ended = {
            let mut active = self.active.lock();
            let ending = active
                .iter()
                .filter(|(_, entry)| keys.contains(&entry.project_id))
                .map(|(request_id, _)| request_id.clone())
                .collect::<Vec<_>>();
            ending
                .into_iter()
                .filter_map(|request_id| active.remove(&request_id))
                .collect::<Vec<_>>()
        };
        self.claims
            .lock()
            .retain(|_, claim| !keys.contains(&claim.project_id));
        // What the project's instruction paths settled on, and the paths
        // held out of its commits, end with it; the held paths are read back
        // from the project's state file when it opens again. A guard still
        // registered (a sweep still to come) keeps them until it ends.
        {
            let mut writes = self.instruction_writes.lock();
            for key in keys {
                writes.closed.insert(key.clone());
                if writes
                    .guards
                    .get(key)
                    .is_some_and(|guards| !guards.is_empty())
                {
                    writes.closing.insert(key.clone());
                } else {
                    writes.forget_scope(key);
                }
            }
        }
        for entry in &ended {
            entry.state.send_replace(TicketState::Cancelled);
            self.emit_activity(entry.session_id.clone(), SessionActivity::Idle);
        }
        ended.len()
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

    /// The sequence number of the newest settled instruction record.
    #[cfg(test)]
    pub fn instruction_write_seq(&self) -> u64 {
        self.instruction_writes.lock().seq
    }

    /// Register a turn's instruction baseline for `scope`: its id, and the
    /// sequence it later asks for the records after. Taken in one step, so
    /// no record after `since` is pruned before the guard is counted.
    pub(crate) fn open_instruction_guard(&self, scope: &str) -> (u64, u64) {
        let mut writes = self.instruction_writes.lock();
        // The project is open again: what it kept while closing is current.
        writes.closing.remove(scope);
        writes.closed.remove(scope);
        writes.next_guard += 1;
        let (id, since) = (writes.next_guard, writes.seq);
        writes
            .guards
            .entry(scope.to_string())
            .or_default()
            .insert(id, since);
        (id, since)
    }

    /// The baseline `id` of `scope` ended (its last sweep is done). With no
    /// guard of the scope left, its settled records are dropped, and a scope
    /// whose project closed meanwhile forgets its held paths too.
    pub(crate) fn close_instruction_guard(&self, scope: &str, id: u64) {
        let mut writes = self.instruction_writes.lock();
        let empty = writes.guards.get_mut(scope).is_some_and(|guards| {
            guards.remove(&id);
            guards.is_empty()
        });
        if empty {
            writes.guards.remove(scope);
            writes.by_scope.remove(scope);
            if writes.closing.contains(scope) {
                writes.forget_scope(scope);
            }
        } else {
            writes.prune(scope);
        }
    }

    /// Whether the project of `scope` closed while a guard of it is still
    /// registered: a report of that guard's sweep has no window to go to.
    pub(crate) fn instruction_scope_closing(&self, scope: &str) -> bool {
        self.instruction_writes.lock().closing.contains(scope)
    }

    /// Whether the project of `scope` closed and no guard of it opened
    /// since: a report about it has no window to go to, even after its last
    /// guard ended.
    pub(crate) fn instruction_scope_closed(&self, scope: &str) -> bool {
        let writes = self.instruction_writes.lock();
        writes.closing.contains(scope) || writes.closed.contains(scope)
    }

    /// Whether a guard of `scope` other than `own` is running (or still
    /// sweeping after its command was dropped).
    pub(crate) fn other_instruction_guards(&self, scope: &str, own: Option<u64>) -> bool {
        self.instruction_writes
            .lock()
            .guards
            .get(scope)
            .is_some_and(|guards| guards.keys().any(|id| Some(*id) != own))
    }

    /// `true` the first time it is asked for `scope` (since the process
    /// started or the project last closed): the caller then reads the held
    /// paths back from the project.
    pub(crate) fn first_held_load(&self, scope: &str) -> bool {
        self.instruction_writes
            .lock()
            .loaded
            .insert(scope.to_string())
    }

    /// Record the content an allowed `fs_write`/`fs_edit` left in the
    /// agent-instruction file `relative` of `scope`. Called inside the
    /// scope's transaction, right after the write succeeded.
    pub(crate) fn record_instruction_write(
        &self,
        scope: &str,
        relative: &str,
        content: crate::instruction_guard::ApprovedContent,
    ) {
        let mut settled: crate::instruction_guard::Settled = content.into();
        settled.relative = relative.to_string();
        self.record_instruction_settled(scope, vec![settled]);
    }

    /// Record what a sweep put back, or a history revert left, at each of
    /// `settled`'s paths of `scope`. Called inside the scope's transaction.
    pub(crate) fn record_instruction_settled(
        &self,
        scope: &str,
        settled: Vec<crate::instruction_guard::Settled>,
    ) {
        if settled.is_empty() {
            return;
        }
        let mut writes = self.instruction_writes.lock();
        for item in settled {
            writes.seq += 1;
            let seq = writes.seq;
            writes
                .by_scope
                .entry(scope.to_string())
                .or_default()
                .insert(crate::git::path_key(&item.relative), (seq, item));
        }
        writes.prune(scope);
    }

    /// What the instruction paths of `scope` settled on after `since`,
    /// keyed by `git::path_key`: whichever request or sweep settled them, in
    /// flight or done.
    pub(crate) fn instruction_writes_since(
        &self,
        scope: &str,
        since: u64,
    ) -> HashMap<String, crate::instruction_guard::Settled> {
        self.instruction_writes
            .lock()
            .by_scope
            .get(scope)
            .map(|writes| {
                writes
                    .iter()
                    .filter(|(_, (seq, _))| *seq > since)
                    .map(|(key, (_, content))| (key.clone(), content.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Hold `paths` (each with what it was when held) of `scope` out of
    /// every commit until they change.
    pub(crate) fn hold_instruction_paths(&self, scope: &str, paths: Vec<(String, String)>) {
        if paths.is_empty() {
            return;
        }
        let mut writes = self.instruction_writes.lock();
        let held = writes.held.entry(scope.to_string()).or_default();
        for (relative, fingerprint) in paths {
            held.insert(crate::git::path_key(&relative), (relative, fingerprint));
        }
    }

    /// The held paths of `scope`, each with what it was when held.
    pub(crate) fn held_instruction_paths(&self, scope: &str) -> Vec<(String, String)> {
        self.instruction_writes
            .lock()
            .held
            .get(scope)
            .map(|held| held.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Stop holding `paths` of `scope`.
    pub(crate) fn release_instruction_paths(&self, scope: &str, paths: &[String]) {
        if paths.is_empty() {
            return;
        }
        let mut writes = self.instruction_writes.lock();
        if let Some(held) = writes.held.get_mut(scope) {
            for path in paths {
                held.remove(&crate::git::path_key(path));
            }
        }
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

    fn settled_file(relative: &str) -> crate::instruction_guard::Settled {
        crate::instruction_guard::ApprovedContent {
            relative: relative.to_string(),
            sha256: "0".repeat(64),
            bytes: Some(Vec::new()),
        }
        .into()
    }

    fn settled_keys(writes: &ProjectWriteCoordinator, scope: &str) -> BTreeSet<String> {
        writes
            .instruction_writes_since(scope, 0)
            .into_keys()
            .collect()
    }

    #[test]
    fn settled_instruction_records_live_only_as_long_as_a_guard_can_read_them() {
        let writes = ProjectWriteCoordinator::silent();
        let (first, first_since) = writes.open_instruction_guard("eps:p");
        writes.record_instruction_settled("eps:p", vec![settled_file("AGENTS.md")]);
        let (second, second_since) = writes.open_instruction_guard("eps:p");
        assert!(second_since > first_since);
        // The second guard's sweep deletes CLAUDE.md: a tombstone.
        writes.record_instruction_settled(
            "eps:p",
            vec![crate::instruction_guard::Settled::absent("CLAUDE.md")],
        );
        writes.record_instruction_settled("eps:other", vec![settled_file("GEMINI.md")]);
        assert_eq!(
            settled_keys(&writes, "eps:p"),
            BTreeSet::from([
                crate::git::path_key("AGENTS.md"),
                crate::git::path_key("CLAUDE.md")
            ])
        );
        assert!(writes.other_instruction_guards("eps:p", Some(first)));

        // Once the older guard ends, only what the newer one can still see
        // stays; the tombstone keeps it from recreating the file.
        writes.close_instruction_guard("eps:p", first);
        let left = writes.instruction_writes_since("eps:p", second_since);
        assert_eq!(left.len(), 1);
        assert!(left[&crate::git::path_key("CLAUDE.md")].is_absent());
        assert_eq!(settled_keys(&writes, "eps:p").len(), 1);
        assert!(!writes.other_instruction_guards("eps:p", Some(second)));

        // The last guard out leaves nothing behind; another scope keeps its
        // records until a guard of its own ends.
        writes.close_instruction_guard("eps:p", second);
        assert!(settled_keys(&writes, "eps:p").is_empty());
        assert_eq!(settled_keys(&writes, "eps:other").len(), 1);
    }

    #[test]
    fn a_closing_project_forgets_its_instruction_records_and_held_paths() {
        let writes = ProjectWriteCoordinator::silent();
        writes.record_instruction_settled("eps:p", vec![settled_file("AGENTS.md")]);
        writes.hold_instruction_paths("eps:p", vec![("CLAUDE.md".to_string(), "dir".to_string())]);
        writes.hold_instruction_paths("eps:q", vec![("CLAUDE.md".to_string(), "dir".to_string())]);
        assert!(writes.first_held_load("eps:p"));
        assert!(!writes.first_held_load("eps:p"));

        writes.release_project(&["eps:p".to_string(), "map:p".to_string()]);

        assert!(settled_keys(&writes, "eps:p").is_empty());
        assert!(writes.held_instruction_paths("eps:p").is_empty());
        assert_eq!(writes.held_instruction_paths("eps:q").len(), 1);
        // Opened again, it reads its held paths back from the project.
        assert!(writes.first_held_load("eps:p"));
    }

    #[test]
    fn a_closed_project_stays_closed_for_reports_until_a_guard_opens_again() {
        let writes = ProjectWriteCoordinator::silent();
        let (guard, _) = writes.open_instruction_guard("eps:p");
        writes.release_project(&["eps:p".to_string()]);
        assert!(writes.instruction_scope_closing("eps:p"));
        assert!(writes.instruction_scope_closed("eps:p"));

        // Its last guard ends: nothing is kept for it, but a report that
        // comes now still has no window to go to.
        writes.close_instruction_guard("eps:p", guard);
        assert!(!writes.instruction_scope_closing("eps:p"));
        assert!(writes.instruction_scope_closed("eps:p"));
        assert!(!writes.instruction_scope_closed("eps:q"));

        // A project closed with no guard running is closed too.
        writes.release_project(&["eps:q".to_string()]);
        assert!(writes.instruction_scope_closed("eps:q"));

        // A turn of the project opened it again.
        let (again, _) = writes.open_instruction_guard("eps:p");
        assert!(!writes.instruction_scope_closed("eps:p"));
        writes.close_instruction_guard("eps:p", again);
        assert!(!writes.instruction_scope_closed("eps:p"));
    }
}
