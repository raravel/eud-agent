//! Post-acceptance harness synchronization.
//!
//! Code/map changes are reviewed first. Accepted changes create one durable job that
//! optionally waits for user runtime verification, asks Codex for one structured
//! document delta, stages that delta in an isolated workspace, and exposes a separate
//! review surface. The foreground implementation turn never writes project documents.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::{Component, Path};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::DataDirs;
use crate::journal::{JournalEntry, JournalStore, JournalTarget, WriteTool};
use crate::memory::{write_atomic_bytes, ProjectMemory};
use crate::workspace::{
    apply_exact_text_edits, completion_worklog_path, ExactTextEdit, WorkspaceDocumentUpdate,
    WorkspaceManager, WorkspaceTurnRecorder,
};

const JOB_SCHEMA_VERSION: u32 = 2;
const MAX_JOBS_PER_SESSION: usize = 100;
const MAX_PROMPT_CONTEXT_BYTES: usize = 192 * 1024;
const MAX_DELTA_DOCUMENTS: usize = 8;
const MAX_SUMMARY_BYTES: usize = 4 * 1024;
const MEMORY_FILES: [&str; 4] = ["resources", "structure", "conventions", "lessons"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessJobStatus {
    WaitingRuntime,
    Pending,
    Running,
    Review,
    Failed,
    Completed,
    Rejected,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeVerification {
    NotRequired,
    Waiting,
    Confirmed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildEvidence {
    pub ok: bool,
    pub error_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessMemoryUpdate {
    pub file: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessDocumentPatch {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<ExactTextEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessDelta {
    pub summary: String,
    #[serde(default)]
    pub documents: Vec<HarnessDocumentPatch>,
    #[serde(default)]
    pub memory_updates: Vec<HarnessMemoryUpdate>,
    #[serde(default)]
    pub promoted_fact_ids: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessProviderBinding {
    pub provider: crate::provider::ProviderId,
    pub model: String,
    pub reasoning: Option<crate::provider::ReasoningSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

impl Default for HarnessProviderBinding {
    fn default() -> Self {
        Self {
            provider: crate::provider::ProviderId::Codex,
            model: "default".to_string(),
            reasoning: None,
            base_url: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessJob {
    pub schema_version: u32,
    pub id: String,
    pub session_id: String,
    #[serde(default)]
    pub provider_binding: HarnessProviderBinding,
    pub project: String,
    pub workspace_id: String,
    pub source_request_id: String,
    pub harness_request_id: Option<String>,
    pub workspace_session_id: String,
    pub status: HarnessJobStatus,
    pub runtime_verification: RuntimeVerification,
    pub attempts: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub request_text: String,
    pub approved_plan: Option<String>,
    pub final_answer: String,
    pub accepted_entries: Vec<JournalEntry>,
    /// Facts pinned at source changeset acceptance; absent on legacy jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_state_promotion: Option<crate::task_state::TaskStatePromotionInput>,
    pub build: Option<BuildEvidence>,
    pub delta: Option<HarnessDelta>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retry_feedback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retry_delta: Option<HarnessDelta>,
    #[serde(default)]
    pub dismissed: bool,
}

impl HarnessJob {
    /// Constructs the complete durable source snapshot in one place; keeping the
    /// fields explicit prevents partial or stale post-acceptance jobs.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_provider(
        session_id: String,
        provider_binding: HarnessProviderBinding,
        project: String,
        workspace_id: String,
        source_request_id: String,
        request_text: String,
        approved_plan: Option<String>,
        final_answer: String,
        accepted_entries: Vec<JournalEntry>,
        build: Option<BuildEvidence>,
    ) -> Self {
        let id = format!("harness-{}", uuid::Uuid::new_v4().simple());
        let runtime_verification = classify_runtime_verification(&accepted_entries);
        let status = match runtime_verification {
            RuntimeVerification::Waiting => HarnessJobStatus::WaitingRuntime,
            RuntimeVerification::NotRequired | RuntimeVerification::Confirmed => {
                HarnessJobStatus::Pending
            }
            RuntimeVerification::Skipped => HarnessJobStatus::Skipped,
        };
        let now = epoch_seconds();
        Self {
            schema_version: JOB_SCHEMA_VERSION,
            workspace_session_id: id.clone(),
            id,
            session_id,
            provider_binding,
            project,
            workspace_id,
            source_request_id,
            harness_request_id: None,
            status,
            runtime_verification,
            attempts: 0,
            created_at: now,
            updated_at: now,
            request_text,
            approved_plan,
            final_answer,
            accepted_entries,
            task_state_promotion: None,
            build,
            delta: None,
            error: None,
            retry_feedback: None,
            retry_delta: None,
            dismissed: false,
        }
    }
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: String,
        project: String,
        workspace_id: String,
        source_request_id: String,
        request_text: String,
        approved_plan: Option<String>,
        final_answer: String,
        accepted_entries: Vec<JournalEntry>,
        build: Option<BuildEvidence>,
    ) -> Self {
        Self::new_with_provider(
            session_id,
            HarnessProviderBinding::default(),
            project,
            workspace_id,
            source_request_id,
            request_text,
            approved_plan,
            final_answer,
            accepted_entries,
            build,
        )
    }

    pub fn touch(&mut self) {
        self.updated_at = epoch_seconds();
    }

    pub fn fail(&mut self, message: String) {
        self.status = HarnessJobStatus::Failed;
        self.retry_feedback = Some(message.clone());
        self.error = Some(message);
        self.touch();
    }

    pub fn retry(&mut self) -> Result<(), String> {
        if self.status != HarnessJobStatus::Failed {
            return Err("only a failed harness job can be retried".to_string());
        }
        self.status = HarnessJobStatus::Pending;
        if let Some(error) = self.error.take() {
            self.retry_feedback = Some(error);
        }
        self.touch();
        Ok(())
    }

    pub fn skip_runtime(&mut self) -> Result<(), String> {
        if self.status != HarnessJobStatus::WaitingRuntime {
            return Err("harness job is not waiting for runtime verification".to_string());
        }
        self.runtime_verification = RuntimeVerification::Skipped;
        self.status = HarnessJobStatus::Skipped;
        self.error = None;
        self.touch();
        Ok(())
    }

    pub fn dismiss(&mut self) -> Result<(), String> {
        if !matches!(
            self.status,
            HarnessJobStatus::Failed
                | HarnessJobStatus::Completed
                | HarnessJobStatus::Rejected
                | HarnessJobStatus::Skipped
        ) {
            return Err("only a terminal harness job can be dismissed".to_string());
        }
        self.dismissed = true;
        self.touch();
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessJobView {
    pub id: String,
    pub session_id: String,
    pub source_request_id: String,
    pub status: HarnessJobStatus,
    pub runtime_verification: RuntimeVerification,
    pub attempts: u32,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub memory_files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changeset: Option<crate::ipc::ChangesetEvent>,
    pub dismissed: bool,
}

impl HarnessJob {
    pub fn view(&self, changeset: Option<crate::ipc::ChangesetEvent>) -> HarnessJobView {
        HarnessJobView {
            id: self.id.clone(),
            session_id: self.session_id.clone(),
            source_request_id: self.source_request_id.clone(),
            status: self.status,
            runtime_verification: self.runtime_verification,
            attempts: self.attempts,
            created_at: self.created_at,
            updated_at: self.updated_at,
            summary: self.delta.as_ref().map(|delta| delta.summary.clone()),
            error: self.error.clone(),
            memory_files: self
                .delta
                .as_ref()
                .map(|delta| {
                    delta
                        .memory_updates
                        .iter()
                        .map(|update| update.file.clone())
                        .collect()
                })
                .unwrap_or_default(),
            changeset,
            dismissed: self.dismissed,
        }
    }
}

#[derive(Clone)]
pub struct HarnessJobStore {
    dirs: DataDirs,
    lock: Arc<Mutex<()>>,
}

impl HarnessJobStore {
    pub fn new(dirs: DataDirs) -> Self {
        Self {
            dirs,
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn create(&self, job: &HarnessJob) -> io::Result<()> {
        let _guard = self.guard()?;
        fs::create_dir_all(self.dirs.harness_jobs_dir())?;
        if self.job_path(&job.id).exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("harness job `{}` already exists", job.id),
            ));
        }
        self.save_unlocked(job)
    }

    pub fn save(&self, job: &HarnessJob) -> io::Result<()> {
        let _guard = self.guard()?;
        self.save_unlocked(job)
    }

    pub fn load(&self, id: &str) -> io::Result<HarnessJob> {
        let _guard = self.guard()?;
        self.load_unlocked(id)
    }

    pub fn list_session(&self, session_id: &str) -> io::Result<Vec<HarnessJob>> {
        let _guard = self.guard()?;
        let root = self.dirs.harness_jobs_dir();
        let mut jobs = Vec::new();
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(jobs),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file()
                || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let Ok(job) = read_job(&entry.path()) else {
                continue;
            };
            if job.session_id == session_id {
                jobs.push(job);
            }
        }
        jobs.sort_by_key(|job| (job.created_at, job.id.clone()));
        if jobs.len() > MAX_JOBS_PER_SESSION {
            jobs.drain(..jobs.len() - MAX_JOBS_PER_SESSION);
        }
        Ok(jobs)
    }

    pub fn delete_session(&self, session_id: &str) -> io::Result<Vec<HarnessJob>> {
        let _guard = self.guard()?;
        let root = self.dirs.harness_jobs_dir();
        let mut removed = Vec::new();
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(removed),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file() {
                continue;
            }
            let Ok(job) = read_job(&path) else {
                continue;
            };
            if job.session_id == session_id {
                fs::remove_file(path)?;
                removed.push(job);
            }
        }
        Ok(removed)
    }

    pub fn recover_interrupted(&self, session_id: &str) -> io::Result<Vec<HarnessJob>> {
        let mut jobs = self.list_session(session_id)?;
        for job in &mut jobs {
            if job.status == HarnessJobStatus::Running {
                job.fail(
                    "앱 종료로 하네스 동기화가 중단되었습니다. 다시 시도해 주세요.".to_string(),
                );
                self.save(job)?;
            }
        }
        Ok(jobs)
    }

    fn guard(&self) -> io::Result<MutexGuard<'_, ()>> {
        self.lock
            .lock()
            .map_err(|_| io::Error::other("harness job store lock poisoned"))
    }

    fn load_unlocked(&self, id: &str) -> io::Result<HarnessJob> {
        validate_job_id(id)?;
        read_job(&self.job_path(id))
    }

    fn save_unlocked(&self, job: &HarnessJob) -> io::Result<()> {
        validate_job_id(&job.id)?;
        let bytes = serde_json::to_vec_pretty(job).map_err(io::Error::other)?;
        write_atomic_bytes(&self.job_path(&job.id), &bytes).map_err(io::Error::other)
    }

    fn job_path(&self, id: &str) -> std::path::PathBuf {
        self.dirs.harness_jobs_dir().join(format!("{id}.json"))
    }
}

pub fn classify_runtime_verification(entries: &[JournalEntry]) -> RuntimeVerification {
    let requires_runtime = entries.iter().any(|entry| {
        matches!(
            entry.tool,
            WriteTool::DatSet
                | WriteTool::XdatSet
                | WriteTool::TblSet
                | WriteTool::ReqSet
                | WriteTool::BtnSet
                | WriteTool::FileWrite
                | WriteTool::FileCreate
                | WriteTool::FileDelete
                | WriteTool::FileRename
                | WriteTool::FileMove
                | WriteTool::PluginAdd
                | WriteTool::PluginEdit
                | WriteTool::PluginRemove
                | WriteTool::PluginMove
                | WriteTool::LocationWrite
                | WriteTool::PlayerSetup
                | WriteTool::SwitchWrite
                | WriteTool::MapSound
        )
    });
    if requires_runtime {
        RuntimeVerification::Waiting
    } else {
        RuntimeVerification::NotRequired
    }
}

pub fn output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "documents", "memoryUpdates", "promotedFactIds"],
        "properties": {
            "promotedFactIds": {
                "type": "array",
                "maxItems": 64,
                "items": {"type": "string", "minLength": 1, "maxLength": 128}
            },
            "summary": {"type": "string", "minLength": 1, "maxLength": MAX_SUMMARY_BYTES},
            "documents": {
                "type": "array",
                "maxItems": MAX_DELTA_DOCUMENTS,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["path", "create", "edits"],
                    "properties": {
                        "path": {"type": "string", "minLength": 1, "maxLength": 240},
                        "create": {"type": ["string", "null"], "maxLength": 1048576},
                        "edits": {
                            "type": "array",
                            "maxItems": 32,
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["old_text", "new_text"],
                                "properties": {
                                    "old_text": {"type": "string", "minLength": 1},
                                    "new_text": {"type": "string"}
                                }
                            }
                        }
                    }
                }
            },
            "memoryUpdates": {
                "type": "array",
                "maxItems": 4,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["file", "content"],
                    "properties": {
                        "file": {"type": "string", "enum": MEMORY_FILES},
                        "content": {"type": "string", "maxLength": 24576}
                    }
                }
            }
        }
    })
}

pub fn parse_delta(text: &str) -> Result<HarnessDelta, String> {
    let delta: HarnessDelta = serde_json::from_str(text)
        .map_err(|error| format!("structured harness delta is invalid: {error}"))?;
    validate_delta(&delta)?;
    Ok(delta)
}

pub fn validate_delta(delta: &HarnessDelta) -> Result<(), String> {
    let summary = delta.summary.trim();
    if summary.is_empty() || summary.len() > MAX_SUMMARY_BYTES {
        return Err("harness summary must contain 1 to 4096 bytes".to_string());
    }
    if delta.promoted_fact_ids.len() > 64 {
        return Err("harness delta exceeds the 64 promoted-fact limit".to_string());
    }
    let mut promoted = HashSet::new();
    if delta
        .promoted_fact_ids
        .iter()
        .any(|id| !promoted.insert(id.as_str()))
    {
        return Err("harness delta contains duplicate promoted fact ids".to_string());
    }
    if delta.documents.len() > MAX_DELTA_DOCUMENTS {
        return Err(format!(
            "harness delta exceeds the {MAX_DELTA_DOCUMENTS}-document limit"
        ));
    }
    let mut paths = HashSet::new();
    let mut creates_topic_spec = false;
    for document in &delta.documents {
        validate_document_path(&document.path)?;
        if !paths.insert(document.path.as_str()) {
            return Err(format!("duplicate harness document `{}`", document.path));
        }
        creates_topic_spec |= document.create.is_some()
            && document.path.starts_with("specs/")
            && document.path != "specs/index.md";
        match (&document.create, document.edits.is_empty()) {
            (Some(content), true) if !content.is_empty() => {}
            (None, false) => {}
            _ => {
                return Err(format!(
                    "harness document `{}` must use exactly one of non-empty create or edits",
                    document.path
                ));
            }
        }
    }
    if creates_topic_spec && !paths.contains("specs/index.md") {
        return Err(
            "creating a canonical topic spec requires the same delta to update `specs/index.md`"
                .to_string(),
        );
    }
    let mut memory_files = HashSet::new();
    for update in &delta.memory_updates {
        if !MEMORY_FILES.contains(&update.file.as_str()) {
            return Err(format!("unknown harness memory file `{}`", update.file));
        }
        if !memory_files.insert(update.file.as_str()) {
            return Err(format!("duplicate harness memory file `{}`", update.file));
        }
    }
    Ok(())
}

pub fn generation_prompt(job: &HarnessJob, dirs: &DataDirs) -> Result<String, String> {
    let documents = canonical_document_context(dirs, &job.workspace_id)?;
    let memory = ProjectMemory::new(dirs.memory_dir(), job.project.clone());
    let memory_context = MEMORY_FILES
        .iter()
        .map(|name| format!("## {name}.md\n{}", memory.read(name)))
        .collect::<Vec<_>>()
        .join("\n\n");
    let retry_delta = job
        .retry_delta
        .as_ref()
        .map(serde_json::to_string_pretty)
        .transpose()
        .map_err(|error| error.to_string())?;
    let accepted =
        serde_json::to_string_pretty(&job.accepted_entries).map_err(|error| error.to_string())?;
    let accepted_budget = if retry_delta.is_some() {
        MAX_PROMPT_CONTEXT_BYTES / 3
    } else {
        MAX_PROMPT_CONTEXT_BYTES / 2
    };
    let accepted = truncate_utf8(&accepted, accepted_budget);
    let promotion = job
        .task_state_promotion
        .as_ref()
        .map(serde_json::to_string_pretty)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| "(none)".to_string());
    let documents = truncate_utf8(&documents, MAX_PROMPT_CONTEXT_BYTES / 3);
    let memory_context = truncate_utf8(&memory_context, MAX_PROMPT_CONTEXT_BYTES / 6);
    let retry_feedback = job
        .retry_feedback
        .as_deref()
        .map(|error| {
            format!(
                "The previous structured delta was rejected before document promotion. Correct \
the failure instead of repeating the same delta.\nValidation error: {error}"
            )
        })
        .unwrap_or_else(|| "(none; first attempt)".to_string());
    let retry_delta = retry_delta
        .map(|delta| truncate_utf8(&delta, MAX_PROMPT_CONTEXT_BYTES / 6))
        .unwrap_or_else(|| "(none)".to_string());
    Ok(format!(
        "You are the eud-agent post-acceptance harness synchronizer. Return exactly one JSON object matching the supplied output schema. Do not call tools, do not inspect the filesystem, and do not emit Markdown fences or prose. All required context is inline.\n\n\
The code/map changes below are already accepted. Update only durable current-behavior specs and durable project memory. Do not write plans or worklogs; the server creates the worklog deterministically. Prefer exact edits to existing topic specs. Exact edits are applied in order; every old_text must match exactly once in the content produced by all earlier edits, so use non-overlapping anchors or account for earlier replacements. Create a new specs or decisions page only when no canonical topic can hold the accepted fact. Do not duplicate facts between specs and memory: memory is only for resource allocations, file topology/roles, stable conventions, or user corrections. Task-state promotion candidates are optional evidence, not a requirement to change documents. Put a candidate id in promotedFactIds only when this delta actually incorporates that exact accepted fact into a specs/decisions document or allowed project-memory file; otherwise omit it.\n\n\
[previous harness failure]
{}

[previous rejected delta]
{}

[request]
{}

[approved plan]
{}

[implementation answer]
{}

[build evidence]
{}

[runtime verification]
{:?}

[accepted task-state promotion candidates]
{}

[accepted journal entries]
{}

[canonical documents]
{}

[project memory]
{}",
        retry_feedback,
        retry_delta,
        job.request_text,
        job.approved_plan.as_deref().unwrap_or("(direct change; no approved plan)"),
        job.final_answer,
        serde_json::to_string(&job.build).map_err(|error| error.to_string())?,
        job.runtime_verification,
        promotion,
        accepted,
        documents,
        memory_context,
    ))
}

pub fn stage_delta(
    dirs: &DataDirs,
    journal: JournalStore,
    job: &mut HarnessJob,
    delta: HarnessDelta,
) -> Result<usize, String> {
    match stage_delta_inner(dirs, journal, job, &delta) {
        Ok((count, request_id)) => {
            job.harness_request_id = Some(request_id);
            job.delta = Some(delta);
            job.retry_delta = None;
            Ok(count)
        }
        Err(error) => {
            job.retry_delta = Some(delta);
            Err(error)
        }
    }
}

fn stage_delta_inner(
    dirs: &DataDirs,
    journal: JournalStore,
    job: &HarnessJob,
    delta: &HarnessDelta,
) -> Result<(usize, String), String> {
    validate_delta(delta)?;
    let allowed_promotion_ids = job
        .task_state_promotion
        .as_ref()
        .map(|input| {
            input
                .fact_ids
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    if delta
        .promoted_fact_ids
        .iter()
        .any(|id| !allowed_promotion_ids.contains(id.as_str()))
    {
        return Err("harness delta promoted an unapproved task-state fact".to_string());
    }
    if !delta.promoted_fact_ids.is_empty()
        && delta.documents.is_empty()
        && delta.memory_updates.is_empty()
    {
        return Err(
            "harness delta cannot promote facts without a document or memory update".to_string(),
        );
    }
    let request_id = format!("req-{}-{}", job.id, job.attempts);
    let manager = WorkspaceManager::new(dirs.clone());
    let workspace = manager
        .prepare_document_session(&job.workspace_id, &job.project, &job.workspace_session_id)
        .map_err(|error| error.to_string())?;
    let baseline = manager
        .begin_turn(&workspace, &request_id)
        .map_err(|error| error.to_string())?;
    let mut recorder = WorkspaceTurnRecorder::new(manager.clone(), baseline, journal);

    let mut updates = Vec::with_capacity(delta.documents.len() + 1);
    for document in &delta.documents {
        let current = manager
            .document_content(&workspace, &document.path)
            .map_err(|error| error.to_string())?;
        let content = if let Some(created) = &document.create {
            if current.is_some() {
                return Err(format!(
                    "harness document `{}` already exists and cannot be created",
                    document.path
                ));
            }
            created.clone()
        } else {
            let current = current
                .ok_or_else(|| format!("harness document `{}` does not exist", document.path))?;
            apply_exact_text_edits(&document.path, &current, &document.edits)
                .map_err(|error| error.to_string())?
        };
        updates.push(WorkspaceDocumentUpdate {
            path: document.path.clone(),
            content,
        });
    }

    let worklog_path =
        completion_worklog_path(&job.source_request_id).map_err(|error| error.to_string())?;
    updates.push(WorkspaceDocumentUpdate {
        path: worklog_path,
        content: render_worklog(job, delta),
    });
    manager
        .apply_document_updates(&workspace, &updates)
        .map_err(|error| error.to_string())?;
    let count = recorder.finish().map_err(|error| error.to_string())?;
    if count == 0 {
        return Err("harness delta produced no reviewable documents".to_string());
    }
    Ok((count, request_id))
}

pub struct AppliedMemoryUpdates {
    project: String,
    previous: Vec<(String, String)>,
}

pub fn apply_memory_updates(
    dirs: &DataDirs,
    job: &HarnessJob,
) -> Result<AppliedMemoryUpdates, String> {
    let updates = job
        .delta
        .as_ref()
        .map(|delta| delta.memory_updates.as_slice())
        .unwrap_or_default();
    let memory = ProjectMemory::new(dirs.memory_dir(), job.project.clone());
    let previous = updates
        .iter()
        .map(|update| (update.file.clone(), memory.read(&update.file)))
        .collect::<Vec<_>>();
    for (index, update) in updates.iter().enumerate() {
        let result = memory.write(&update.file, &update.content);
        if !result.ok {
            for (file, content) in previous[..index].iter().rev() {
                let _ = memory.write(file, content);
            }
            return Err(format!(
                "harness memory update `{}` failed: {}",
                update.file, result.reason
            ));
        }
    }
    Ok(AppliedMemoryUpdates {
        project: job.project.clone(),
        previous,
    })
}

pub fn rollback_memory_updates(dirs: &DataDirs, applied: AppliedMemoryUpdates) {
    let memory = ProjectMemory::new(dirs.memory_dir(), applied.project);
    for (file, content) in applied.previous.into_iter().rev() {
        let _ = memory.write(&file, &content);
    }
}

pub fn task_state_promotion_audit(
    dirs: &DataDirs,
    job: &HarnessJob,
    accepted: bool,
) -> Result<Option<crate::task_state::TaskStatePromotionAudit>, String> {
    let Some(input) = job.task_state_promotion.as_ref() else {
        return Ok(None);
    };
    let promoted_fact_ids = if accepted {
        job.delta
            .as_ref()
            .map(|delta| delta.promoted_fact_ids.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut document_refs = Vec::new();
    let mut memory_refs = Vec::new();
    if accepted {
        let delta = job
            .delta
            .as_ref()
            .ok_or_else(|| "accepted harness job has no structured delta".to_string())?;
        for document in &delta.documents {
            let path = dirs
                .workspaces_dir()
                .join(&job.workspace_id)
                .join(document.path.replace('/', std::path::MAIN_SEPARATOR_STR));
            document_refs.push(promoted_ref(&document.path, &path)?);
        }
        let memory = ProjectMemory::new(dirs.memory_dir(), job.project.clone());
        let store = memory
            .store_dir()
            .ok_or_else(|| "accepted harness job has no project memory store".to_string())?;
        for update in &delta.memory_updates {
            let name = format!("{}.md", update.file);
            memory_refs.push(promoted_ref(&name, &store.join(&name))?);
        }
    }
    Ok(Some(crate::task_state::TaskStatePromotionAudit {
        harness_job_id: job.id.clone(),
        source_event_id: input.source_event_id.clone(),
        fact_ids: promoted_fact_ids,
        accepted,
        document_refs,
        memory_refs,
        timestamp: crate::session::now_unix_seconds(),
    }))
}

fn promoted_ref(relative: &str, path: &Path) -> Result<crate::task_state::PromotedRef, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("promoted artifact `{relative}` is unavailable: {error}"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "promoted artifact `{relative}` is not a regular file"
        ));
    }
    let bytes = fs::read(path)
        .map_err(|error| format!("promoted artifact `{relative}` read failed: {error}"))?;
    Ok(crate::task_state::PromotedRef {
        path: relative.to_string(),
        sha256: crate::task_state::sha256_bytes(&bytes),
    })
}

pub fn cleanup_job_workspace(dirs: &DataDirs, job: &HarnessJob) {
    let root = dirs
        .session_workspaces_dir()
        .join(&job.workspace_id)
        .join(&job.workspace_session_id);
    let _ = fs::remove_dir_all(root);
}

fn render_worklog(job: &HarnessJob, delta: &HarnessDelta) -> String {
    let mut targets = BTreeSet::new();
    for entry in &job.accepted_entries {
        targets.insert(render_target(&entry.target));
    }
    let target_lines = targets
        .into_iter()
        .map(|target| format!("- `{target}`"))
        .collect::<Vec<_>>()
        .join("\n");
    let build = match &job.build {
        Some(build) if build.ok => "- Complete project build: passed".to_string(),
        Some(build) => format!(
            "- Complete project build: failed with {} error(s)",
            build.error_count
        ),
        None => "- Complete project build: not recorded".to_string(),
    };
    let runtime = match job.runtime_verification {
        RuntimeVerification::Confirmed => "- Runtime verification: confirmed by the user",
        RuntimeVerification::NotRequired => {
            "- Runtime verification: not required for the accepted change category"
        }
        RuntimeVerification::Waiting => "- Runtime verification: pending",
        RuntimeVerification::Skipped => "- Runtime verification: skipped by the user",
    };
    let spec_links = delta
        .documents
        .iter()
        .filter(|document| document.path.starts_with("specs/"))
        .map(|document| {
            let relative = document.path.trim_start_matches("specs/");
            format!("- [{}](../specs/{relative})", document.path)
        })
        .collect::<Vec<_>>();
    let spec_links = if spec_links.is_empty() {
        "- [Project specifications](../specs/index.md)".to_string()
    } else {
        spec_links.join("\n")
    };
    format!(
        "# {} worklog\n\n## Actual result\n\n{}\n\n## Accepted targets\n\n{}\n\n## Verification\n\n{}\n{}\n\n## Canonical specifications\n\n{}\n",
        job.source_request_id, delta.summary, target_lines, build, runtime, spec_links
    )
}

fn render_target(target: &JournalTarget) -> String {
    match target {
        JournalTarget::Dat {
            table,
            dat,
            obj_id,
            property,
        } => format!("{table}:{dat}:{obj_id}:{property}"),
        JournalTarget::Path { path } => path.clone(),
        JournalTarget::WorkspacePath { path, .. } => path.clone(),
        JournalTarget::Rename { from, to } => format!("{from} -> {to}"),
        JournalTarget::Setting { key } => format!("setting:{key}"),
        JournalTarget::Plugin { plugin_id } => format!("plugin:{plugin_id}"),
        JournalTarget::Map { path, summary } => format!("{path} ({summary})"),
        JournalTarget::MapSound {
            source_map,
            mpq_path,
            ..
        } => format!("{} ({mpq_path})", source_map.display()),
    }
}

fn canonical_document_context(dirs: &DataDirs, workspace_id: &str) -> Result<String, String> {
    validate_workspace_id(workspace_id)?;
    let root = dirs.workspaces_dir().join(workspace_id);
    let mut files = Vec::new();
    for directory in ["specs", "decisions"] {
        collect_markdown_files(&root, Path::new(directory), &mut files)?;
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut output = String::new();
    for (path, content) in files {
        output.push_str(&format!("## {path}\n{content}\n\n"));
        if output.len() >= MAX_PROMPT_CONTEXT_BYTES {
            break;
        }
    }
    Ok(output)
}

fn collect_markdown_files(
    root: &Path,
    relative: &Path,
    files: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let directory = root.join(relative);
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        let child_relative = relative.join(entry.file_name());
        if file_type.is_dir() {
            collect_markdown_files(root, &child_relative, files)?;
        } else if file_type.is_file()
            && entry.path().extension().and_then(|value| value.to_str()) == Some("md")
        {
            let content = fs::read_to_string(entry.path()).map_err(|error| error.to_string())?;
            files.push((child_relative.to_string_lossy().replace('\\', "/"), content));
        }
    }
    Ok(())
}

fn validate_document_path(path: &str) -> Result<(), String> {
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || candidate
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !path.ends_with(".md")
        || !(path.starts_with("specs/") || path.starts_with("decisions/"))
    {
        return Err(format!("unsafe harness document path `{path}`"));
    }
    Ok(())
}

fn validate_job_id(id: &str) -> io::Result<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "harness job id contains unsafe characters",
        ));
    }
    Ok(())
}

fn validate_workspace_id(id: &str) -> Result<(), String> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("workspace id must be a 64-character hex digest".to_string());
    }
    Ok(())
}

fn read_job(path: &Path) -> io::Result<HarnessJob> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &text[..end])
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Snapshot;
    use std::path::PathBuf;

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "eud-agent-harness-test-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn entry(tool: WriteTool) -> JournalEntry {
        JournalEntry {
            id: "file-1".to_string(),
            seq: 1,
            tool,
            target: JournalTarget::Path {
                path: "main".to_string(),
            },
            before: Snapshot::Created,
            after: Snapshot::Created,
            ts: 1,
        }
    }

    fn assert_strict_object_requirements(schema: &Value, path: &str) {
        if schema.get("type") == Some(&json!("object")) {
            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("{path}: object schema has no properties"));
            let required = schema
                .get("required")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("{path}: object schema has no required array"))
                .iter()
                .filter_map(Value::as_str)
                .collect::<BTreeSet<_>>();
            let property_names = properties
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            assert_eq!(
                required, property_names,
                "{path}: strict response schema must require every property"
            );
            for (name, property) in properties {
                assert_strict_object_requirements(property, &format!("{path}.{name}"));
            }
        }
        if let Some(items) = schema.get("items") {
            assert_strict_object_requirements(items, &format!("{path}[]"));
        }
    }

    #[test]
    fn output_schema_requires_every_property_of_strict_objects() {
        assert_strict_object_requirements(&output_schema(), "$");
    }

    #[test]
    fn runtime_verification_is_required_only_for_live_project_mutations() {
        assert_eq!(
            classify_runtime_verification(&[entry(WriteTool::FileWrite)]),
            RuntimeVerification::Waiting
        );
        assert_eq!(
            classify_runtime_verification(&[entry(WriteTool::MapSound)]),
            RuntimeVerification::Waiting
        );
        assert_eq!(
            classify_runtime_verification(&[entry(WriteTool::WorkspaceWrite)]),
            RuntimeVerification::NotRequired
        );
        assert_eq!(
            classify_runtime_verification(&[entry(WriteTool::SettingsSet)]),
            RuntimeVerification::NotRequired
        );
    }

    #[test]
    fn runtime_skip_cancels_the_harness_without_starting_generation() {
        let mut job = HarnessJob::new(
            "session".to_string(),
            "Project".to_string(),
            "a".repeat(64),
            "req-code".to_string(),
            "Change runtime behavior".to_string(),
            None,
            "Done".to_string(),
            vec![entry(WriteTool::FileWrite)],
            None,
        );
        assert_eq!(job.status, HarnessJobStatus::WaitingRuntime);

        job.skip_runtime().unwrap();

        assert_eq!(job.status, HarnessJobStatus::Skipped);
        assert_eq!(job.runtime_verification, RuntimeVerification::Skipped);
        assert!(job.skip_runtime().is_err());
    }

    #[test]
    fn generation_prompt_includes_rejected_delta_and_failure_before_retry() {
        let base = unique_temp_dir("retry-feedback");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let workspace_id = "a".repeat(64);
        let document = dirs
            .workspaces_dir()
            .join(&workspace_id)
            .join("specs/gameplay.md");
        fs::create_dir_all(document.parent().unwrap()).unwrap();
        fs::write(&document, "# Gameplay\n\nCurrent behavior.\n").unwrap();
        let mut job = HarnessJob::new(
            "session".to_string(),
            "Project".to_string(),
            workspace_id,
            "req-code".to_string(),
            "Change behavior".to_string(),
            None,
            "Done".to_string(),
            vec![entry(WriteTool::WorkspaceWrite)],
            None,
        );
        let rejected_delta = HarnessDelta {
            summary: "Changed gameplay behavior.".to_string(),
            documents: vec![HarnessDocumentPatch {
                path: "specs/gameplay.md".to_string(),
                create: None,
                edits: vec![
                    ExactTextEdit {
                        old_text: "Current behavior.".to_string(),
                        new_text: "Updated behavior.".to_string(),
                    },
                    ExactTextEdit {
                        old_text: "Current behavior.".to_string(),
                        new_text: "Final behavior.".to_string(),
                    },
                ],
            }],
            memory_updates: Vec::new(),
            promoted_fact_ids: Vec::new(),
        };
        let failure = stage_delta(
            &dirs,
            JournalStore::new(dirs.app_data()),
            &mut job,
            rejected_delta.clone(),
        )
        .unwrap_err();
        assert_eq!(
            failure,
            "file_edit edit 1 old_text was not found in `specs/gameplay.md`"
        );
        assert_eq!(job.retry_delta.as_ref(), Some(&rejected_delta));
        job.fail(failure.clone());
        job.retry().unwrap();
        assert_eq!(job.status, HarnessJobStatus::Pending);
        assert!(job.error.is_none());

        let prompt = generation_prompt(&job, &dirs).unwrap();

        assert!(prompt.contains("[previous harness failure]"));
        assert!(prompt.contains(&failure));
        assert!(prompt.contains("[previous rejected delta]"));
        assert!(prompt.contains("\"new_text\": \"Final behavior.\""));
        assert!(prompt.contains("Exact edits are applied in order"));

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn delta_rejects_mixed_create_and_edit_or_non_document_paths() {
        let mixed = HarnessDelta {
            summary: "summary".to_string(),
            documents: vec![HarnessDocumentPatch {
                path: "specs/game.md".to_string(),
                create: Some("new".to_string()),
                edits: vec![ExactTextEdit {
                    old_text: "old".to_string(),
                    new_text: "new".to_string(),
                }],
            }],
            memory_updates: Vec::new(),
            promoted_fact_ids: Vec::new(),
        };
        assert!(validate_delta(&mixed).is_err());

        let unsafe_path = HarnessDelta {
            summary: "summary".to_string(),
            documents: vec![HarnessDocumentPatch {
                path: "plans/approved.md".to_string(),
                create: Some("replacement".to_string()),
                edits: Vec::new(),
            }],
            memory_updates: Vec::new(),
            promoted_fact_ids: Vec::new(),
        };
        assert!(validate_delta(&unsafe_path).is_err());
    }

    #[test]
    fn structured_delta_stages_spec_and_server_worklog_as_separate_review() {
        let base = unique_temp_dir("stage");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let manager = WorkspaceManager::new(dirs.clone());
        let canonical = manager
            .prepare_snapshot(&crate::bridge_io::EpsSnapshot {
                project: "Project".to_string(),
                identity: "C:/maps/project.scx".to_string(),
                files: Vec::new(),
            })
            .unwrap();
        fs::write(
            canonical.root.join("specs/game.md"),
            "# Gameplay\n\nOld behavior.\n",
        )
        .unwrap();
        let mut job = HarnessJob::new(
            "session".to_string(),
            "Project".to_string(),
            canonical.id.clone(),
            "req-code".to_string(),
            "Change behavior".to_string(),
            None,
            "Build passed".to_string(),
            vec![entry(WriteTool::FileWrite)],
            Some(BuildEvidence {
                ok: true,
                error_count: 0,
            }),
        );
        job.runtime_verification = RuntimeVerification::Confirmed;
        let delta = HarnessDelta {
            summary: "Changed gameplay behavior.".to_string(),
            documents: vec![HarnessDocumentPatch {
                path: "specs/game.md".to_string(),
                create: None,
                edits: vec![ExactTextEdit {
                    old_text: "Old behavior.".to_string(),
                    new_text: "Accepted behavior.".to_string(),
                }],
            }],
            memory_updates: Vec::new(),
            promoted_fact_ids: Vec::new(),
        };
        let journal = JournalStore::new(dirs.app_data());

        let count = stage_delta(&dirs, journal.clone(), &mut job, delta).unwrap();
        assert_eq!(count, 2);
        let request_id = job.harness_request_id.as_deref().unwrap();
        let changeset = journal.changeset(request_id).unwrap();
        assert_eq!(changeset.items.len(), 2);
        let workspace_root = dirs
            .session_workspaces_dir()
            .join(&canonical.id)
            .join(&job.workspace_session_id);
        assert!(fs::read_to_string(workspace_root.join("specs/game.md"))
            .unwrap()
            .contains("Accepted behavior."));
        let worklog = fs::read_to_string(workspace_root.join("worklog/req-code.md")).unwrap();
        assert!(worklog.contains("confirmed by the user"));
        assert!(worklog.contains("../specs/game.md"));

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn accepted_harness_outputs_produce_hashed_promotion_audit_only_for_named_facts() {
        let base = unique_temp_dir("promotion-audit");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let workspace_id = "a".repeat(64);
        let document = dirs
            .workspaces_dir()
            .join(&workspace_id)
            .join("specs/game.md");
        fs::create_dir_all(document.parent().unwrap()).unwrap();
        fs::write(&document, "Accepted behavior.").unwrap();
        let memory = ProjectMemory::new(dirs.memory_dir(), "Project");
        assert!(memory.write("resources", "Enemy roster = 10").ok);

        let mut job = HarnessJob::new(
            "session".to_string(),
            "Project".to_string(),
            workspace_id,
            "req-code".to_string(),
            "Change behavior".to_string(),
            None,
            "Done".to_string(),
            vec![entry(WriteTool::SettingsSet)],
            None,
        );
        job.task_state_promotion = Some(crate::task_state::TaskStatePromotionInput {
            source_task_revision: 2,
            source_event_id: "event-source".to_string(),
            fact_ids: vec!["enemy-roster".to_string()],
            candidates: vec![crate::task_state::PromotionCandidate {
                id: "enemy-roster".to_string(),
                category: "target_set".to_string(),
                text: "All 10 enemies".to_string(),
                provenance: vec![crate::task_state::Provenance::UserTurn {
                    client_turn_id: "11111111-1111-4111-8111-111111111111".to_string(),
                    exact_quote: "all ten enemies".to_string(),
                }],
            }],
        });
        job.delta = Some(HarnessDelta {
            summary: "Synced accepted behavior.".to_string(),
            documents: vec![HarnessDocumentPatch {
                path: "specs/game.md".to_string(),
                create: None,
                edits: vec![ExactTextEdit {
                    old_text: "Old behavior.".to_string(),
                    new_text: "Accepted behavior.".to_string(),
                }],
            }],
            memory_updates: vec![HarnessMemoryUpdate {
                file: "resources".to_string(),
                content: "Enemy roster = 10".to_string(),
            }],
            promoted_fact_ids: vec!["enemy-roster".to_string()],
        });

        let audit = task_state_promotion_audit(&dirs, &job, true)
            .unwrap()
            .unwrap();
        assert_eq!(audit.fact_ids, vec!["enemy-roster"]);
        assert_eq!(audit.document_refs[0].path, "specs/game.md");
        assert_eq!(audit.document_refs[0].sha256.len(), 64);
        assert_eq!(audit.memory_refs[0].path, "resources.md");
        assert_eq!(audit.memory_refs[0].sha256.len(), 64);

        let rejected = task_state_promotion_audit(&dirs, &job, false)
            .unwrap()
            .unwrap();
        assert!(!rejected.accepted);
        assert!(rejected.fact_ids.is_empty());
        assert!(rejected.document_refs.is_empty());
        assert!(rejected.memory_refs.is_empty());
        fs::remove_dir_all(base).ok();
    }
    #[test]
    fn completed_or_failed_jobs_can_be_dismissed_but_active_jobs_cannot() {
        for status in [
            HarnessJobStatus::Completed,
            HarnessJobStatus::Failed,
            HarnessJobStatus::Rejected,
            HarnessJobStatus::Skipped,
        ] {
            let mut job = HarnessJob::new(
                "session".to_string(),
                "Project".to_string(),
                "a".repeat(64),
                "req-code".to_string(),
                "Change runtime behavior".to_string(),
                None,
                "Done".to_string(),
                vec![entry(WriteTool::FileWrite)],
                None,
            );
            job.status = status;
            job.dismiss().unwrap();
            assert!(job.dismissed);
        }

        let mut active = HarnessJob::new(
            "session".to_string(),
            "Project".to_string(),
            "a".repeat(64),
            "req-code".to_string(),
            "Change runtime behavior".to_string(),
            None,
            "Done".to_string(),
            vec![entry(WriteTool::FileWrite)],
            None,
        );
        assert!(active.dismiss().is_err());
        assert!(!active.dismissed);
    }

    #[test]
    fn durable_job_store_recovers_interrupted_generation_as_retryable_failure() {
        let base = unique_temp_dir("recover");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let store = HarnessJobStore::new(dirs.clone());
        let mut job = HarnessJob::new(
            "session".to_string(),
            "Project".to_string(),
            "a".repeat(64),
            "req-code".to_string(),
            "Change setting".to_string(),
            None,
            "Done".to_string(),
            vec![entry(WriteTool::SettingsSet)],
            None,
        );
        assert_eq!(job.status, HarnessJobStatus::Pending);
        job.status = HarnessJobStatus::Running;
        store.create(&job).unwrap();

        let recovered = store.recover_interrupted("session").unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].status, HarnessJobStatus::Failed);
        assert!(recovered[0]
            .error
            .as_deref()
            .is_some_and(|message| message.contains("다시 시도")));
        assert_eq!(
            store.load(&job.id).unwrap().status,
            HarnessJobStatus::Failed
        );

        fs::remove_dir_all(base).ok();
    }
}
