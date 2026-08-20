//! Per-project Codex filesystem workspace.
//!
//! A project workspace is a real Codex cwd under `%appdata%\eud-agent\workspaces`.
//! Agent-authored documents are writable and reviewed after each turn; `source/` is a
//! coherent editor EPSNAPSHOT mirror and is read-only under Codex's split filesystem
//! permission profile. Trusted baselines live in the sibling `.state/` directory, outside
//! the Codex cwd, so the model cannot rewrite rollback data.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bridge_io::{BridgeIo, EpsSnapshot, HEARTBEAT_STALE_AFTER};
use crate::config::{self, DataDirs};
use crate::journal::{JournalEntry, JournalStore, JournalTarget, Snapshot, WriteTool};
use crate::memory::write_atomic_bytes;

pub const SOURCE_DIR: &str = "source";
pub const TEMP_DIR: &str = ".tmp";
const CODEGRAPH_RUNTIME_PATH: &str = ".codegraph";
const BASELINES_DIR: &str = "baselines";
const BASELINE_MARKER: &str = ".baseline";
const MAX_FILES: usize = 2_048;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;

const DOCUMENT_DIRS: [&str; 4] = ["specs", "plans", "decisions", "worklog"];
pub const SPEC_INDEX_PATH: &str = "specs/index.md";

pub fn approved_plan_path(request_id: &str) -> io::Result<String> {
    let request_id = normalize_token(request_id, "request id")?;
    Ok(format!("plans/{request_id}.md"))
}

pub fn completion_worklog_path(request_id: &str) -> io::Result<String> {
    let request_id = normalize_token(request_id, "request id")?;
    Ok(format!("worklog/{request_id}.md"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedWorkspace {
    pub id: String,
    pub project: String,
    pub root: PathBuf,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceBaseline {
    pub request_id: String,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub workspace_root: PathBuf,
    pub baseline_root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceChange {
    pub path: String,
    pub kind: WorkspaceChangeKind,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileEntry {
    pub path: String,
    pub source: bool,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustedWorkspaceState {
    version: u32,
    id: String,
    project: String,
    identity_hash: String,
    #[serde(default)]
    documents: BTreeMap<String, TrustedDocumentState>,
    #[serde(default)]
    approved_plans: BTreeMap<String, TrustedPlanState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustedDocumentState {
    revision: u64,
    state: String,
    accepted_at: u64,
    request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustedPlanState {
    revision: u32,
    approved_at: u64,
    markdown_sha256: String,
}

#[derive(Debug, Clone)]
pub struct WorkspaceManager {
    dirs: DataDirs,
}

impl WorkspaceManager {
    pub fn new(dirs: DataDirs) -> Self {
        Self { dirs }
    }

    /// Resolve the configured bridge and build the current project's workspace.
    pub fn prepare_current(&self) -> Result<PreparedWorkspace, String> {
        let config = self.dirs.load_config().map_err(|error| error.to_string())?;
        let editor_path = config.editor_path.trim();
        if editor_path.is_empty() {
            return Err("editor path is not configured".to_string());
        }
        let bridge = BridgeIo::new(config::editor_ipc_dir(Path::new(editor_path)));
        bridge
            .read_status_snapshot(HEARTBEAT_STALE_AFTER)
            .map_err(|error| error.to_string())?;
        let snapshot = bridge
            .snapshot_eps(&Default::default(), None)
            .map_err(|error| error.to_string())?;
        self.prepare_snapshot(&snapshot)
            .map_err(|error| error.to_string())
    }

    /// Create the durable document directories and atomically refresh `source/`.
    pub fn prepare_snapshot(&self, snapshot: &EpsSnapshot) -> io::Result<PreparedWorkspace> {
        if snapshot.project.trim().is_empty() || snapshot.identity.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace snapshot has no project identity",
            ));
        }

        let id = project_id(&snapshot.identity);
        let root = self.workspace_root(&id)?;
        fs::create_dir_all(&root)?;
        ensure_plain_directory(&root)?;
        for directory in DOCUMENT_DIRS {
            let path = root.join(directory);
            fs::create_dir_all(&path)?;
            ensure_plain_directory(&path)?;
        }
        let temp = root.join(TEMP_DIR);
        fs::create_dir_all(&temp)?;
        ensure_plain_directory(&temp)?;

        self.refresh_source(&root, &id, snapshot)?;
        let mut trusted = self.load_state(&id)?;
        trusted.version = 1;
        trusted.id = id.clone();
        trusted.project = snapshot.project.clone();
        trusted.identity_hash = project_id(&snapshot.identity);
        self.save_state(&trusted)?;

        Ok(PreparedWorkspace {
            id,
            project: snapshot.project.clone(),
            root,
            session_id: None,
        })
    }

    /// Prepare one session-owned Codex cwd from the latest coherent editor
    /// snapshot and the canonical accepted document tree.
    pub fn prepare_session_current(&self, session_id: &str) -> Result<PreparedWorkspace, String> {
        let config = self.dirs.load_config().map_err(|error| error.to_string())?;
        let editor_path = config.editor_path.trim();
        if editor_path.is_empty() {
            return Err("editor path is not configured".to_string());
        }
        let bridge = BridgeIo::new(config::editor_ipc_dir(Path::new(editor_path)));
        bridge
            .read_status_snapshot(HEARTBEAT_STALE_AFTER)
            .map_err(|error| error.to_string())?;
        let snapshot = bridge
            .snapshot_eps(&Default::default(), None)
            .map_err(|error| error.to_string())?;
        self.prepare_session_snapshot(&snapshot, session_id)
            .map_err(|error| error.to_string())
    }

    pub fn prepare_session_snapshot(
        &self,
        snapshot: &EpsSnapshot,
        session_id: &str,
    ) -> io::Result<PreparedWorkspace> {
        let canonical = self.prepare_snapshot(snapshot)?;
        let session_id = normalize_token(session_id, "session id")?;
        let root = self.session_workspace_root(&canonical.id, &session_id)?;
        fs::create_dir_all(&root)?;
        ensure_plain_directory(&root)?;
        for directory in DOCUMENT_DIRS {
            let path = root.join(directory);
            fs::create_dir_all(&path)?;
            ensure_plain_directory(&path)?;
        }
        let temp = root.join(TEMP_DIR);
        fs::create_dir_all(&temp)?;
        ensure_plain_directory(&temp)?;
        sync_documents(&canonical.root, &root)?;
        self.refresh_source(&root, &canonical.id, snapshot)?;
        Ok(PreparedWorkspace {
            id: canonical.id,
            project: canonical.project,
            root,
            session_id: Some(session_id),
        })
    }

    /// Capture a crash-safe baseline outside the writable Codex cwd.
    ///
    /// Re-entering the same request (session-resume fallback) reuses the original baseline,
    /// so changes from a timed-out first attempt do not silently become accepted state.
    pub fn begin_turn(
        &self,
        workspace: &PreparedWorkspace,
        request_id: &str,
    ) -> io::Result<WorkspaceBaseline> {
        let request_id = normalize_token(request_id, "request id")?;
        let baseline_root = self
            .dirs
            .workspace_state_dir()
            .join(BASELINES_DIR)
            .join(&request_id)
            .join(&workspace.id);
        if !baseline_root.join(BASELINE_MARKER).is_file() {
            if baseline_root.exists() {
                fs::remove_dir_all(&baseline_root)?;
            }
            let parent = baseline_root.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "baseline has no parent")
            })?;
            fs::create_dir_all(parent)?;
            let staged = parent.join(format!(".stage-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&staged)?;
            let snapshot = scan_text_tree(&workspace.root, ScanMode::Writable)?;
            write_tree(&staged, &snapshot)?;
            atomic_write(&staged.join(BASELINE_MARKER), b"1")?;
            fs::rename(staged, &baseline_root)?;
        }

        Ok(WorkspaceBaseline {
            request_id,
            workspace_id: workspace.id.clone(),
            session_id: workspace.session_id.clone(),
            workspace_root: workspace.root.clone(),
            baseline_root,
        })
    }

    /// Diff the current writable tree against the trusted turn baseline.
    pub fn changes(&self, baseline: &WorkspaceBaseline) -> io::Result<Vec<WorkspaceChange>> {
        let before = scan_text_tree(&baseline.baseline_root, ScanMode::Baseline)?;
        let after = scan_text_tree(&baseline.workspace_root, ScanMode::Writable)?;
        let paths: BTreeSet<_> = before.keys().chain(after.keys()).cloned().collect();
        let mut changes = Vec::new();
        for path in paths {
            let old = before.get(&path);
            let new = after.get(&path);
            if old == new {
                continue;
            }
            let kind = match (old, new) {
                (None, Some(_)) => WorkspaceChangeKind::Created,
                (Some(_), None) => WorkspaceChangeKind::Deleted,
                (Some(_), Some(_)) => WorkspaceChangeKind::Modified,
                (None, None) => continue,
            };
            changes.push(WorkspaceChange {
                path,
                kind,
                before: old.cloned(),
                after: new.cloned(),
            });
        }
        Ok(changes)
    }

    pub fn finish_turn(&self, baseline: &WorkspaceBaseline) -> io::Result<()> {
        if baseline.baseline_root.exists() {
            fs::remove_dir_all(&baseline.baseline_root)?;
        }
        if let Some(request_dir) = baseline.baseline_root.parent() {
            if request_dir.is_dir() && fs::read_dir(request_dir)?.next().is_none() {
                fs::remove_dir(request_dir)?;
            }
        }
        Ok(())
    }

    pub fn list_files(&self, workspace: &PreparedWorkspace) -> io::Result<Vec<WorkspaceFileEntry>> {
        let files = scan_files(&workspace.root, ScanMode::All)?;
        let state = self.load_state(&workspace.id)?;
        Ok(files
            .into_iter()
            .map(|(path, size)| {
                let source = path == SOURCE_DIR || path.starts_with("source/");
                let trusted = (!source).then(|| state.documents.get(&path)).flatten();
                let approved_plan = approved_plan_for_path(&state.approved_plans, &path);
                WorkspaceFileEntry {
                    source,
                    state: approved_plan
                        .map(|_| "approved".to_string())
                        .or_else(|| trusted.map(|entry| entry.state.clone())),
                    revision: approved_plan
                        .map(|plan| u64::from(plan.revision))
                        .or_else(|| trusted.map(|entry| entry.revision)),
                    path,
                    size,
                }
            })
            .collect())
    }

    /// Persist authoritative acceptance metadata outside the Codex cwd.
    pub fn record_accepted_entries(
        &self,
        request_id: &str,
        entries: &[JournalEntry],
    ) -> io::Result<()> {
        let promoted = self.promote_entries(entries)?;
        let metadata = (|| {
            let mut grouped = BTreeMap::<String, BTreeMap<String, String>>::new();
            let mut ordered = entries.iter().collect::<Vec<_>>();
            ordered.sort_by_key(|entry| entry.seq);
            for entry in ordered {
                let JournalTarget::WorkspacePath {
                    workspace_id, path, ..
                } = &entry.target
                else {
                    continue;
                };
                let state = if matches!(entry.tool, WriteTool::WorkspaceDelete) {
                    "deleted"
                } else {
                    "accepted"
                };
                grouped
                    .entry(workspace_id.clone())
                    .or_default()
                    .insert(path.clone(), state.to_string());
            }

            for (workspace_id, documents) in grouped {
                let mut state = self.load_state(&workspace_id)?;
                for (path, accepted_state) in documents {
                    let revision = match state.documents.get(&path) {
                        Some(entry) if entry.request_id == request_id => entry.revision,
                        Some(entry) => entry.revision.saturating_add(1),
                        None => 1,
                    };
                    state.documents.insert(
                        path,
                        TrustedDocumentState {
                            revision,
                            state: accepted_state,
                            accepted_at: epoch_seconds(),
                            request_id: request_id.to_string(),
                        },
                    );
                }
                self.save_state(&state)?;
            }
            Ok::<(), io::Error>(())
        })();
        if let Err(error) = metadata {
            if let Err(rollback) = restore_promotions(&promoted) {
                return Err(io::Error::other(format!(
                    "acceptance metadata failed: {error}; canonical rollback failed: {rollback}"
                )));
            }
            return Err(error);
        }
        Ok(())
    }

    /// Persist the exact user-approved plan in the durable workspace and record its
    /// authoritative approval metadata outside the model-writable root.
    ///
    /// This app-owned write happens between Codex turns, before the execution baseline,
    /// so rejecting implementation changes never deletes the approved plan history.
    pub fn record_plan_approval(
        &self,
        workspace_id: &str,
        request_id: &str,
        revision: u32,
        markdown: &str,
    ) -> io::Result<()> {
        let plan_relative = approved_plan_path(request_id)?;
        let root = self.workspace_root(workspace_id)?;
        ensure_plain_directory(&root)?;
        ensure_plain_directory(&root.join("plans"))?;
        let plan_path = confined_path(&root, &plan_relative, false)?;
        let previous = optional_regular_file_bytes(&plan_path)?;
        let mut state = self.load_state(workspace_id)?;
        state.approved_plans.insert(
            request_id.to_string(),
            TrustedPlanState {
                revision,
                approved_at: epoch_seconds(),
                markdown_sha256: project_id(markdown),
            },
        );

        atomic_write(&plan_path, markdown.as_bytes())?;
        if let Err(save_error) = self.save_state(&state) {
            if let Err(rollback_error) =
                restore_optional_file_bytes(&plan_path, previous.as_deref())
            {
                return Err(io::Error::other(format!(
                    "approval metadata write failed: {save_error}; plan rollback failed: {rollback_error}"
                )));
            }
            return Err(save_error);
        }
        Ok(())
    }

    /// Validate the durable project-wiki postconditions for one approved-plan execution.
    ///
    /// The approved plan is immutable. `specs/index.md` must link to a non-empty topic
    /// spec, and the request worklog must link back to a canonical topic spec. Returned
    /// strings are corrective instructions suitable for the Codex repair turn.
    pub fn completion_doc_gaps(
        &self,
        workspace_id: &str,
        request_id: &str,
        approved_markdown: &str,
    ) -> io::Result<Vec<String>> {
        let root = self.workspace_root(workspace_id)?;
        self.completion_doc_gaps_at(&root, request_id, approved_markdown)
    }

    pub fn completion_doc_gaps_for_workspace(
        &self,
        workspace: &PreparedWorkspace,
        request_id: &str,
        approved_markdown: &str,
    ) -> io::Result<Vec<String>> {
        self.completion_doc_gaps_at(&workspace.root, request_id, approved_markdown)
    }

    fn completion_doc_gaps_at(
        &self,
        root: &Path,
        request_id: &str,
        approved_markdown: &str,
    ) -> io::Result<Vec<String>> {
        let plan_path = approved_plan_path(request_id)?;
        let worklog_path = completion_worklog_path(request_id)?;
        let files = scan_text_tree(root, ScanMode::Writable)?;
        let mut gaps = Vec::new();

        match files.get(&plan_path) {
            None => gaps.push(format!(
                "`{plan_path}` is missing; restore the exact user-approved plan."
            )),
            Some(content) if content != approved_markdown => gaps.push(format!(
                "`{plan_path}` differs from the user-approved plan; restore it exactly."
            )),
            Some(_) => {}
        }

        let topic_specs = files
            .iter()
            .filter_map(|(path, content)| {
                (path.starts_with("specs/")
                    && path != SPEC_INDEX_PATH
                    && path.ends_with(".md")
                    && !content.trim().is_empty())
                .then_some(path.as_str())
            })
            .collect::<BTreeSet<_>>();

        match files.get(SPEC_INDEX_PATH) {
            None => gaps.push(format!(
                "`{SPEC_INDEX_PATH}` is missing; create the canonical project-wiki index."
            )),
            Some(content) if content.trim().is_empty() => gaps.push(format!(
                "`{SPEC_INDEX_PATH}` is empty; summarize the project and link its topic specs."
            )),
            Some(content) if !markdown_links_to_any(content, SPEC_INDEX_PATH, &topic_specs) => {
                gaps.push(format!(
                    "`{SPEC_INDEX_PATH}` must link to at least one non-empty topic page under `specs/`."
                ));
            }
            Some(_) => {}
        }

        match files.get(&worklog_path) {
            None => gaps.push(format!(
                "`{worklog_path}` is missing; record the actual result, verification, and canonical specs."
            )),
            Some(content) if content.trim().is_empty() => gaps.push(format!(
                "`{worklog_path}` is empty; record the actual result, verification, and canonical specs."
            )),
            Some(content) if !markdown_links_to_any(content, &worklog_path, &topic_specs) => {
                gaps.push(format!(
                    "`{worklog_path}` must link to at least one canonical topic page under `specs/`."
                ));
            }
            Some(_) => {}
        }

        Ok(gaps)
    }

    pub fn read_file(&self, workspace_id: &str, relative: &str) -> io::Result<String> {
        let root = self.workspace_root(workspace_id)?;
        let path = confined_path(&root, relative, true)?;
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "workspace viewer only reads regular files",
            ));
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace file exceeds the viewer size limit",
            ));
        }
        let bytes = fs::read(path)?;
        decode_utf8(bytes, relative)
    }

    /// Restore or remove one writable file during changeset rejection.
    pub fn restore_file(
        &self,
        workspace_id: &str,
        session_id: Option<&str>,
        relative: &str,
        content: Option<&str>,
    ) -> io::Result<()> {
        let root = match session_id {
            Some(session_id) => self.session_workspace_root(workspace_id, session_id)?,
            None => self.workspace_root(workspace_id)?,
        };
        let path = confined_path(&root, relative, false)?;
        match content {
            Some(content) => atomic_write(&path, content.as_bytes()),
            None => match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        }
    }

    fn promote_entries(
        &self,
        entries: &[JournalEntry],
    ) -> io::Result<Vec<(PathBuf, Option<Vec<u8>>)>> {
        let mut pending = BTreeMap::<PathBuf, (Option<Vec<u8>>, Option<Vec<u8>>)>::new();
        let mut ordered = entries.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|entry| entry.seq);
        for entry in ordered {
            let JournalTarget::WorkspacePath {
                workspace_id,
                session_id: Some(_),
                path,
            } = &entry.target
            else {
                continue;
            };
            let canonical_root = self.workspace_root(workspace_id)?;
            let canonical_path = confined_path(&canonical_root, path, false)?;
            let trusted = self.load_state(workspace_id)?;
            if let Some(plan) = approved_plan_for_path(&trusted.approved_plans, path) {
                let content = match &entry.after {
                    Snapshot::FileContent { content } => content,
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            format!("approved plan `{path}` is immutable"),
                        ))
                    }
                };
                if project_id(content) != plan.markdown_sha256 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("approved plan `{path}` is immutable"),
                    ));
                }
                continue;
            }
            let after = match &entry.after {
                Snapshot::FileContent { content } => Some(content.as_bytes().to_vec()),
                Snapshot::Deleted => None,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("workspace journal `{path}` has no promotable after snapshot"),
                    ))
                }
            };
            let before = match pending.get(&canonical_path) {
                Some((before, _)) => before.clone(),
                None => optional_regular_file_bytes(&canonical_path)?,
            };
            pending.insert(canonical_path, (before, after));
        }

        let mut promoted = Vec::new();
        for (path, (before, after)) in pending {
            let applied = restore_optional_file_bytes(&path, after.as_deref());
            if let Err(error) = applied {
                if let Err(rollback) = restore_promotions(&promoted) {
                    return Err(io::Error::other(format!(
                        "canonical promotion failed: {error}; rollback failed: {rollback}"
                    )));
                }
                return Err(error);
            }
            promoted.push((path, before));
        }
        Ok(promoted)
    }

    fn workspace_root(&self, workspace_id: &str) -> io::Result<PathBuf> {
        let workspace_id = normalize_workspace_id(workspace_id)?;
        Ok(self.dirs.workspaces_dir().join(workspace_id))
    }

    fn session_workspace_root(&self, workspace_id: &str, session_id: &str) -> io::Result<PathBuf> {
        let workspace_id = normalize_workspace_id(workspace_id)?;
        let session_id = normalize_token(session_id, "session id")?;
        Ok(self
            .dirs
            .session_workspaces_dir()
            .join(workspace_id)
            .join(session_id))
    }

    fn state_path(&self, workspace_id: &str) -> io::Result<PathBuf> {
        let workspace_id = normalize_workspace_id(workspace_id)?;
        Ok(self
            .dirs
            .workspace_state_dir()
            .join("projects")
            .join(format!("{workspace_id}.json")))
    }

    fn load_state(&self, workspace_id: &str) -> io::Result<TrustedWorkspaceState> {
        let path = self.state_path(workspace_id)?;
        if !path.is_file() {
            return Ok(TrustedWorkspaceState::default());
        }
        let bytes = fs::read(path)?;
        let state: TrustedWorkspaceState = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if !state.id.is_empty() && state.id != workspace_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "trusted workspace state id does not match its file",
            ));
        }
        Ok(state)
    }

    fn save_state(&self, state: &TrustedWorkspaceState) -> io::Result<()> {
        let path = self.state_path(&state.id)?;
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        atomic_write(&path, &bytes)
    }

    fn refresh_source(
        &self,
        workspace_root: &Path,
        workspace_id: &str,
        snapshot: &EpsSnapshot,
    ) -> io::Result<()> {
        let stage_parent = self
            .dirs
            .workspace_state_dir()
            .join("source-staging")
            .join(workspace_id);
        fs::create_dir_all(&stage_parent)?;
        let staged = stage_parent.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&staged)?;

        let populate = (|| -> io::Result<()> {
            let mut seen = BTreeSet::new();
            for file in &snapshot.files {
                let relative = normalize_relative_path(&file.path, true)?;
                let key = relative.to_lowercase();
                if !seen.insert(key) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("source mirror path collision: {relative}"),
                    ));
                }
                let Some(content) = &file.content else {
                    continue;
                };
                let target = confined_path(&staged, &relative, true)?;
                atomic_write(&target, content.as_bytes())?;
            }
            Ok(())
        })();
        if let Err(error) = populate {
            let _ = fs::remove_dir_all(&staged);
            return Err(error);
        }

        let source = workspace_root.join(SOURCE_DIR);
        if source.exists() {
            ensure_plain_directory(&source)?;
            fs::remove_dir_all(&source)?;
        }
        fs::rename(&staged, &source)?;
        ensure_plain_directory(&source)
    }
}

/// RAII turn recorder: every exit path, including timeout cancellation, diffs
/// the real workspace and journals writable-file changes before releasing the
/// trusted baseline.
pub struct WorkspaceTurnRecorder {
    manager: WorkspaceManager,
    baseline: Option<WorkspaceBaseline>,
    journal: JournalStore,
}

impl WorkspaceTurnRecorder {
    pub fn new(
        manager: WorkspaceManager,
        baseline: WorkspaceBaseline,
        journal: JournalStore,
    ) -> Self {
        Self {
            manager,
            baseline: Some(baseline),
            journal,
        }
    }

    pub fn finish(&mut self) -> io::Result<usize> {
        let Some(baseline) = self.baseline.take() else {
            return Ok(0);
        };
        let changes = self.manager.changes(&baseline)?;
        let mut seq = self.journal.entry_count(&baseline.request_id) as u64;
        for change in &changes {
            seq = seq.checked_add(1).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "workspace journal sequence overflow",
                )
            })?;
            let (tool, before, after) = match change.kind {
                WorkspaceChangeKind::Created => (
                    WriteTool::WorkspaceCreate,
                    Snapshot::Created,
                    Snapshot::FileContent {
                        content: change.after.clone().unwrap_or_default(),
                    },
                ),
                WorkspaceChangeKind::Modified => (
                    WriteTool::WorkspaceWrite,
                    Snapshot::FileContent {
                        content: change.before.clone().unwrap_or_default(),
                    },
                    Snapshot::FileContent {
                        content: change.after.clone().unwrap_or_default(),
                    },
                ),
                WorkspaceChangeKind::Deleted => (
                    WriteTool::WorkspaceDelete,
                    Snapshot::DeletedFile {
                        content: change.before.clone().unwrap_or_default(),
                        position: None,
                    },
                    Snapshot::Deleted,
                ),
            };
            self.journal
                .record(
                    &baseline.request_id,
                    JournalEntry {
                        id: format!("workspace-{seq}"),
                        seq,
                        tool,
                        target: JournalTarget::WorkspacePath {
                            workspace_id: baseline.workspace_id.clone(),
                            session_id: baseline.session_id.clone(),
                            path: change.path.clone(),
                        },
                        before,
                        after,
                        ts: epoch_seconds(),
                    },
                )
                .map_err(io::Error::other)?;
        }
        if !changes.is_empty() {
            self.journal
                .persist(&baseline.request_id)
                .map_err(io::Error::other)?;
        }
        self.manager.finish_turn(&baseline)?;
        Ok(changes.len())
    }
}

impl Drop for WorkspaceTurnRecorder {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            eprintln!("eud-agent: workspace turn finalization failed: {error}");
        }
    }
}

fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn project_id(identity: &str) -> String {
    let digest = Sha256::digest(identity.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn normalize_workspace_id(value: &str) -> io::Result<String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "workspace id must be a 64-character hex digest",
        ));
    }
    Ok(value.to_ascii_lowercase())
}

fn normalize_token(value: &str, label: &str) -> io::Result<String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} contains unsafe characters"),
        ));
    }
    Ok(value.to_string())
}

fn approved_plan_for_path<'a>(
    plans: &'a BTreeMap<String, TrustedPlanState>,
    path: &str,
) -> Option<&'a TrustedPlanState> {
    let request_id = path.strip_prefix("plans/")?.strip_suffix(".md")?;
    (!request_id.is_empty() && !request_id.contains('/'))
        .then(|| plans.get(request_id))
        .flatten()
}

fn optional_regular_file_bytes(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "approved plan path is not a regular file",
        ));
    }
    fs::read(path).map(Some)
}

fn restore_optional_file_bytes(path: &Path, content: Option<&[u8]>) -> io::Result<()> {
    match content {
        Some(content) => atomic_write(path, content),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
    }
}

fn restore_promotions(promoted: &[(PathBuf, Option<Vec<u8>>)]) -> io::Result<()> {
    for (path, before) in promoted.iter().rev() {
        restore_optional_file_bytes(path, before.as_deref())?;
    }
    Ok(())
}

fn markdown_links_to_any(markdown: &str, source_path: &str, candidates: &BTreeSet<&str>) -> bool {
    markdown_link_destinations(markdown)
        .into_iter()
        .filter_map(|destination| resolve_markdown_destination(source_path, destination))
        .any(|target| candidates.contains(target.as_str()))
}

fn markdown_link_destinations(markdown: &str) -> Vec<&str> {
    let mut destinations = Vec::new();
    let mut remaining = markdown;
    while let Some(open) = remaining.find("](") {
        let after_open = &remaining[open + 2..];
        let Some(close) = after_open.find(')') else {
            break;
        };
        destinations.push(after_open[..close].trim());
        remaining = &after_open[close + 1..];
    }
    destinations
}

fn resolve_markdown_destination(source_path: &str, destination: &str) -> Option<String> {
    let destination = destination.trim();
    let destination = if let Some(inner) = destination.strip_prefix('<') {
        inner.split_once('>')?.0
    } else {
        destination.split_ascii_whitespace().next()?
    };
    let destination = destination.split(['#', '?']).next()?;
    if destination.is_empty()
        || destination.starts_with('/')
        || destination.contains('\\')
        || destination.contains(':')
    {
        return None;
    }

    let mut segments = source_path
        .rsplit_once('/')
        .map(|(parent, _)| parent.split('/').collect::<Vec<_>>())
        .unwrap_or_default();
    for segment in destination.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            segment => segments.push(segment),
        }
    }
    (!segments.is_empty()).then(|| segments.join("/"))
}

fn normalize_relative_path(value: &str, allow_source: bool) -> io::Result<String> {
    if value.is_empty() || value.contains('\0') || value.contains('\\') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "workspace path must be relative and use '/' separators",
        ));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || value.starts_with('/')
        || value
            .as_bytes()
            .get(1)
            .is_some_and(|byte| *byte == b':' && value.as_bytes()[0].is_ascii_alphabetic())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "workspace path must be relative",
        ));
    }
    let mut segments = Vec::new();
    for component in path.components() {
        let Component::Normal(segment) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace path is not normalized",
            ));
        };
        let segment = segment.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "workspace path is not UTF-8")
        })?;
        if segment.is_empty() || segment.contains(':') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace path contains an unsafe segment",
            ));
        }
        segments.push(segment);
    }
    if segments.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "workspace path is empty",
        ));
    }
    if segments
        .first()
        .is_some_and(|segment| *segment == CODEGRAPH_RUNTIME_PATH)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "runtime workspace paths are internal",
        ));
    }
    if !allow_source
        && segments
            .first()
            .is_some_and(|segment| *segment == SOURCE_DIR || *segment == TEMP_DIR)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "generated workspace paths are read-only",
        ));
    }
    Ok(segments.join("/"))
}

fn confined_path(root: &Path, relative: &str, allow_source: bool) -> io::Result<PathBuf> {
    let relative = normalize_relative_path(relative, allow_source)?;
    let target = relative
        .split('/')
        .fold(root.to_path_buf(), |path, segment| path.join(segment));
    if target.starts_with(root) {
        Ok(target)
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "workspace path escapes its root",
        ))
    }
}

fn ensure_plain_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "workspace path is not a plain directory: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanMode {
    All,
    Writable,
    Baseline,
}

fn scan_text_tree(root: &Path, mode: ScanMode) -> io::Result<BTreeMap<String, String>> {
    let files = scan_files(root, mode)?;
    let mut output = BTreeMap::new();
    for (relative, _) in files {
        let bytes = fs::read(confined_path(root, &relative, true)?)?;
        let content = decode_utf8(bytes, &relative)?;
        output.insert(relative, content);
    }
    Ok(output)
}

fn scan_files(root: &Path, mode: ScanMode) -> io::Result<BTreeMap<String, u64>> {
    if !root.is_dir() {
        return Ok(BTreeMap::new());
    }
    ensure_plain_directory(root)?;
    let mut files = BTreeMap::new();
    let mut total = 0_u64;
    scan_directory(root, root, mode, &mut files, &mut total)?;
    Ok(files)
}

fn scan_directory(
    root: &Path,
    current: &Path,
    mode: ScanMode,
    files: &mut BTreeMap<String, u64>,
    total: &mut u64,
) -> io::Result<()> {
    let mut entries = fs::read_dir(current)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "scan escaped root"))?
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let top = relative.split('/').next().unwrap_or_default();
        // Codex CodeGraph creates this top-level link to its external index. It is runtime
        // metadata, never a project document; skip it before symlink validation so normal
        // anti-escape checks remain strict for every user-visible workspace path.
        if top == CODEGRAPH_RUNTIME_PATH {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "workspace symlinks are forbidden: {}",
                    entry.path().display()
                ),
            ));
        }
        let skip = match mode {
            ScanMode::All => top == TEMP_DIR,
            ScanMode::Writable => top == SOURCE_DIR || top == TEMP_DIR,
            ScanMode::Baseline => relative == BASELINE_MARKER,
        };
        if skip {
            continue;
        }
        if metadata.is_dir() {
            scan_directory(root, &entry.path(), mode, files, total)?;
        } else if metadata.is_file() {
            if metadata.len() > MAX_FILE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("workspace file exceeds 1 MiB: {relative}"),
                ));
            }
            *total = total.checked_add(metadata.len()).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "workspace size overflow")
            })?;
            if *total > MAX_TOTAL_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "workspace exceeds the 32 MiB text budget",
                ));
            }
            if files.len() >= MAX_FILES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "workspace exceeds the 2048-file budget",
                ));
            }
            let relative = normalize_relative_path(&relative, true)?;
            files.insert(relative, metadata.len());
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_bytes(path, bytes).map_err(io::Error::other)
}

fn decode_utf8(bytes: Vec<u8>, relative: &str) -> io::Result<String> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("workspace file must be UTF-8 without BOM: {relative}"),
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("workspace file is not UTF-8 text: {relative}"),
        )
    })
}

fn write_tree(root: &Path, files: &BTreeMap<String, String>) -> io::Result<()> {
    for (relative, content) in files {
        let target = confined_path(root, relative, true)?;
        atomic_write(&target, content.as_bytes())?;
    }
    Ok(())
}

/// Delta-sync accepted canonical documents into a session root. Generated
/// `source/` is refreshed separately from one coherent editor snapshot.
fn sync_documents(canonical_root: &Path, session_root: &Path) -> io::Result<()> {
    let canonical = scan_text_tree(canonical_root, ScanMode::Writable)?;
    let current = scan_text_tree(session_root, ScanMode::Writable)?;
    for relative in current.keys().filter(|path| !canonical.contains_key(*path)) {
        let target = confined_path(session_root, relative, false)?;
        match fs::remove_file(target) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    for (relative, content) in canonical {
        if current.get(&relative) == Some(&content) {
            continue;
        }
        let target = confined_path(session_root, &relative, false)?;
        atomic_write(&target, content.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_io::EpsSnapshotFile;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("eud-agent-workspace-{tag}-{nanos}"))
    }

    fn manager(tag: &str) -> (PathBuf, WorkspaceManager) {
        let base = unique_temp_dir(tag);
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        (base, WorkspaceManager::new(dirs))
    }

    fn snapshot() -> EpsSnapshot {
        EpsSnapshot {
            project: "Example".to_string(),
            identity: "C:/maps/example.e3s".to_string(),
            files: vec![
                EpsSnapshotFile {
                    path: "main.eps".to_string(),
                    ftype: "CUIEps".to_string(),
                    content: Some("function onPluginStart() {}".to_string()),
                },
                EpsSnapshotFile {
                    path: "lib/util.eps".to_string(),
                    ftype: "CUIEps".to_string(),
                    content: Some("function util() {}".to_string()),
                },
            ],
        }
    }

    #[test]
    fn prepare_creates_durable_dirs_and_coherent_source_mirror() {
        let (base, manager) = manager("prepare");
        let workspace = manager.prepare_snapshot(&snapshot()).unwrap();

        for directory in DOCUMENT_DIRS {
            assert!(workspace.root.join(directory).is_dir());
        }
        assert_eq!(
            fs::read_to_string(workspace.root.join("source/main.eps")).unwrap(),
            "function onPluginStart() {}"
        );
        assert_eq!(
            fs::read_to_string(workspace.root.join("source/lib/util.eps")).unwrap(),
            "function util() {}"
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn baseline_diff_excludes_source_and_restores_writable_files() {
        let (base, manager) = manager("diff");
        let workspace = manager.prepare_snapshot(&snapshot()).unwrap();
        write_atomic_bytes(&workspace.root.join("specs/game.md"), b"old").unwrap();
        let baseline = manager.begin_turn(&workspace, "req-1").unwrap();

        write_atomic_bytes(&workspace.root.join("specs/game.md"), b"new").unwrap();
        write_atomic_bytes(&workspace.root.join("plans/next.md"), b"plan").unwrap();
        write_atomic_bytes(&workspace.root.join("source/main.eps"), b"tampered").unwrap();
        let changes = manager.changes(&baseline).unwrap();

        assert_eq!(changes.len(), 2);
        assert!(changes.iter().any(|change| {
            change.path == "specs/game.md" && change.kind == WorkspaceChangeKind::Modified
        }));
        assert!(changes.iter().any(|change| {
            change.path == "plans/next.md" && change.kind == WorkspaceChangeKind::Created
        }));
        manager
            .restore_file(&workspace.id, None, "specs/game.md", Some("old"))
            .unwrap();
        manager
            .restore_file(&workspace.id, None, "plans/next.md", None)
            .unwrap();
        assert_eq!(
            fs::read_to_string(workspace.root.join("specs/game.md")).unwrap(),
            "old"
        );
        assert!(!workspace.root.join("plans/next.md").exists());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn turn_recorder_journals_workspace_file_kinds() {
        let (base, manager) = manager("journal");
        let workspace = manager.prepare_snapshot(&snapshot()).unwrap();
        atomic_write(&workspace.root.join("specs/game.md"), b"old").unwrap();
        atomic_write(&workspace.root.join("decisions/remove.md"), b"obsolete").unwrap();
        let baseline = manager.begin_turn(&workspace, "req-journal").unwrap();

        atomic_write(&workspace.root.join("specs/game.md"), b"new").unwrap();
        atomic_write(&workspace.root.join("plans/next.md"), b"plan").unwrap();
        fs::remove_file(workspace.root.join("decisions/remove.md")).unwrap();

        let journal = JournalStore::new(base.join("roaming/eud-agent"));
        let mut recorder = WorkspaceTurnRecorder::new(manager.clone(), baseline, journal.clone());
        assert_eq!(recorder.finish().unwrap(), 3);
        let changeset = journal.changeset("req-journal").unwrap();
        assert!(changeset.items.iter().any(|item| {
            item.path.as_deref() == Some("specs/game.md")
                && item.kind == crate::journal::ChangesetItemKind::WorkspaceModified
        }));
        assert!(changeset.items.iter().any(|item| {
            item.path.as_deref() == Some("plans/next.md")
                && item.kind == crate::journal::ChangesetItemKind::WorkspaceCreated
        }));
        assert!(changeset.items.iter().any(|item| {
            item.path.as_deref() == Some("decisions/remove.md")
                && item.kind == crate::journal::ChangesetItemKind::WorkspaceDeleted
        }));
        let raw = JournalStore::load(base.join("roaming/eud-agent"), "req-journal").unwrap();
        manager
            .record_accepted_entries("req-journal", &raw.entries)
            .unwrap();
        let listed = manager.list_files(&workspace).unwrap();
        let accepted = listed
            .iter()
            .find(|file| file.path == "specs/game.md")
            .unwrap();
        assert_eq!(accepted.state.as_deref(), Some("accepted"));
        assert_eq!(accepted.revision, Some(1));
        assert!(listed
            .iter()
            .find(|file| file.path == "source/main.eps")
            .unwrap()
            .state
            .is_none());

        manager
            .record_plan_approval(&workspace.id, "req-journal", 2, "# Approved plan")
            .unwrap();
        assert_eq!(
            fs::read_to_string(workspace.root.join("plans/req-journal.md")).unwrap(),
            "# Approved plan"
        );
        let approved_plan = manager
            .list_files(&workspace)
            .unwrap()
            .into_iter()
            .find(|file| file.path == "plans/req-journal.md")
            .unwrap();
        assert_eq!(approved_plan.state.as_deref(), Some("approved"));
        assert_eq!(approved_plan.revision, Some(2));
        let post_approval_baseline = manager.begin_turn(&workspace, "req-execution").unwrap();
        assert!(manager.changes(&post_approval_baseline).unwrap().is_empty());
        manager.finish_turn(&post_approval_baseline).unwrap();
        let trusted = manager.load_state(&workspace.id).unwrap();
        assert_eq!(
            trusted.approved_plans["req-journal"].markdown_sha256,
            project_id("# Approved plan")
        );
        assert!(manager
            .state_path(&workspace.id)
            .unwrap()
            .starts_with(base.join("roaming/eud-agent/workspaces/.state")));
        assert!(!manager
            .state_path(&workspace.id)
            .unwrap()
            .starts_with(&workspace.root));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn approved_plan_completion_requires_linked_specs_and_worklog() {
        let (base, manager) = manager("completion-docs");
        let workspace = manager.prepare_snapshot(&snapshot()).unwrap();
        let approved_plan = "# Add combat progression";
        manager
            .record_plan_approval(&workspace.id, "req-wiki", 1, approved_plan)
            .unwrap();

        let gaps = manager
            .completion_doc_gaps(&workspace.id, "req-wiki", approved_plan)
            .unwrap();
        assert!(gaps.iter().any(|gap| gap.contains("specs/index.md")));
        assert!(gaps.iter().any(|gap| gap.contains("worklog/req-wiki.md")));

        atomic_write(
            &workspace.root.join("specs/combat.md"),
            b"# Combat\n\nImplemented progression.",
        )
        .unwrap();
        atomic_write(
            &workspace.root.join(SPEC_INDEX_PATH),
            b"# Project wiki\n\nCombat exists but is not linked.",
        )
        .unwrap();
        atomic_write(
            &workspace.root.join("worklog/req-wiki.md"),
            b"# Result\n\nVerified the build.",
        )
        .unwrap();
        let gaps = manager
            .completion_doc_gaps(&workspace.id, "req-wiki", approved_plan)
            .unwrap();
        assert_eq!(gaps.len(), 2);
        assert!(gaps.iter().all(|gap| gap.contains("must link")));

        atomic_write(
            &workspace.root.join(SPEC_INDEX_PATH),
            b"# Project wiki\n\n- [Combat](combat.md#progression)",
        )
        .unwrap();
        atomic_write(
            &workspace.root.join("worklog/req-wiki.md"),
            b"# Result\n\nVerified the build.\n\nSpec: [Combat](../specs/combat.md)",
        )
        .unwrap();
        assert!(manager
            .completion_doc_gaps(&workspace.id, "req-wiki", approved_plan)
            .unwrap()
            .is_empty());

        atomic_write(
            &workspace.root.join("plans/req-wiki.md"),
            b"# Replaced plan",
        )
        .unwrap();
        let gaps = manager
            .completion_doc_gaps(&workspace.id, "req-wiki", approved_plan)
            .unwrap();
        assert!(gaps.iter().any(|gap| gap.contains("differs")));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn paths_cannot_escape_or_target_generated_dirs() {
        let (base, manager) = manager("paths");
        let workspace = manager.prepare_snapshot(&snapshot()).unwrap();

        assert!(manager
            .restore_file(&workspace.id, None, "../outside.md", Some("x"))
            .is_err());
        assert!(manager
            .restore_file(&workspace.id, None, "source/main.eps", Some("x"))
            .is_err());
        assert!(manager
            .restore_file(&workspace.id, None, "C:/outside.md", Some("x"))
            .is_err());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn runtime_codegraph_metadata_is_excluded_from_diffs_and_paths() {
        let (base, manager) = manager("codegraph");
        let workspace = manager.prepare_snapshot(&snapshot()).unwrap();
        let baseline = manager.begin_turn(&workspace, "req-codegraph").unwrap();

        fs::write(workspace.root.join(CODEGRAPH_RUNTIME_PATH), [0xff]).unwrap();

        assert!(manager.changes(&baseline).unwrap().is_empty());
        assert!(manager
            .list_files(&workspace)
            .unwrap()
            .iter()
            .all(|file| file.path != CODEGRAPH_RUNTIME_PATH));
        assert!(manager
            .restore_file(&workspace.id, None, ".codegraph/index.db", Some("x"))
            .is_err());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn session_snapshots_stay_independent_and_accept_promotes_to_canonical() {
        let (base, manager) = manager("session-promote");
        let initial = snapshot();
        let canonical = manager.prepare_snapshot(&initial).unwrap();
        write_atomic_bytes(&canonical.root.join("specs/game.md"), b"accepted").unwrap();

        let session_a = manager
            .prepare_session_snapshot(&initial, "session-a")
            .unwrap();
        let baseline = manager.begin_turn(&session_a, "req-session-a").unwrap();
        write_atomic_bytes(&session_a.root.join("specs/game.md"), b"session-a change").unwrap();

        let mut changed_snapshot = initial.clone();
        changed_snapshot.files[0].content = Some("function changed() {}".to_string());
        let session_b = manager
            .prepare_session_snapshot(&changed_snapshot, "session-b")
            .unwrap();
        assert_eq!(
            fs::read_to_string(session_a.root.join("source/main.eps")).unwrap(),
            "function onPluginStart() {}"
        );
        assert_eq!(
            fs::read_to_string(session_b.root.join("source/main.eps")).unwrap(),
            "function changed() {}"
        );

        let journal = JournalStore::new(base.join("roaming"));
        let mut recorder = WorkspaceTurnRecorder::new(manager.clone(), baseline, journal.clone());
        assert_eq!(recorder.finish().unwrap(), 1);
        let entries = JournalStore::load(base.join("roaming"), "req-session-a")
            .unwrap()
            .entries;
        assert!(entries.iter().all(|entry| matches!(
            &entry.target,
            JournalTarget::WorkspacePath {
                session_id: Some(session_id),
                ..
            } if session_id == "session-a"
        )));
        manager
            .record_accepted_entries("req-session-a", &entries)
            .unwrap();
        assert_eq!(
            fs::read_to_string(canonical.root.join("specs/game.md")).unwrap(),
            "session-a change"
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn session_reject_leaves_canonical_bytes_and_approved_plan_unchanged() {
        let (base, manager) = manager("session-reject");
        let canonical = manager.prepare_snapshot(&snapshot()).unwrap();
        write_atomic_bytes(&canonical.root.join("specs/game.md"), b"accepted").unwrap();
        manager
            .record_plan_approval(&canonical.id, "req-approved", 1, "# Approved")
            .unwrap();
        let session = manager
            .prepare_session_snapshot(&snapshot(), "session-c")
            .unwrap();

        manager
            .restore_file(
                &session.id,
                session.session_id.as_deref(),
                "specs/game.md",
                Some("rejected change"),
            )
            .unwrap();
        manager
            .restore_file(
                &session.id,
                session.session_id.as_deref(),
                "specs/game.md",
                Some("accepted"),
            )
            .unwrap();

        assert_eq!(
            fs::read_to_string(canonical.root.join("specs/game.md")).unwrap(),
            "accepted"
        );
        assert_eq!(
            fs::read_to_string(canonical.root.join("plans/req-approved.md")).unwrap(),
            "# Approved"
        );
        fs::remove_dir_all(base).ok();
    }
}
