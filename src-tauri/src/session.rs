//! Named conversation sessions that survive a full app restart.
//!
//! Rust owns every session file (feature: session restore, decision A): a session
//! is auto-created on a conversation's first turn (no save button) and its log is
//! pushed by the panel via `session_update_log` after each turn; the panel never
//! touches the filesystem. Files live under
//! `%appdata%\eud-agent\sessions\` (Roaming, decision D — small, user-owned, preserved
//! by the self-update). Every file is written via [`crate::memory::write_atomic_bytes`]
//! (temp + rename, UTF-8 **without BOM**), the same write semantics as memory/wiki.
//!
//! `index.json` drives the list UI (ordered by the latest submitted user
//! conversation); one `<session-id>.json` record holds the full session.
//! The `panelLog` blob is opaque to Rust and stored/returned verbatim as a
//! [`serde_json::Value`]; its schema is owned
//! solely by the panel. A corrupt/missing `index.json` yields an empty list (never
//! crash startup); a corrupt `<id>.json` on open is a graceful `Err` surfaced to the
//! panel.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize};

use crate::config::DataDirs;
use crate::memory::write_atomic_bytes;

const SCHEMA_VERSION: u32 = 3;
const INDEX_FILE: &str = "index.json";
static SESSION_WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    #[default]
    Eps,
    Map,
}

/// Session list-entry metadata (drives the list UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub id: String,
    pub name: String,
    pub project: String,
    #[serde(default)]
    pub kind: SessionKind,
    pub created_at: u64,
    #[serde(
        alias = "updatedAt",
        deserialize_with = "deserialize_last_conversation_at"
    )]
    pub last_conversation_at: u64,
}

fn deserialize_last_conversation_at<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let timestamp = u64::deserialize(deserializer)?;
    Ok(if timestamp < 10_000_000_000 {
        timestamp.saturating_mul(1_000)
    } else {
        timestamp
    })
}

/// One full session record: [`SessionMeta`] (flattened) + the resumable thread id,
/// the pending changeset req-ids, and the panel-owned log blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    #[serde(flatten)]
    pub meta: SessionMeta,
    /// `null` until the first turn emits `ThreadStarted`.
    pub thread_id: Option<String>,
    /// Unarchived journal request ids. The concurrent writer contract restores
    /// exactly one pending project writer and reports conflicting records.
    pub pending_request_ids: Vec<String>,
    /// Last active context and cumulative token snapshot reported by Codex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<crate::ipc::ContextUsage>,
    /// Opaque to Rust — stored and returned verbatim; the panel owns its schema.
    pub panel_log: serde_json::Value,
    /// Durable instruction epoch and successful model-delivery cursor.
    #[serde(default)]
    pub context_state: crate::context_state::SessionContextState,
    /// Append-only active-task event graph and its derived current projection.
    #[serde(default)]
    pub task_state: crate::task_state::SessionTaskState,
}

/// The on-disk `index.json` shape: latest-conversation-first metadata rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionIndex {
    schema_version: u32,
    sessions: Vec<SessionMeta>,
}

impl Default for SessionIndex {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            sessions: Vec::new(),
        }
    }
}

/// Reads/writes named session records under `%appdata%\eud-agent\sessions\`.
#[derive(Debug, Clone)]
pub struct SessionStore {
    dirs: DataDirs,
    sessions_dir: PathBuf,
}

impl SessionStore {
    /// Construct the store and perform the one-time v3 clean cutover.
    ///
    /// Existing names and panel conversation logs survive. Codex threads, context
    /// snapshots, pending review ids, unaccepted journals, session workspaces, and
    /// interrupted harness jobs are reset so the post-acceptance harness state
    /// machine never inherits ambiguous legacy ownership.
    pub fn new(dirs: &DataDirs) -> Self {
        let store = Self {
            dirs: dirs.clone(),
            sessions_dir: dirs.sessions_dir(),
        };
        if let Err(error) = store.reset_legacy_sessions() {
            eprintln!("eud-agent: legacy session reset failed: {error}");
        }
        store
    }
    /// The session list, sorted by the latest user conversation. A missing or
    /// corrupt `index.json` yields `[]` (never crash startup).
    pub fn list(&self) -> anyhow::Result<Vec<SessionMeta>> {
        let _guard = self.lock()?;
        let mut sessions = self.read_index().sessions;
        sort_sessions(&mut sessions);
        Ok(sessions)
    }
    pub fn list_kind(&self, kind: SessionKind) -> anyhow::Result<Vec<SessionMeta>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|session| session.kind == kind)
            .collect())
    }

    /// Load one full record by id. A corrupt/missing record is a graceful `Err`.
    pub fn load(&self, id: &str) -> anyhow::Result<SessionRecord> {
        let _guard = self.lock()?;
        self.load_unlocked(id)
    }

    /// Write the `<id>.json` record and rewrite the latest-conversation-first
    /// `index.json`. Both via [`write_atomic_bytes`] (temp + rename, UTF-8 no BOM).
    ///
    /// Each write is individually atomic but the pair is not transactional: if the
    /// index rewrite fails after the record write succeeds, the record exists but is
    /// not listed (an invisible orphan). The shared store lock serializes the engine
    /// and panel-side session commands; the next successful save self-heals the index.
    pub fn save(&self, rec: &SessionRecord) -> anyhow::Result<()> {
        let _guard = self.lock()?;
        self.save_unlocked(rec)
    }

    /// Delete the record file and remove it from the index. A missing record file is
    /// tolerated (the index entry is still dropped).
    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let _guard = self.lock()?;
        let path = self.record_path(id);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        let mut index = self.read_index();
        index.sessions.retain(|meta| meta.id != id);
        self.write_index(&index)
    }

    /// Rename a session without changing its conversation recency.
    pub fn rename(&self, id: &str, name: &str) -> anyhow::Result<()> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        record.meta.name = name.to_string();
        self.save_unlocked(&record)
    }

    /// Replace one session's opaque panel log without changing its conversation
    /// recency. Multi-session tabs autosave independently while another turn runs.
    pub fn update_panel_log(&self, id: &str, panel_log: serde_json::Value) -> anyhow::Result<()> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        record.panel_log = panel_log;
        self.save_unlocked(&record)
    }

    /// Mark a newly submitted user conversation and return its Unix-millisecond
    /// timestamp. The timestamp advances past every indexed session so the touched
    /// row sorts first even when multiple submissions share one wall-clock tick.
    pub fn touch_conversation(&self, id: &str) -> anyhow::Result<u64> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        let indexed_next = self
            .read_index()
            .sessions
            .iter()
            .map(|meta| meta.last_conversation_at)
            .max()
            .unwrap_or_default()
            .saturating_add(1);
        let timestamp = now_unix_millis().max(indexed_next);
        record.meta.last_conversation_at = timestamp;
        self.save_unlocked(&record)?;
        Ok(timestamp)
    }

    /// Persist the latest Codex context snapshot without changing the panel-owned log.
    pub fn update_context_usage(
        &self,
        id: &str,
        context_usage: crate::ipc::ContextUsage,
    ) -> anyhow::Result<()> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        record.context_usage = Some(context_usage);
        self.save_unlocked(&record)
    }
    /// Update only primary runtime ownership fields under the latest-record lock.
    pub fn update_runtime_state(
        &self,
        id: &str,
        thread_id: Option<String>,
        pending_request_ids: Vec<String>,
    ) -> anyhow::Result<()> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        record.thread_id = thread_id;
        record.pending_request_ids = pending_request_ids;
        self.save_unlocked(&record)
    }

    pub fn drop_pending_request(&self, id: &str, request_id: &str) -> anyhow::Result<()> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        record
            .pending_request_ids
            .retain(|pending| pending != request_id);
        self.save_unlocked(&record)
    }

    /// Initialize defaulted context fields without resetting a resumable legacy thread.
    pub fn initialize_context_state(
        &self,
        id: &str,
        static_baseline: &str,
        memory: Option<&str>,
        wiki: Option<&str>,
    ) -> anyhow::Result<crate::context_state::SessionContextState> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        let before = record.context_state.clone();
        if record.context_state.static_prompt_fingerprint.is_empty() {
            if let Some(thread_id) = record.thread_id.clone() {
                record.context_state.adopt_legacy_thread(
                    static_baseline,
                    thread_id,
                    memory,
                    wiki,
                    record.task_state.projection.revision,
                );
            } else {
                record.context_state.initialize_baseline(static_baseline);
            }
        }
        if record.context_state != before {
            self.save_unlocked(&record)?;
        }
        Ok(record.context_state)
    }

    /// Start a new instruction epoch and optionally clear the provider thread.
    pub fn reset_context_epoch(
        &self,
        id: &str,
        static_baseline: &str,
        clear_thread: bool,
    ) -> anyhow::Result<u64> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        let epoch = record.context_state.reset_epoch(static_baseline);
        if clear_thread {
            record.thread_id = None;
            record.context_usage = None;
        }
        self.save_unlocked(&record)?;
        Ok(epoch)
    }

    /// Commit a successful model delivery iff the prepared epoch is still current.
    pub fn commit_context_delivery(
        &self,
        id: &str,
        expected_epoch: u64,
        cursor: crate::context_state::ModelContextCursor,
    ) -> anyhow::Result<()> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        if record.context_state.instruction_epoch != expected_epoch {
            anyhow::bail!("instruction epoch changed before delivery commit");
        }
        if cursor.epoch != expected_epoch {
            anyhow::bail!("delivery cursor epoch does not match expected epoch");
        }
        record.context_state.delivered = cursor;
        self.save_unlocked(&record)
    }

    /// Append one event against the exact current branch leaf.
    pub fn append_task_event(
        &self,
        id: &str,
        expected_leaf: Option<&str>,
        event: crate::task_state::TaskStateEvent,
    ) -> anyhow::Result<crate::task_state::SessionTaskState> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        record
            .task_state
            .append_event(expected_leaf, event)
            .map_err(anyhow::Error::msg)?;
        let state = record.task_state.clone();
        self.save_unlocked(&record)?;
        Ok(state)
    }

    /// Atomically rewind the panel log, task branch, and instruction delivery.
    pub fn move_task_leaf_for_rewind(
        &self,
        id: &str,
        panel_log: serde_json::Value,
    ) -> anyhow::Result<Option<String>> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        let client_turn_id = last_client_turn_id(&panel_log);
        record
            .task_state
            .move_leaf_to_client_turn(client_turn_id.as_deref())
            .map_err(anyhow::Error::msg)?;
        record.thread_id = None;
        record.pending_request_ids.clear();
        record.context_usage = None;
        record.panel_log = panel_log;
        record.context_state.instruction_epoch = record
            .context_state
            .instruction_epoch
            .max(1)
            .saturating_add(1);
        record.context_state.delivered = crate::context_state::ModelContextCursor::default();
        self.save_unlocked(&record)?;
        Ok(client_turn_id)
    }

    /// Reset delivery after compaction and retain an auditable branch boundary.
    pub fn record_compaction_boundary(
        &self,
        id: &str,
        static_baseline: &str,
    ) -> anyhow::Result<u64> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        let epoch = record.context_state.reset_epoch(static_baseline);
        let expected_leaf = record.task_state.leaf_id.clone();
        record
            .task_state
            .append_event(
                expected_leaf.as_deref(),
                crate::task_state::TaskStateEvent::new(
                    None,
                    None,
                    crate::task_state::TaskStateEventKind::CompactionBoundary {
                        instruction_epoch: epoch,
                    },
                ),
            )
            .map_err(anyhow::Error::msg)?;
        self.save_unlocked(&record)?;
        Ok(epoch)
    }

    /// Persist promotion audit independently; mutate the current projection only
    /// when the source event remains on the active branch.
    pub fn record_task_promotion(
        &self,
        id: &str,
        audit: crate::task_state::TaskStatePromotionAudit,
    ) -> anyhow::Result<bool> {
        let _guard = self.lock()?;
        let mut record = self.load_unlocked(id)?;
        let attached = record
            .task_state
            .is_current_ancestor(&audit.source_event_id);
        record.task_state.promotion_audits.push(audit.clone());
        if attached {
            let expected_leaf = record.task_state.leaf_id.clone();
            let kind = if audit.accepted {
                crate::task_state::TaskStateEventKind::PromotionAccepted {
                    harness_job_id: audit.harness_job_id.clone(),
                    fact_ids: audit.fact_ids.clone(),
                    document_refs: audit.document_refs.clone(),
                    memory_refs: audit.memory_refs.clone(),
                }
            } else {
                crate::task_state::TaskStateEventKind::PromotionRejected {
                    harness_job_id: audit.harness_job_id.clone(),
                }
            };
            record
                .task_state
                .append_event(
                    expected_leaf.as_deref(),
                    crate::task_state::TaskStateEvent::new(None, None, kind),
                )
                .map_err(anyhow::Error::msg)?;
        }
        self.save_unlocked(&record)?;
        Ok(attached)
    }

    fn lock(&self) -> anyhow::Result<MutexGuard<'static, ()>> {
        SESSION_WRITE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("session store lock poisoned"))
    }

    fn load_unlocked(&self, id: &str) -> anyhow::Result<SessionRecord> {
        let path = self.record_path(id);
        let bytes = std::fs::read(&path)
            .map_err(|err| anyhow::anyhow!("session '{id}' not found: {err}"))?;
        // `File.ReadAllText` strips a BOM; serde_json does not. Strip a UTF-8 BOM
        // defensively so a hand-edited file still parses.
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
        let mut record: SessionRecord = serde_json::from_slice(bytes)
            .map_err(|err| anyhow::anyhow!("session '{id}' is corrupt: {err}"))?;
        if let Err(error) = record.task_state.repair_cache() {
            eprintln!("eud-agent: session '{id}' task-state replay failed: {error}");
            let events = std::mem::take(&mut record.task_state.events);
            let audits = std::mem::take(&mut record.task_state.promotion_audits);
            record.task_state = crate::task_state::SessionTaskState::default();
            record.task_state.events = events;
            record.task_state.promotion_audits = audits;
            record.task_state.compilation_stale = true;
        }
        Ok(record)
    }

    fn save_unlocked(&self, rec: &SessionRecord) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(rec)?;
        write_atomic_bytes(&self.record_path(&rec.meta.id), &bytes)?;
        self.upsert_index(rec.meta.clone())
    }

    /// Read `index.json`, yielding [`SessionIndex::default`] on any missing/corrupt
    /// content (never crash startup).
    fn read_index(&self) -> SessionIndex {
        let path = self.sessions_dir.join(INDEX_FILE);
        let Ok(bytes) = std::fs::read(&path) else {
            return SessionIndex::default();
        };
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
        serde_json::from_slice(bytes).unwrap_or_default()
    }

    fn write_index(&self, index: &SessionIndex) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(index)?;
        write_atomic_bytes(&self.sessions_dir.join(INDEX_FILE), &bytes)?;
        Ok(())
    }

    /// Upsert `meta` and retain latest-conversation-first index ordering.
    fn upsert_index(&self, meta: SessionMeta) -> anyhow::Result<()> {
        let mut index = self.read_index();
        index.schema_version = SCHEMA_VERSION;
        index.sessions.retain(|existing| existing.id != meta.id);
        index.sessions.push(meta);
        sort_sessions(&mut index.sessions);
        self.write_index(&index)
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.dirs.sessions_dir().join(format!("{id}.json"))
    }

    fn reset_legacy_sessions(&self) -> anyhow::Result<()> {
        let _guard = self.lock()?;
        let mut index = self.read_index();
        if index.schema_version >= SCHEMA_VERSION {
            return Ok(());
        }

        for meta in &index.sessions {
            let Ok(mut record) = self.load_unlocked(&meta.id) else {
                continue;
            };
            record.thread_id = None;
            record.pending_request_ids.clear();
            record.context_usage = None;
            let bytes = serde_json::to_vec_pretty(&record)?;
            write_atomic_bytes(&self.record_path(&meta.id), &bytes)?;
        }

        clear_directory(&self.dirs.session_workspaces_dir(), None)?;
        clear_directory(&self.dirs.workspace_state_dir().join("baselines"), None)?;
        clear_directory(&self.dirs.journal_dir(), Some("accepted"))?;
        clear_directory(&self.dirs.harness_jobs_dir(), None)?;

        index.schema_version = SCHEMA_VERSION;
        self.write_index(&index)?;
        Ok(())
    }
}

fn last_client_turn_id(panel_log: &serde_json::Value) -> Option<String> {
    panel_log
        .get("log")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .rev()
        .find(|entry| entry.get("kind").and_then(serde_json::Value::as_str) == Some("you"))
        .and_then(|entry| entry.get("clientTurnId"))
        .and_then(serde_json::Value::as_str)
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .map(str::to_string)
}

fn clear_directory(path: &std::path::Path, preserve_name: Option<&str>) -> anyhow::Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
        return Ok(());
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if preserve_name.is_some_and(|name| entry.file_name() == std::ffi::OsStr::new(name)) {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn sort_sessions(sessions: &mut [SessionMeta]) {
    sessions.sort_by(|left, right| {
        right
            .last_conversation_at
            .cmp(&left.last_conversation_at)
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });
}

/// Mint a v4-style UUID string in Rust (decision: never panel-side / `Math.random`).
///
/// The project carries no `uuid` crate; this composes 128 bits from the wall clock,
/// the process id, and a monotonic counter, then formats them with the version-4 +
/// RFC-4122 variant nibbles set so the id is a well-formed v4 UUID string.
pub fn new_session_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id() as u64;

    let hi = nanos ^ (pid.rotate_left(32));
    let lo = counter.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17) ^ nanos.rotate_right(13);

    // version 4 (nibble 13) and RFC-4122 variant (nibble 17).
    let time_hi_and_version = ((hi >> 48) as u16 & 0x0FFF) | 0x4000;
    let clock_seq = ((lo >> 48) as u16 & 0x3FFF) | 0x8000;

    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (hi >> 32) as u32,
        (hi >> 16) as u16,
        time_hi_and_version,
        clock_seq,
        lo & 0x0000_FFFF_FFFF_FFFF,
    )
}

/// Current Unix time in seconds (0 if the clock is before the epoch).
pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// Current Unix time in milliseconds (0 if the clock is before the epoch).
pub fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{ContextUsage, TokenUsageBreakdown};
    use serde_json::json;
    use std::fs;
    use std::sync::Arc;

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-session-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store(tag: &str) -> (PathBuf, SessionStore) {
        let base = unique_temp_dir(tag);
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        (base, SessionStore::new(&dirs))
    }

    fn sample_record(id: &str, name: &str) -> SessionRecord {
        SessionRecord {
            meta: SessionMeta {
                id: id.to_string(),
                name: name.to_string(),
                project: "mymap".to_string(),
                kind: SessionKind::Eps,
                created_at: 1_718_000_000,
                last_conversation_at: 1_718_000_000_000,
            },
            thread_id: Some("019ece1c-thread".to_string()),
            pending_request_ids: vec!["req-1a2b3c4d".to_string()],
            context_usage: None,
            panel_log: json!({
                "schemaVersion": 1,
                "logSeq": 2,
                "log": [
                    { "id": 1, "kind": "you", "text": "유닛 HP 올려줘" },
                    { "id": 2, "kind": "agent", "text": "...markdown..." }
                ]
            }),
            context_state: Default::default(),
            task_state: Default::default(),
        }
    }

    fn sample_usage() -> ContextUsage {
        ContextUsage {
            last: TokenUsageBreakdown {
                input_tokens: 30,
                cached_input_tokens: 20,
                cache_write_input_tokens: 0,
                output_tokens: 5,
                reasoning_output_tokens: 2,
                total_tokens: 35,
            },
            total: TokenUsageBreakdown {
                input_tokens: 50,
                cached_input_tokens: 40,
                cache_write_input_tokens: 1,
                output_tokens: 8,
                reasoning_output_tokens: 3,
                total_tokens: 58,
            },
            model_context_window: Some(128_000),
        }
    }

    fn semantic_goal_event(
        turn_id: &str,
        request_id: &str,
        base_revision: u64,
        fact_id: &str,
    ) -> crate::task_state::TaskStateEvent {
        crate::task_state::TaskStateEvent::new(
            Some(turn_id.to_string()),
            Some(request_id.to_string()),
            crate::task_state::TaskStateEventKind::SemanticDelta {
                delta: crate::task_state::TaskStateDelta {
                    base_revision,
                    operations: vec![crate::task_state::TaskStateOperation::Upsert {
                        entity: crate::task_state::TaskStateEntity::Goal {
                            fact: crate::task_state::StateFact {
                                id: fact_id.to_string(),
                                status: crate::task_state::FactStatus::Active,
                                text: "goal".to_string(),
                                provenance: vec![crate::task_state::Provenance::UserTurn {
                                    client_turn_id: turn_id.to_string(),
                                    exact_quote: "goal".to_string(),
                                }],
                            },
                        },
                    }],
                },
            },
        )
    }

    #[test]
    fn save_list_load_rename_delete_round_trip() {
        let (base, store) = store("round-trip");

        // Empty store yields [].
        assert!(store.list().unwrap().is_empty());

        let first = sample_record(&new_session_id(), "유닛 HP 작업");
        store.save(&first).unwrap();

        // A second session with a newer conversation sorts first.
        let mut second = sample_record(&new_session_id(), "무기 데미지");
        second.meta.last_conversation_at = 1_718_009_999_000;
        store.save(&second).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, second.meta.id);
        assert_eq!(listed[1].id, first.meta.id);

        // Load returns the full record verbatim, including the opaque panelLog.
        let loaded = store.load(&first.meta.id).unwrap();
        assert_eq!(loaded, first);
        assert_eq!(loaded.panel_log["logSeq"], json!(2));

        // Rename updates only the name and leaves conversation order untouched.
        store.rename(&first.meta.id, "유닛 HP 완료").unwrap();
        let reloaded = store.load(&first.meta.id).unwrap();
        assert_eq!(reloaded.meta.name, "유닛 HP 완료");
        assert_eq!(
            reloaded.meta.last_conversation_at,
            first.meta.last_conversation_at
        );
        assert_eq!(store.list().unwrap()[0].id, second.meta.id);

        // Log autosave is also recency-neutral; a new user turn advances and promotes.
        store
            .update_panel_log(&first.meta.id, json!({ "schemaVersion": 2, "log": [] }))
            .unwrap();
        assert_eq!(store.list().unwrap()[0].id, second.meta.id);
        let touched = store.touch_conversation(&first.meta.id).unwrap();
        assert!(touched > second.meta.last_conversation_at);
        assert_eq!(store.list().unwrap()[0].id, first.meta.id);

        // Delete removes the record file AND the index entry.
        store.delete(&first.meta.id).unwrap();
        assert!(store.load(&first.meta.id).is_err());
        let after = store.list().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, second.meta.id);

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn session_kind_lists_keep_eps_and_map_surfaces_isolated() {
        let (base, store) = store("kind-isolation");
        let eps = sample_record(&new_session_id(), "EPS");
        let mut map = sample_record(&new_session_id(), "Map");
        map.meta.kind = SessionKind::Map;
        store.save(&eps).unwrap();
        store.save(&map).unwrap();
        assert_eq!(store.list_kind(SessionKind::Eps).unwrap(), vec![eps.meta]);
        assert_eq!(store.list_kind(SessionKind::Map).unwrap(), vec![map.meta]);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn context_usage_update_preserves_the_panel_log() {
        let (base, store) = store("context-usage");
        let record = sample_record(&new_session_id(), "토큰 사용량");
        let original_log = record.panel_log.clone();
        store.save(&record).unwrap();

        let usage = sample_usage();
        store
            .update_context_usage(&record.meta.id, usage.clone())
            .unwrap();

        let loaded = store.load(&record.meta.id).unwrap();
        assert_eq!(loaded.context_usage, Some(usage));
        assert_eq!(loaded.panel_log, original_log);

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn session_files_are_valid_utf8_without_bom() {
        let (base, store) = store("no-bom");
        let record = sample_record(&new_session_id(), "한글 이름");
        store.save(&record).unwrap();

        let dirs_root = store.sessions_dir.clone();
        let index_bytes = fs::read(dirs_root.join(INDEX_FILE)).unwrap();
        let record_bytes = fs::read(dirs_root.join(format!("{}.json", record.meta.id))).unwrap();

        for (label, bytes) in [("index.json", &index_bytes), ("record.json", &record_bytes)] {
            assert!(
                !bytes.starts_with(&[0xEF, 0xBB, 0xBF]),
                "{label} must not have a BOM"
            );
            assert!(
                std::str::from_utf8(bytes).is_ok(),
                "{label} must be valid UTF-8"
            );
        }

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn legacy_updated_at_seconds_migrates_to_last_conversation_millis() {
        let (base, store) = store("legacy-updated-at");
        fs::create_dir_all(&store.sessions_dir).unwrap();
        let id = new_session_id();
        let legacy = json!({
            "id": id,
            "name": "기존 세션",
            "project": "mymap",
            "createdAt": 1_718_000_000_u64,
            "updatedAt": 1_718_009_999_u64,
            "threadId": null,
            "pendingRequestIds": [],
            "panelLog": null
        });
        fs::write(store.record_path(&id), serde_json::to_vec(&legacy).unwrap()).unwrap();

        let loaded = store.load(&id).unwrap();
        assert_eq!(loaded.meta.last_conversation_at, 1_718_009_999_000);
        assert_eq!(loaded.meta.kind, SessionKind::Eps);
        assert_eq!(
            loaded.context_state,
            crate::context_state::SessionContextState::default()
        );
        assert_eq!(
            loaded.task_state,
            crate::task_state::SessionTaskState::default()
        );
        store.save(&loaded).unwrap();
        let migrated: serde_json::Value =
            serde_json::from_slice(&fs::read(store.record_path(&id)).unwrap()).unwrap();
        assert_eq!(migrated["lastConversationAt"], json!(1_718_009_999_000_u64));
        assert_eq!(migrated["kind"], json!("eps"));
        assert!(migrated.get("updatedAt").is_none());

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn corrupt_index_yields_empty_list() {
        let (base, store) = store("corrupt-index");
        fs::create_dir_all(&store.sessions_dir).unwrap();
        fs::write(store.sessions_dir.join(INDEX_FILE), b"not json {{{").unwrap();

        // Never crash startup: corrupt index -> [].
        assert!(store.list().unwrap().is_empty());

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn corrupt_record_is_a_graceful_err() {
        let (base, store) = store("corrupt-record");
        fs::create_dir_all(&store.sessions_dir).unwrap();
        let id = new_session_id();
        fs::write(store.record_path(&id), b"{ broken").unwrap();

        assert!(store.load(&id).is_err());

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn v3_cutover_preserves_conversation_but_clears_legacy_execution_state() {
        let base = unique_temp_dir("v3-cutover");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let record = sample_record(&new_session_id(), "보존할 대화");
        let index = json!({
            "schemaVersion": 2,
            "sessions": [record.meta.clone()]
        });
        fs::write(
            dirs.sessions_dir().join(INDEX_FILE),
            serde_json::to_vec_pretty(&index).unwrap(),
        )
        .unwrap();
        fs::write(
            dirs.sessions_dir().join(format!("{}.json", record.meta.id)),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .unwrap();
        fs::write(dirs.journal_dir().join("req-pending.json"), b"pending").unwrap();
        fs::create_dir_all(dirs.journal_dir().join("accepted")).unwrap();
        fs::write(
            dirs.journal_dir()
                .join("accepted")
                .join("req-accepted.json"),
            b"accepted",
        )
        .unwrap();
        fs::create_dir_all(dirs.session_workspaces_dir().join("project/session")).unwrap();
        fs::write(
            dirs.session_workspaces_dir()
                .join("project/session/spec.md"),
            b"stale",
        )
        .unwrap();

        let migrated = SessionStore::new(&dirs);
        let loaded = migrated.load(&record.meta.id).unwrap();
        assert_eq!(loaded.meta, record.meta);
        assert_eq!(loaded.panel_log, record.panel_log);
        assert!(loaded.thread_id.is_none());
        assert!(loaded.pending_request_ids.is_empty());
        assert!(loaded.context_usage.is_none());
        assert!(!dirs.journal_dir().join("req-pending.json").exists());
        assert!(dirs
            .journal_dir()
            .join("accepted")
            .join("req-accepted.json")
            .is_file());
        assert!(fs::read_dir(dirs.session_workspaces_dir())
            .unwrap()
            .next()
            .is_none());
        let migrated_index: serde_json::Value =
            serde_json::from_slice(&fs::read(dirs.sessions_dir().join(INDEX_FILE)).unwrap())
                .unwrap();
        assert_eq!(migrated_index["schemaVersion"], json!(SCHEMA_VERSION));

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn new_task_and_context_state_round_trip_and_repair_cached_projection() {
        let (base, store) = store("task-state-round-trip");
        let record = sample_record(&new_session_id(), "state");
        store.save(&record).unwrap();
        let turn = "11111111-1111-4111-8111-111111111111";
        let state = store
            .append_task_event(
                &record.meta.id,
                None,
                semantic_goal_event(turn, "req-1", 0, "goal-1"),
            )
            .unwrap();
        assert_eq!(state.projection.goals.len(), 1);

        let mut corrupt = store.load(&record.meta.id).unwrap();
        corrupt.task_state.projection_checksum = "corrupt".to_string();
        store.save(&corrupt).unwrap();
        let repaired = store.load(&record.meta.id).unwrap();
        assert_eq!(repaired.task_state.projection.goals[0].id, "goal-1");
        assert_ne!(repaired.task_state.projection_checksum, "corrupt");
        assert_eq!(
            repaired.context_state,
            crate::context_state::SessionContextState::default()
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn runtime_and_task_updates_do_not_overwrite_each_other() {
        let (base, store) = store("runtime-task-atomic");
        let record = sample_record(&new_session_id(), "atomic");
        let id = record.meta.id.clone();
        store.save(&record).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let runtime_store = store.clone();
        let runtime_id = id.clone();
        let runtime_barrier = Arc::clone(&barrier);
        let runtime = std::thread::spawn(move || {
            runtime_barrier.wait();
            runtime_store
                .update_runtime_state(
                    &runtime_id,
                    Some("thread-new".to_string()),
                    vec!["req-pending".to_string()],
                )
                .unwrap();
        });
        let task_store = store.clone();
        let task_id = id.clone();
        let task_barrier = Arc::clone(&barrier);
        let task = std::thread::spawn(move || {
            task_barrier.wait();
            task_store
                .append_task_event(
                    &task_id,
                    None,
                    semantic_goal_event(
                        "22222222-2222-4222-8222-222222222222",
                        "req-2",
                        0,
                        "goal-2",
                    ),
                )
                .unwrap();
        });
        barrier.wait();
        runtime.join().unwrap();
        task.join().unwrap();

        let loaded = store.load(&id).unwrap();
        assert_eq!(loaded.thread_id.as_deref(), Some("thread-new"));
        assert_eq!(loaded.pending_request_ids, vec!["req-pending"]);
        assert_eq!(loaded.task_state.projection.goals[0].id, "goal-2");
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn panel_log_and_detached_promotion_updates_do_not_overwrite_each_other() {
        let (base, store) = store("panel-promotion-atomic");
        let record = sample_record(&new_session_id(), "atomic");
        let id = record.meta.id.clone();
        store.save(&record).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let panel_store = store.clone();
        let panel_id = id.clone();
        let panel_barrier = Arc::clone(&barrier);
        let panel = std::thread::spawn(move || {
            panel_barrier.wait();
            panel_store
                .update_panel_log(
                    &panel_id,
                    json!({"schemaVersion": 2, "logSeq": 1, "log": [
                        {"id": 1, "kind": "you", "text": "new"}
                    ]}),
                )
                .unwrap();
        });
        let promotion_store = store.clone();
        let promotion_id = id.clone();
        let promotion_barrier = Arc::clone(&barrier);
        let promotion = std::thread::spawn(move || {
            promotion_barrier.wait();
            assert!(!promotion_store
                .record_task_promotion(
                    &promotion_id,
                    crate::task_state::TaskStatePromotionAudit {
                        harness_job_id: "harness-detached".to_string(),
                        source_event_id: "detached-event".to_string(),
                        fact_ids: Vec::new(),
                        accepted: true,
                        document_refs: Vec::new(),
                        memory_refs: Vec::new(),
                        timestamp: 1,
                    },
                )
                .unwrap());
        });
        barrier.wait();
        panel.join().unwrap();
        promotion.join().unwrap();

        let loaded = store.load(&id).unwrap();
        assert_eq!(loaded.panel_log["log"][0]["text"], "new");
        assert_eq!(loaded.task_state.promotion_audits.len(), 1);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn rewind_uses_client_turn_anchor_and_legacy_prefix_fails_closed() {
        let (base, store) = store("rewind-anchor");
        let record = sample_record(&new_session_id(), "rewind");
        let id = record.meta.id.clone();
        store.save(&record).unwrap();
        let first_turn = "33333333-3333-4333-8333-333333333333";
        let second_turn = "44444444-4444-4444-8444-444444444444";
        let first = store
            .append_task_event(
                &id,
                None,
                semantic_goal_event(first_turn, "req-first", 0, "first"),
            )
            .unwrap();
        store
            .append_task_event(
                &id,
                first.leaf_id.as_deref(),
                semantic_goal_event(second_turn, "req-second", 1, "second"),
            )
            .unwrap();

        store
            .move_task_leaf_for_rewind(
                &id,
                json!({"schemaVersion": 2, "logSeq": 1, "log": [{
                    "id": 1,
                    "kind": "you",
                    "text": "first",
                    "clientTurnId": first_turn
                }]}),
            )
            .unwrap();
        let rewound = store.load(&id).unwrap();
        assert_eq!(rewound.task_state.events.len(), 2);
        assert_eq!(rewound.task_state.projection.goals.len(), 1);
        assert_eq!(rewound.task_state.projection.goals[0].id, "first");
        assert!(rewound.thread_id.is_none());
        assert!(rewound.pending_request_ids.is_empty());
        assert!(rewound.context_state.instruction_epoch >= 2);

        store
            .move_task_leaf_for_rewind(
                &id,
                json!({"schemaVersion": 2, "logSeq": 1, "log": [{
                    "id": 1, "kind": "you", "text": "legacy"
                }]}),
            )
            .unwrap();
        let legacy = store.load(&id).unwrap();
        assert!(legacy.task_state.leaf_id.is_none());
        assert_eq!(
            legacy.task_state.projection,
            crate::task_state::ActiveTaskProjection::default()
        );
        assert_eq!(legacy.task_state.events.len(), 2);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn new_session_id_is_a_well_formed_unique_v4_uuid() {
        let a = new_session_id();
        let b = new_session_id();
        assert_ne!(a, b);
        // 8-4-4-4-12 hex with version 4 and RFC-4122 variant.
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(a.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        assert!(parts[2].starts_with('4'), "version nibble must be 4");
        assert!(
            matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'),
            "variant nibble must be RFC-4122"
        );
    }
}
