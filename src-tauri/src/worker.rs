//! Background workers: EPS sessions another EPS session started.
//!
//! A parent EPS session hands a self-contained goal to a worker with
//! `worker_start`. The worker is an ordinary EPS session (the parent's provider
//! and model, `SessionMeta::worker_parent` set) that runs one interactive turn
//! on the goal in the background, with its own write registration, so its
//! edits are committed by owned-path commits like any other turn. The durable
//! [`WorkerRecord`] lives on the parent's session record; the parent is
//! continued automatically when a worker settles and reads the result through
//! the `[workers]` prompt note, `worker_status`, and `worker_revert`.

use serde::{Deserialize, Serialize};

/// Workers one parent may have queued or running at the same time.
pub const MAX_ACTIVE_WORKERS: usize = 3;

/// Characters of the worker's final answer kept as its summary.
pub const WORKER_SUMMARY_LIMIT: usize = 4000;

/// Workers listed in the `[workers]` prompt note, newest first.
pub const PROMPT_WORKER_LIMIT: usize = 8;

/// The fixed user message a parent continues with after a worker settled.
pub const WORKER_CONTINUE_TEXT: &str =
    "작업자 결과를 확인했습니다. [workers] 상태를 기준으로 이어서 진행해 주세요.";

/// The engine section every turn of a worker session carries (the user text
/// is only the goal, or what the user later types into the worker's pane).
pub const WORKER_PROMPT: &str = "[background worker]\nYou are a background worker another EPS session of this project (your parent) started. You see none of the parent's conversation: the goal (your first message) is your whole task. Complete exactly that goal and nothing beyond it. You cannot ask the user questions, propose plans, start workers, or hand map placement to the Map Agent; decide reasonable details yourself and state every assumption in your summary. Other sessions may be changing the project at the same time, so change only the files and area the goal gives you. After changing sources, plugins, or Python dependencies, run build_run and repair every reported error. End with a concise summary of what you changed (files, behavior, build result); the parent reads it.";

/// The subject of every commit a worker's turn makes.
pub fn worker_commit_summary(title: &str) -> String {
    format!("[작업자] {title}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WorkerStatus {
    /// Persisted before the turn is submitted.
    Queued,
    /// The worker session is running its turn.
    Running,
    /// The turn ended with an answer.
    Settled,
    Failed {
        reason: String,
    },
    /// Stopped by `worker_cancel`, the worker session's own stop, or the
    /// parent's cancel.
    Cancelled,
    /// The app restarted while the worker was queued or running.
    Interrupted,
}

impl WorkerStatus {
    /// Still counts against the parent's limit and blocks a revert.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    /// A settlement the parent conversation picks up on its own. A cancelled
    /// worker was stopped deliberately and never continues the parent.
    pub fn continues_interactively(&self) -> bool {
        matches!(self, Self::Settled | Self::Failed { .. })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Settled => "settled",
            Self::Failed { .. } => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRecord {
    pub id: String,
    pub child_session_id: String,
    /// The parent request whose `worker_start` created the worker.
    pub parent_request_id: String,
    pub title: String,
    pub goal: String,
    pub status: WorkerStatus,
    /// The worker's final answer, bounded to [`WORKER_SUMMARY_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The commits the worker's turn made, oldest first.
    #[serde(default)]
    pub commits: Vec<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_ok: Option<bool>,
    /// The worker commits `worker_revert` already reverted, newest first. A
    /// worker is reverted at most once; a partial revert (a conflict) lists
    /// only the commits it undid.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reverted: Vec<String>,
    /// Whether a parent turn has already been shown this settlement.
    #[serde(default)]
    pub reported: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

impl WorkerRecord {
    pub fn new(
        child_session_id: String,
        parent_request_id: String,
        title: String,
        goal: String,
    ) -> Self {
        let now = crate::session::now_unix_millis();
        Self {
            id: format!("worker-{}", uuid::Uuid::new_v4()),
            child_session_id,
            parent_request_id,
            title,
            goal,
            status: WorkerStatus::Queued,
            summary: None,
            commits: Vec::new(),
            changed_files: Vec::new(),
            build_ok: None,
            reverted: Vec::new(),
            reported: false,
            created_at: now,
            updated_at: now,
        }
    }

    /// The payload `worker_start`, `worker_status`, and `worker_cancel` return.
    pub fn to_tool_value(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "workerId": self.id,
            "sessionId": self.child_session_id,
            "title": self.title,
            "goal": self.goal,
            "status": self.status.label(),
            "commits": self.commits,
            "changedFiles": self.changed_files,
        });
        if let WorkerStatus::Failed { reason } = &self.status {
            value["reason"] = serde_json::Value::String(reason.clone());
        }
        if let Some(summary) = &self.summary {
            value["summary"] = serde_json::Value::String(summary.clone());
        }
        if let Some(build_ok) = self.build_ok {
            value["buildOk"] = serde_json::Value::Bool(build_ok);
        }
        if !self.reverted.is_empty() {
            value["reverted"] = serde_json::json!(self.reverted);
        }
        value
    }

    /// One entry of the `[workers]` prompt note.
    pub fn prompt_line(&self) -> String {
        let mut line = format!(
            "- {} status={} title={}",
            self.id,
            self.status.label(),
            self.title
        );
        if let WorkerStatus::Failed { reason } = &self.status {
            line.push_str(&format!(" reason={reason}"));
        }
        if let Some(build_ok) = self.build_ok {
            line.push_str(if build_ok {
                " build=ok"
            } else {
                " build=failed"
            });
        }
        if !self.commits.is_empty() {
            let short = self
                .commits
                .iter()
                .map(|sha| sha.chars().take(10).collect::<String>())
                .collect::<Vec<_>>();
            line.push_str(&format!(" commits={}", short.join(",")));
        }
        if !self.changed_files.is_empty() {
            line.push_str(&format!(" files={}", self.changed_files.join(",")));
        }
        if !self.reverted.is_empty() {
            line.push_str(&format!(" reverted={}", self.reverted.len()));
        }
        if let Some(summary) = &self.summary {
            line.push_str(&format!("\n  summary: {}", summary.replace('\n', "\n  ")));
        }
        line
    }
}

/// What one worker turn produced, read from the worker's engine after it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerTurnOutcome {
    pub answer: String,
    pub cancelled: bool,
    pub commits: Vec<String>,
    pub changed_files: Vec<String>,
    pub build_ok: Option<bool>,
    /// Why the turn failed after it began; what it committed is still kept.
    pub error: Option<String>,
}

/// Record the end of a worker's turn. A worker already marked cancelled stays
/// cancelled, and a failed turn is failed, but either keeps whatever the turn
/// committed so it can be reverted. `Err` is a turn that never began.
pub fn settle(worker: &mut WorkerRecord, outcome: Result<WorkerTurnOutcome, String>) {
    match outcome {
        Ok(outcome) => {
            let summary = outcome.answer.trim();
            if !summary.is_empty() {
                worker.summary = Some(bounded(summary, WORKER_SUMMARY_LIMIT));
            }
            worker.commits = outcome.commits;
            worker.changed_files = outcome.changed_files;
            worker.build_ok = outcome.build_ok;
            if worker.status != WorkerStatus::Cancelled {
                worker.status = match outcome.error {
                    Some(reason) => WorkerStatus::Failed { reason },
                    None if outcome.cancelled => WorkerStatus::Cancelled,
                    None => WorkerStatus::Settled,
                };
            }
        }
        Err(reason) => {
            if worker.status != WorkerStatus::Cancelled {
                worker.status = WorkerStatus::Failed { reason };
            }
        }
    }
    worker.reported = false;
}

fn bounded(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut kept = text.chars().take(limit - 1).collect::<String>();
    kept.push('…');
    kept
}

/// The usage error `worker_start` returns at the active-worker limit.
pub fn limit_refusal(active: usize) -> String {
    format!(
        "worker_limit: this session already has {active} queued or running workers (at most {MAX_ACTIVE_WORKERS}); wait for one to settle, which continues this conversation, or cancel one with worker_cancel"
    )
}

/// A fresh EPS session for one worker: the parent's provider and model with an
/// empty conversation, named after the worker's title.
pub fn child_session(
    parent: &crate::session::SessionRecord,
    title: &str,
) -> crate::session::SessionRecord {
    let mut provider_binding = parent.provider_binding.clone();
    provider_binding.conversation =
        crate::provider::ProviderConversationState::empty(provider_binding.provider);
    crate::session::SessionRecord {
        meta: crate::session::SessionMeta {
            id: crate::session::new_session_id(),
            name: title.to_string(),
            project: parent.meta.project.clone(),
            kind: crate::session::SessionKind::Eps,
            provider: provider_binding.provider,
            model: provider_binding.model.clone(),
            created_at: crate::session::now_unix_seconds(),
            last_conversation_at: crate::session::now_unix_millis(),
            team_parent: None,
            worker_parent: Some(parent.meta.id.clone()),
        },
        provider_binding,
        pending_request_ids: Vec::new(),
        context_usage: None,
        panel_log: serde_json::Value::Null,
        context_state: Default::default(),
        task_state: Default::default(),
        autonomous_run: None,
        team_tasks: Vec::new(),
        workers: Vec::new(),
    }
}

/// The worker whose settlement the parent should be continued with: settled
/// or failed and not yet shown to a parent turn. Never while the parent has a
/// live autonomous run: a run cannot start while workers are active, and a
/// settlement left from before it is shown by the next interactive turn.
pub fn next_continuation(parent: &crate::session::SessionRecord) -> Option<String> {
    if parent
        .autonomous_run
        .as_ref()
        .is_some_and(|run| !run.status.is_terminal())
    {
        return None;
    }
    parent
        .workers
        .iter()
        .find(|worker| worker.status.continues_interactively() && !worker.reported)
        .map(|worker| worker.id.clone())
}

/// The worker to continue `parent` with once one of its commands ended.
/// Nothing after a turn the user cancelled: a Stop must stop, not start the
/// next continuation. The settlement then waits for the next command the user
/// starts, or for another worker's settlement.
pub fn continuation_after_command(
    parent: &crate::session::SessionRecord,
    turn_cancelled: bool,
) -> Option<String> {
    if turn_cancelled {
        return None;
    }
    next_continuation(parent)
}

/// Whether a worker's turn may still start once it holds its session's
/// engine: its parent must still record it as running. A worker cancelled
/// (by its own stop or its parent's cancel) or gone while the turn waited for
/// the engine runs no turn at all.
pub fn worker_turn_may_start(
    parent: Option<&crate::session::SessionRecord>,
    worker_id: &str,
) -> bool {
    parent.is_some_and(|parent| {
        parent
            .workers
            .iter()
            .any(|worker| worker.id == worker_id && worker.status == WorkerStatus::Running)
    })
}

/// The worker a continuation turn of `parent` starts on: the announced
/// `worker_id` when it still waits to be reported, else any other unreported
/// settlement. `None` while the parent is busy (its next turn reports the
/// settlement) or has a live autonomous run.
pub fn continuation_target(
    parent: &crate::session::SessionRecord,
    worker_id: &str,
    parent_idle: bool,
) -> Option<WorkerRecord> {
    if !parent_idle {
        return None;
    }
    let next = next_continuation(parent)?;
    let announced_waits = parent.workers.iter().any(|worker| {
        worker.id == worker_id && worker.status.continues_interactively() && !worker.reported
    });
    let id = if announced_waits { worker_id } else { &next };
    parent
        .workers
        .iter()
        .find(|worker| worker.id == id)
        .cloned()
}

/// Render the `[workers]` prompt note. `None` when there are no workers.
pub fn prompt_note(workers: &[WorkerRecord]) -> Option<String> {
    if workers.is_empty() {
        return None;
    }
    let mut ordered = workers.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(b.id.cmp(&a.id)));
    let lines = ordered
        .into_iter()
        .take(PROMPT_WORKER_LIMIT)
        .map(WorkerRecord::prompt_line)
        .collect::<Vec<_>>()
        .join("\n");
    let guidance = if workers.iter().any(|worker| worker.status.is_active()) {
        "Queued or running workers continue this conversation on their own when they settle: do not poll worker_status or wait for them, and do not edit the files they own meanwhile. Review settled workers' changes with worker_status before building on them, and revert a wrong result with worker_revert."
    } else {
        "Review settled workers' changes with worker_status before building on them, and revert a wrong result with worker_revert. Failed, cancelled, or interrupted workers may have committed part of their work."
    };
    Some(format!("[workers]\n{lines}\n{guidance}"))
}

/// The `[workers]` note for the parent turn about to start, with the ids of
/// the settlements it shows for the first time. The caller marks those
/// reported only once the turn succeeded, so a failed turn leaves them to
/// continue the parent again.
pub fn prompt_note_for_turn(
    sessions: &crate::session::SessionStore,
    parent_id: &str,
) -> Option<(String, Vec<String>)> {
    let record = sessions.load(parent_id).ok()?;
    let note = prompt_note(&record.workers)?;
    let shown = record
        .workers
        .iter()
        .filter(|worker| !worker.status.is_active() && !worker.reported)
        .map(|worker| worker.id.clone())
        .collect();
    Some((note, shown))
}

/// What reverting a worker's commits did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertOutcome {
    /// `(worker commit, its revert commit)`, newest worker commit first.
    pub reverted: Vec<(String, crate::git::CommitRecord)>,
    /// Why the revert stopped early, naming the commit that conflicted.
    pub error: Option<String>,
}

/// Revert a worker's commits newest first, each as its own revert commit.
/// Stops at the first commit that does not revert cleanly; the commits already
/// reverted stay reverted and are reported with the conflict.
pub fn revert_commits(root: &std::path::Path, commits: &[String]) -> RevertOutcome {
    let mut reverted = Vec::new();
    for sha in commits.iter().rev() {
        match crate::git::revert(root, sha) {
            Ok(record) => reverted.push((sha.clone(), record)),
            Err(error) => {
                let done = reverted
                    .iter()
                    .map(|(original, _)| original.as_str())
                    .collect::<Vec<_>>();
                let error = format!(
                    "worker_revert stopped at commit {sha}: {error} (already reverted: [{}])",
                    done.join(", ")
                );
                return RevertOutcome {
                    reverted,
                    error: Some(error),
                };
            }
        }
    }
    RevertOutcome {
        reverted,
        error: None,
    }
}

/// The project files a set of commits changed, sorted and deduplicated.
pub fn changed_files(root: &std::path::Path, commits: &[String]) -> Vec<String> {
    let mut files = std::collections::BTreeSet::new();
    for sha in commits {
        match crate::git::commit_paths_of(root, sha) {
            Ok(paths) => files.extend(paths),
            Err(error) => {
                eprintln!("eud-agent: files of worker commit {sha} could not be read: {error}")
            }
        }
    }
    files.into_iter().collect()
}

/// The `worker` core-to-panel event on the parent session.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerEvent {
    pub worker: WorkerRecord,
    /// The worker session, on the event that created it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<crate::session::SessionMeta>,
    /// The worker session's turn, so the panel records it as sent there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The parent turn the engine starts itself after a settlement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation: Option<crate::team::TeamTaskContinuation>,
}

impl WorkerEvent {
    pub fn status(worker: WorkerRecord) -> Self {
        Self {
            worker,
            session: None,
            client_turn_id: None,
            text: None,
            continuation: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(id: &str, status: WorkerStatus, updated_at: u64) -> WorkerRecord {
        let mut worker = WorkerRecord::new(
            format!("child-{id}"),
            "req".to_string(),
            format!("title {id}"),
            format!("goal {id}"),
        );
        worker.id = id.to_string();
        worker.status = status;
        worker.updated_at = updated_at;
        worker
    }

    #[test]
    fn only_a_settled_or_failed_worker_continues_the_parent() {
        assert!(WorkerStatus::Settled.continues_interactively());
        assert!(WorkerStatus::Failed { reason: "x".into() }.continues_interactively());
        for waiting in [
            WorkerStatus::Queued,
            WorkerStatus::Running,
            WorkerStatus::Cancelled,
            WorkerStatus::Interrupted,
        ] {
            assert!(!waiting.continues_interactively(), "{waiting:?}");
        }
        assert!(WorkerStatus::Queued.is_active());
        assert!(WorkerStatus::Running.is_active());
        assert!(!WorkerStatus::Settled.is_active());
        assert_eq!(
            serde_json::to_value(WorkerStatus::Failed {
                reason: "boom".into()
            })
            .unwrap(),
            serde_json::json!({"kind": "failed", "reason": "boom"})
        );
    }

    #[test]
    fn a_settled_turn_keeps_a_bounded_summary_and_its_commits() {
        let mut record = worker("w", WorkerStatus::Running, 1);
        record.reported = true;
        settle(
            &mut record,
            Ok(WorkerTurnOutcome {
                answer: format!("  {}  ", "가".repeat(WORKER_SUMMARY_LIMIT + 10)),
                cancelled: false,
                commits: vec!["abc".into()],
                changed_files: vec!["src/a.eps".into()],
                build_ok: Some(true),
                error: None,
            }),
        );
        assert_eq!(record.status, WorkerStatus::Settled);
        let summary = record.summary.as_deref().unwrap();
        assert_eq!(summary.chars().count(), WORKER_SUMMARY_LIMIT);
        assert!(summary.ends_with('…'));
        assert_eq!(record.commits, vec!["abc".to_string()]);
        assert!(!record.reported, "a new settlement is reported again");
        let value = record.to_tool_value();
        assert_eq!(value["status"], "settled");
        assert_eq!(value["buildOk"], true);
        assert_eq!(value["changedFiles"][0], "src/a.eps");
    }

    #[test]
    fn a_cancel_wins_over_the_turn_that_raced_it_but_keeps_its_commits() {
        let mut cancelled = worker("w", WorkerStatus::Cancelled, 1);
        settle(
            &mut cancelled,
            Ok(WorkerTurnOutcome {
                answer: "done".into(),
                commits: vec!["abc".into()],
                ..Default::default()
            }),
        );
        assert_eq!(cancelled.status, WorkerStatus::Cancelled);
        assert_eq!(cancelled.commits, vec!["abc".to_string()]);

        let mut stopped = worker("w", WorkerStatus::Running, 1);
        settle(
            &mut stopped,
            Ok(WorkerTurnOutcome {
                cancelled: true,
                ..Default::default()
            }),
        );
        assert_eq!(stopped.status, WorkerStatus::Cancelled);

        let mut failed = worker("w", WorkerStatus::Running, 1);
        settle(&mut failed, Err("provider down".into()));
        assert_eq!(
            failed.status,
            WorkerStatus::Failed {
                reason: "provider down".into()
            }
        );
    }

    #[test]
    fn next_continuation_skips_reported_cancelled_and_autonomous_parents() {
        let binding = crate::provider::ProviderBinding::new(
            crate::provider::ProviderId::Codex,
            "gpt-test".to_string(),
            None,
        )
        .unwrap();
        let mut parent = crate::session::SessionRecord {
            meta: crate::session::SessionMeta {
                id: "parent".into(),
                name: "parent".into(),
                project: "project".into(),
                kind: crate::session::SessionKind::Eps,
                provider: binding.provider,
                model: binding.model.clone(),
                created_at: 1,
                last_conversation_at: 1,
                team_parent: None,
                worker_parent: None,
            },
            provider_binding: binding,
            pending_request_ids: Vec::new(),
            context_usage: None,
            panel_log: serde_json::Value::Null,
            context_state: Default::default(),
            task_state: Default::default(),
            autonomous_run: None,
            team_tasks: Vec::new(),
            workers: vec![
                worker("cancelled", WorkerStatus::Cancelled, 1),
                worker("running", WorkerStatus::Running, 1),
            ],
        };
        assert_eq!(next_continuation(&parent), None);
        let mut reported = worker("reported", WorkerStatus::Settled, 2);
        reported.reported = true;
        parent.workers.push(reported);
        assert_eq!(next_continuation(&parent), None);
        parent
            .workers
            .push(worker("fresh", WorkerStatus::Settled, 3));
        assert_eq!(next_continuation(&parent).as_deref(), Some("fresh"));

        parent.autonomous_run = Some(crate::autonomous::AutonomousRunState::new(
            "goal".into(),
            "turn".into(),
            "req".into(),
            "project".into(),
            "rev".into(),
            Default::default(),
            1,
        ));
        assert_eq!(
            next_continuation(&parent),
            None,
            "a live autonomous run reads [workers] itself"
        );
    }

    #[test]
    fn a_continuation_starts_only_on_an_idle_parent_and_never_for_a_cancelled_worker() {
        let binding = crate::provider::ProviderBinding::new(
            crate::provider::ProviderId::Codex,
            "gpt-test".to_string(),
            None,
        )
        .unwrap();
        let mut parent = crate::session::SessionRecord {
            meta: crate::session::SessionMeta {
                id: "parent".into(),
                name: "parent".into(),
                project: "project".into(),
                kind: crate::session::SessionKind::Eps,
                provider: binding.provider,
                model: binding.model.clone(),
                created_at: 1,
                last_conversation_at: 1,
                team_parent: None,
                worker_parent: None,
            },
            provider_binding: binding,
            pending_request_ids: Vec::new(),
            context_usage: None,
            panel_log: serde_json::Value::Null,
            context_state: Default::default(),
            task_state: Default::default(),
            autonomous_run: None,
            team_tasks: Vec::new(),
            workers: vec![worker("cancelled", WorkerStatus::Cancelled, 1)],
        };
        assert!(continuation_target(&parent, "cancelled", true).is_none());

        parent
            .workers
            .push(worker("settled", WorkerStatus::Settled, 2));
        assert!(
            continuation_target(&parent, "settled", false).is_none(),
            "a busy parent reports the settlement after its turn"
        );
        assert_eq!(
            continuation_target(&parent, "settled", true).map(|worker| worker.id),
            Some("settled".to_string())
        );
        // The announced worker was already reported by a turn in between;
        // another settlement still waiting is continued instead.
        parent.workers[1].reported = true;
        assert!(continuation_target(&parent, "settled", true).is_none());
        parent.workers.push(worker(
            "later",
            WorkerStatus::Failed { reason: "x".into() },
            3,
        ));
        assert_eq!(
            continuation_target(&parent, "settled", true).map(|worker| worker.id),
            Some("later".to_string())
        );
    }

    fn parent_record(workers: Vec<WorkerRecord>) -> crate::session::SessionRecord {
        let binding = crate::provider::ProviderBinding::new(
            crate::provider::ProviderId::Codex,
            "gpt-test".to_string(),
            None,
        )
        .unwrap();
        crate::session::SessionRecord {
            meta: crate::session::SessionMeta {
                id: "parent".into(),
                name: "parent".into(),
                project: "project".into(),
                kind: crate::session::SessionKind::Eps,
                provider: binding.provider,
                model: binding.model.clone(),
                created_at: 1,
                last_conversation_at: 1,
                team_parent: None,
                worker_parent: None,
            },
            provider_binding: binding,
            pending_request_ids: Vec::new(),
            context_usage: None,
            panel_log: serde_json::Value::Null,
            context_state: Default::default(),
            task_state: Default::default(),
            autonomous_run: None,
            team_tasks: Vec::new(),
            workers,
        }
    }

    #[test]
    fn a_worker_turn_starts_only_while_its_parent_still_records_it_running() {
        let parent = parent_record(vec![
            worker("running", WorkerStatus::Running, 1),
            worker("stopped", WorkerStatus::Cancelled, 1),
            worker("queued", WorkerStatus::Queued, 1),
        ]);
        assert!(worker_turn_may_start(Some(&parent), "running"));
        assert!(
            !worker_turn_may_start(Some(&parent), "stopped"),
            "a stop that reached the engine first wins"
        );
        assert!(!worker_turn_may_start(Some(&parent), "queued"));
        assert!(!worker_turn_may_start(Some(&parent), "missing"));
        assert!(!worker_turn_may_start(None, "running"));
    }

    #[test]
    fn nothing_continues_the_parent_after_a_cancelled_command() {
        let parent = parent_record(vec![worker("done", WorkerStatus::Settled, 1)]);
        assert_eq!(
            continuation_after_command(&parent, false).as_deref(),
            Some("done")
        );
        assert_eq!(continuation_after_command(&parent, true), None);
    }

    #[test]
    fn a_child_session_uses_the_parent_binding_with_a_fresh_conversation() {
        let mut binding = crate::provider::ProviderBinding::new(
            crate::provider::ProviderId::Codex,
            "gpt-test".to_string(),
            None,
        )
        .unwrap();
        binding.conversation = crate::provider::ProviderConversationState::Codex {
            thread_id: Some("parent-thread".into()),
        };
        let parent = crate::session::SessionRecord {
            meta: crate::session::SessionMeta {
                id: "parent".into(),
                name: "parent".into(),
                project: "project".into(),
                kind: crate::session::SessionKind::Eps,
                provider: binding.provider,
                model: binding.model.clone(),
                created_at: 1,
                last_conversation_at: 1,
                team_parent: None,
                worker_parent: None,
            },
            provider_binding: binding,
            pending_request_ids: Vec::new(),
            context_usage: None,
            panel_log: serde_json::Value::Null,
            context_state: Default::default(),
            task_state: Default::default(),
            autonomous_run: None,
            team_tasks: Vec::new(),
            workers: Vec::new(),
        };
        let child = child_session(&parent, "UI 정리");
        assert_eq!(child.meta.worker_parent.as_deref(), Some("parent"));
        assert_eq!(child.meta.kind, crate::session::SessionKind::Eps);
        assert_eq!(child.meta.name, "UI 정리");
        assert_eq!(child.meta.project, "project");
        assert_eq!(
            child.provider_binding.provider,
            parent.provider_binding.provider
        );
        assert_eq!(child.provider_binding.model, "gpt-test");
        assert_eq!(
            child.provider_binding.conversation,
            crate::provider::ProviderConversationState::empty(child.provider_binding.provider)
        );
        assert_ne!(child.meta.id, parent.meta.id);
    }

    #[test]
    fn prompt_note_lists_newest_first_with_summary_and_guidance() {
        assert!(prompt_note(&[]).is_none());
        let mut settled = worker("w1", WorkerStatus::Settled, 5);
        settled.summary = Some("added\nthe menu".into());
        settled.commits = vec!["0123456789abcdef".into()];
        settled.build_ok = Some(true);
        let note = prompt_note(&[worker("w0", WorkerStatus::Running, 1), settled]).unwrap();
        assert!(note.starts_with(
            "[workers]\n- w1 status=settled title=title w1 build=ok commits=0123456789"
        ));
        assert!(note.contains("summary: added\n  the menu"));
        assert!(note.contains("do not poll worker_status"));
        let done =
            prompt_note(&[worker("w2", WorkerStatus::Failed { reason: "x".into() }, 1)]).unwrap();
        assert!(done.contains("reason=x"));
        assert!(done.contains("worker_revert"));
        assert!(!done.contains("do not poll"));
    }

    #[test]
    fn the_worker_section_states_the_worker_rules() {
        assert!(WORKER_PROMPT.starts_with("[background worker]\n"));
        assert!(WORKER_PROMPT.contains("cannot ask the user"));
        assert!(WORKER_PROMPT.contains("build_run"));
        assert_eq!(worker_commit_summary("메뉴"), "[작업자] 메뉴");
    }

    #[test]
    fn the_worker_event_omits_what_it_does_not_carry() {
        let plain = serde_json::to_value(WorkerEvent::status(worker("w", WorkerStatus::Queued, 1)))
            .unwrap();
        assert_eq!(plain["worker"]["status"]["kind"], "queued");
        assert!(plain.get("session").is_none());
        assert!(plain.get("continuation").is_none());
        let mut continued = WorkerEvent::status(worker("w", WorkerStatus::Settled, 1));
        continued.continuation = Some(crate::team::TeamTaskContinuation {
            client_turn_id: "turn".into(),
            text: WORKER_CONTINUE_TEXT.into(),
        });
        let continued = serde_json::to_value(continued).unwrap();
        assert_eq!(continued["continuation"]["clientTurnId"], "turn");
    }

    fn repo(tag: &str) -> Option<std::path::PathBuf> {
        if !crate::git::available() {
            eprintln!("skipping {tag}: git is not installed on this machine");
            return None;
        }
        let root =
            std::env::temp_dir().join(format!("eud-agent-worker-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.eps"), "const a = 1;").unwrap();
        std::fs::write(root.join("src/b.eps"), "const b = 1;").unwrap();
        assert!(crate::git::prepare(&root).auto_commit_allowed());
        Some(root)
    }

    fn commit(root: &std::path::Path, file: &str, body: &str, subject: &str) -> String {
        std::fs::write(root.join(file), body).unwrap();
        crate::git::commit_turn(root, &crate::git::turn_message(subject, "child", "req"))
            .unwrap()
            .unwrap()
            .sha
    }

    #[test]
    fn revert_commits_undoes_a_worker_newest_first() {
        let Some(root) = repo("revert") else {
            return;
        };
        let first = commit(&root, "src/a.eps", "const a = 2;", "[작업자] a");
        let second = commit(&root, "src/b.eps", "const b = 2;", "[작업자] b");
        assert_eq!(
            changed_files(&root, &[first.clone(), second.clone()]),
            vec!["src/a.eps".to_string(), "src/b.eps".to_string()]
        );
        let outcome = revert_commits(&root, &[first, second.clone()]);
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.reverted.len(), 2);
        assert_eq!(outcome.reverted[0].0, second);
        assert!(
            outcome.reverted[0].1.subject.contains("[작업자] b"),
            "{:?}",
            outcome.reverted
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/a.eps")).unwrap(),
            "const a = 1;"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/b.eps")).unwrap(),
            "const b = 1;"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn revert_commits_stops_at_the_first_conflict() {
        let Some(root) = repo("conflict") else {
            return;
        };
        let first = commit(&root, "src/a.eps", "const a = 2;", "[작업자] a");
        let second = commit(&root, "src/b.eps", "const b = 2;", "[작업자] b");
        // A later change on top of the worker's first commit.
        commit(&root, "src/a.eps", "const a = 3;", "사용자 변경");
        let outcome = revert_commits(&root, &[first.clone(), second.clone()]);
        assert_eq!(outcome.reverted.len(), 1);
        assert_eq!(outcome.reverted[0].0, second);
        let error = outcome.error.unwrap();
        assert!(
            error.contains(&format!("stopped at commit {first}")),
            "{error}"
        );
        assert!(
            error.contains(&second),
            "the reverted newer commit is named: {error}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/b.eps")).unwrap(),
            "const b = 1;"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/a.eps")).unwrap(),
            "const a = 3;"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
