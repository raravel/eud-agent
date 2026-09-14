//! Per-project Codex filesystem workspace.
//!
//! Durable project documents live below the native project root:
//! `<project>/.eud-agent/workspace`. Trusted acceptance metadata lives beside it
//! in `<project>/.eud-agent/state/workspace.json`, outside every model cwd.
//! Session workspaces, temporary turn state, mirrors, and baselines remain
//! machine-local under [`DataDirs`].

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use similar::{DiffTag, TextDiff};

use crate::config::{DataDirs, RecentProjectRecord};
use crate::harness_import::HarnessImportIssue;
use crate::journal::{JournalEntry, JournalStore, JournalTarget, Snapshot, WriteTool};
use crate::memory::write_atomic_bytes;
use crate::native_project::NativeProject;
use crate::source_snapshot::{
    ProjectSnapshot as EpsSnapshot, ProjectSnapshotFile as EpsSnapshotFile,
};

pub const SOURCE_DIR: &str = "source";
pub const TEMP_DIR: &str = ".tmp";
const PROJECT_AGENT_DIR: &str = ".eud-agent";
const PROJECT_WORKSPACE_DIR: &str = "workspace";
const PROJECT_STATE_DIR: &str = "state";
const PROJECT_STATE_FILE: &str = "workspace.json";
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceDocumentUpdate {
    pub path: String,
    pub content: String,
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

fn native_eps_snapshot(dirs: &DataDirs) -> Result<EpsSnapshot, String> {
    let snapshot =
        crate::native_runtime::NativeProjectManager::new(dirs.clone()).source_snapshot()?;
    Ok(EpsSnapshot {
        project: snapshot.project,
        identity: snapshot.identity,
        files: snapshot
            .files
            .into_iter()
            .map(|file| {
                let ftype = if file.path.to_lowercase().ends_with(".py") {
                    "CUIPy"
                } else {
                    "CUIEps"
                };
                EpsSnapshotFile {
                    path: file
                        .path
                        .strip_prefix("src/")
                        .unwrap_or(&file.path)
                        .to_string(),
                    ftype: ftype.to_string(),
                    content: Some(file.content),
                }
            })
            .collect(),
    })
}

#[derive(Debug, Clone)]
pub struct WorkspaceManager {
    dirs: DataDirs,
}
/// Transaction guard for importing the accepted portion of a legacy E3S harness.
///
/// A successful import owns only newly-published documents and trusted state
/// until [`Self::commit`] is called. An empty prepared workspace may be reused;
/// rollback restores its prior state and leaves its generated files untouched.
#[derive(Debug)]
pub(crate) struct LegacyWorkspaceImport {
    source_project: Option<String>,
    target_root: Option<PathBuf>,
    state_path: Option<PathBuf>,
    state_bytes: Option<Vec<u8>>,
    previous_state: Option<Vec<u8>>,
    owned_directories: Vec<PathBuf>,
    owned_files: Vec<(PathBuf, Vec<u8>)>,
    native_memory_source_unique: bool,
    committed: bool,
}

impl LegacyWorkspaceImport {
    pub(crate) fn source_project(&self) -> Option<&str> {
        self.source_project.as_deref()
    }

    pub(crate) fn native_memory_source_is_unique(&self) -> bool {
        self.native_memory_source_unique
    }

    pub(crate) fn commit(mut self) {
        self.committed = true;
    }

    pub(crate) fn rollback(mut self) -> io::Result<()> {
        self.rollback_inner()
    }

    fn rollback_inner(&mut self) -> io::Result<()> {
        if self.committed {
            return Ok(());
        }
        let mut first_error = None;
        for (path, bytes) in self.owned_files.iter().rev() {
            match fs::read(path) {
                Ok(current) if current.as_slice() == bytes.as_slice() => {
                    if let Err(error) = fs::remove_file(path) {
                        if error.kind() != io::ErrorKind::NotFound && first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        if let (Some(path), Some(bytes)) = (&self.state_path, &self.state_bytes) {
            match fs::read(path) {
                Ok(current) if current.as_slice() == bytes.as_slice() => {
                    let restore = match &self.previous_state {
                        Some(previous) => atomic_write(path, previous),
                        None => fs::remove_file(path),
                    };
                    if let Err(error) = restore {
                        if error.kind() != io::ErrorKind::NotFound && first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        for directory in self.owned_directories.iter().rev() {
            if let Err(error) = fs::remove_dir(directory) {
                if !matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) && first_error.is_none()
                {
                    first_error = Some(error);
                }
            }
        }
        if let Some(root) = &self.target_root {
            remove_empty_tree(root, &mut first_error);
        }
        self.committed = true;
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for LegacyWorkspaceImport {
    fn drop(&mut self) {
        let _ = self.rollback_inner();
    }
}

impl WorkspaceManager {
    pub fn new(dirs: DataDirs) -> Self {
        Self { dirs }
    }

    /// Migrate the old machine-local native harness into the explicitly opened
    /// native project. The old store is read-only; every unavailable item is a
    /// review issue and healthy independent items are still imported.
    pub(crate) fn migrate_native_harness(
        &self,
        target: &NativeProject,
        issues: &mut Vec<HarnessImportIssue>,
    ) -> io::Result<LegacyWorkspaceImport> {
        let candidates = self.find_legacy_candidates(target, issues)?;
        let mut guard = self.migrate_candidate_import(target, issues, candidates, true)?;
        if guard.source_project.is_none() {
            guard.native_memory_source_unique = false;
        }
        Ok(guard)
    }

    fn migrate_candidate_import(
        &self,
        target: &NativeProject,
        issues: &mut Vec<HarnessImportIssue>,
        candidates: Vec<(PathBuf, TrustedWorkspaceState, PathBuf)>,
        native_memory_candidate: bool,
    ) -> io::Result<LegacyWorkspaceImport> {
        let target_root = fs::canonicalize(target.root())?;
        let workspace_existed = path_exists_any(
            &target_root
                .join(PROJECT_AGENT_DIR)
                .join(PROJECT_WORKSPACE_DIR),
        );
        let (target_id, target_workspace, target_state_path, previous_state) =
            self.local_target_paths(&target_root)?;
        let mut guard = LegacyWorkspaceImport {
            source_project: None,
            target_root: None,
            state_path: Some(target_state_path.clone()),
            state_bytes: None,
            previous_state: previous_state.clone(),
            owned_directories: Vec::new(),
            owned_files: Vec::new(),
            native_memory_source_unique: false,
            committed: false,
        };
        guard.target_root = (!workspace_existed).then(|| target_workspace.clone());

        let mut imported = TrustedWorkspaceState::default();
        if let Some(bytes) = &previous_state {
            imported = serde_json::from_slice(bytes).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("local trusted state: {error}"),
                )
            })?;
        }
        imported.version = 1;
        imported.id = target_id.clone();
        imported.project = target.manifest().name.clone();
        imported.identity_hash = target_id.clone();

        let mut files = BTreeMap::<String, Vec<u8>>::new();
        if candidates.len() == 1 {
            let (state_path, source_state, source_root) = candidates.into_iter().next().unwrap();
            guard.source_project = Some(source_state.project.clone());
            guard.native_memory_source_unique = if native_memory_candidate {
                self.legacy_native_memory_is_unambiguous(target)?
            } else {
                false
            };
            let mut remaining_bytes = MAX_TOTAL_BYTES;
            let mut lower_paths = BTreeSet::new();

            for (relative_raw, trusted) in source_state.documents.clone() {
                let relative = match normalize_relative_path(&relative_raw, false) {
                    Ok(value) => value,
                    Err(error) => {
                        issues.push(HarnessImportIssue::new(
                            "workspace",
                            state_path.display().to_string(),
                            format!("unsafe document path `{relative_raw}`: {error}"),
                        ));
                        continue;
                    }
                };
                if trusted.revision == 0
                    || normalize_token(&trusted.request_id, "legacy request id").is_err()
                {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        source_root.join(&relative).display().to_string(),
                        "document metadata is corrupt",
                    ));
                    continue;
                }
                match trusted.state.as_str() {
                    "deleted" => {
                        if !imported.documents.contains_key(&relative)
                            && !target_workspace.join(&relative).exists()
                        {
                            imported.documents.insert(relative, trusted);
                        }
                        continue;
                    }
                    "accepted" => {}
                    _ => {
                        issues.push(HarnessImportIssue::new(
                            "workspace",
                            source_root.join(&relative).display().to_string(),
                            "document is not accepted",
                        ));
                        continue;
                    }
                }
                if let Some(request_id) = relative
                    .strip_prefix("plans/")
                    .and_then(|value| value.strip_suffix(".md"))
                {
                    if !source_state.approved_plans.contains_key(request_id) {
                        issues.push(HarnessImportIssue::new(
                            "workspace",
                            source_root.join(&relative).display().to_string(),
                            "plan document has no approval metadata",
                        ));
                    }
                    continue;
                }
                if !lower_paths.insert(relative.to_ascii_lowercase()) {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        source_root.join(&relative).display().to_string(),
                        "document path collides case-insensitively",
                    ));
                    continue;
                }
                match read_import_document(
                    &source_root,
                    &relative,
                    files.len(),
                    &mut remaining_bytes,
                ) {
                    Ok(bytes) => {
                        if imported.documents.contains_key(&relative)
                            || target_workspace.join(&relative).exists()
                        {
                            issues.push(HarnessImportIssue::new(
                                "workspace",
                                target_workspace.join(&relative).display().to_string(),
                                "local document is authoritative; source document was not copied",
                            ));
                        } else {
                            files.insert(relative.clone(), bytes);
                            imported.documents.insert(relative, trusted);
                        }
                    }
                    Err(error) => issues.push(HarnessImportIssue::new(
                        "workspace",
                        source_root.join(&relative).display().to_string(),
                        error.to_string(),
                    )),
                }
            }

            for (request_id, plan) in source_state.approved_plans.clone() {
                let relative = match approved_plan_path(&request_id) {
                    Ok(value) => value,
                    Err(error) => {
                        issues.push(HarnessImportIssue::new(
                            "workspace",
                            source_root.join("plans").display().to_string(),
                            format!("unsafe approval id `{request_id}`: {error}"),
                        ));
                        continue;
                    }
                };
                if plan.revision == 0 || plan.markdown_sha256.len() != 64 {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        source_root.join(&relative).display().to_string(),
                        "approval metadata is corrupt",
                    ));
                    continue;
                }
                if !lower_paths.insert(relative.to_ascii_lowercase()) {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        source_root.join(&relative).display().to_string(),
                        "plan path collides with another imported item",
                    ));
                    continue;
                }
                if let Some(document) = source_state.documents.get(&relative) {
                    if document.state != "accepted" {
                        issues.push(HarnessImportIssue::new(
                            "workspace",
                            source_root.join(&relative).display().to_string(),
                            "approved plan has a deleted or invalid document tombstone",
                        ));
                        continue;
                    }
                }
                let bytes = match read_import_plan(
                    &source_root,
                    &relative,
                    files.len(),
                    &mut remaining_bytes,
                ) {
                    Ok(Some(bytes)) => bytes,
                    Ok(None) => {
                        issues.push(HarnessImportIssue::new(
                            "workspace",
                            source_root.join(&relative).display().to_string(),
                            "approved plan body is unavailable",
                        ));
                        continue;
                    }
                    Err(error) => {
                        issues.push(HarnessImportIssue::new(
                            "workspace",
                            source_root.join(&relative).display().to_string(),
                            error.to_string(),
                        ));
                        continue;
                    }
                };
                if sha256_hex(&bytes) != plan.markdown_sha256 {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        source_root.join(&relative).display().to_string(),
                        "approved plan hash does not match its content",
                    ));
                    continue;
                }
                if imported.documents.contains_key(&relative)
                    || imported.approved_plans.contains_key(&request_id)
                    || target_workspace.join(&relative).exists()
                {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        target_workspace.join(&relative).display().to_string(),
                        "local plan or approval is authoritative; source plan was not copied",
                    ));
                    continue;
                }
                files.insert(relative.clone(), bytes);
                if let Some(document) = source_state.documents.get(&relative) {
                    imported
                        .documents
                        .insert(relative.clone(), document.clone());
                }
                imported.approved_plans.insert(request_id, plan);
            }
        } else if candidates.len() > 1 {
            issues.push(HarnessImportIssue::new(
                "workspace",
                self.dirs
                    .workspace_state_dir()
                    .join("projects")
                    .display()
                    .to_string(),
                "multiple legacy native stores match the project; ownership is ambiguous",
            ));
        }

        // All temporary publication files stay on the target volume. Hard links
        // publish complete files without replacing a concurrently created file.
        let stage_parent = target_root.join(PROJECT_AGENT_DIR).join(".import-staging");
        let stage_root = stage_parent.join(uuid::Uuid::new_v4().to_string());
        let prepare_stage = crate::memory::validate_existing_components(&stage_parent)
            .and_then(|_| fs::create_dir_all(&stage_parent))
            .and_then(|_| ensure_plain_directory(&stage_parent))
            .and_then(|_| fs::create_dir(&stage_root));
        if let Err(error) = prepare_stage {
            issues.push(HarnessImportIssue::new(
                "workspace",
                stage_parent.to_string_lossy(),
                format!("가져오기 임시 폴더를 만들 수 없어 작업 문서를 제외했습니다: {error}"),
            ));
            guard.rollback_inner()?;
            return Ok(guard);
        }
        let publish = (|| -> io::Result<()> {
            for (index, (relative, bytes)) in files.into_iter().enumerate() {
                let staged = stage_root.join(index.to_string());
                let result = (|| -> io::Result<PathBuf> {
                    let path = confined_path(&target_workspace, &relative, false)?;
                    create_import_directories(
                        &target_workspace,
                        path.parent().expect("confined document has a parent"),
                        &mut guard.owned_directories,
                    )?;
                    atomic_write(&staged, &bytes)?;
                    crate::harness_import::publish_staged_file(&staged, &path)?;
                    Ok(path)
                })();
                match result {
                    Ok(path) => guard.owned_files.push((path, bytes)),
                    Err(error) => {
                        imported.documents.remove(&relative);
                        if let Some(request_id) = relative
                            .strip_prefix("plans/")
                            .and_then(|value| value.strip_suffix(".md"))
                        {
                            imported.approved_plans.remove(request_id);
                        }
                        issues.push(HarnessImportIssue::new(
                            "workspace", target_workspace.join(&relative).to_string_lossy(),
                            format!("이 문서를 저장할 수 없어 제외했습니다. 기존 파일은 보존합니다: {error}"),
                        ));
                    }
                }
            }
            let state_bytes = serde_json::to_vec_pretty(&imported)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if previous_state.as_deref() == Some(state_bytes.as_slice()) {
                return Ok(());
            }
            if let Some(previous) = previous_state.as_deref() {
                let current = read_bounded_regular_file(&target_state_path, MAX_FILE_BYTES)?;
                if current != previous {
                    return Err(io::Error::other(
                        "가져오는 동안 승인 기록이 변경되어 기존 기록을 보존했습니다.",
                    ));
                }
                atomic_write(&target_state_path, &state_bytes)?;
            } else {
                let staged = stage_root.join("trusted-state.json");
                atomic_write(&staged, &state_bytes)?;
                crate::harness_import::publish_staged_file(&staged, &target_state_path)?;
            }
            guard.state_bytes = Some(state_bytes);
            Ok(())
        })();
        let cleanup = ensure_plain_directory(&stage_root)
            .and_then(|_| fs::remove_dir_all(&stage_root))
            .and_then(|_| match fs::remove_dir(&stage_parent) {
                Ok(()) => Ok(()),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                    ) =>
                {
                    Ok(())
                }
                Err(error) => Err(error),
            });
        if let Err(error) = cleanup {
            let rollback = guard.rollback_inner();
            return Err(io::Error::other(format!(
                "workspace staging cleanup failed: {error}; publication: {}; rollback: {}",
                publish
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "complete".into()),
                rollback
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "complete".into()),
            )));
        }
        if let Err(error) = publish {
            issues.push(HarnessImportIssue::new(
                "workspace",
                target_state_path.to_string_lossy(),
                format!(
                    "승인 기록을 저장하지 못해 이 작업 문서 저장소의 복사를 되돌렸습니다: {error}"
                ),
            ));
            guard.rollback_inner().map_err(|rollback| {
                io::Error::other(format!(
                    "workspace import failed: {error}; rollback failed: {rollback}"
                ))
            })?;
        }
        Ok(guard)
    }

    /// Import accepted documents associated with the explicitly selected E3S
    /// source. Unlike native migration, ownership is the full canonical source
    /// path recorded by the old trusted state.
    pub(crate) fn import_legacy_harness(
        &self,
        source_e3s: &Path,
        target: &NativeProject,
        issues: &mut Vec<HarnessImportIssue>,
    ) -> io::Result<LegacyWorkspaceImport> {
        let source = fs::canonicalize(source_e3s).unwrap_or_else(|_| source_e3s.to_path_buf());
        let candidates = self.find_legacy_source_candidates(&source, issues)?;
        self.migrate_candidate_import(target, issues, candidates, false)
    }

    fn find_legacy_source_candidates(
        &self,
        source: &Path,
        issues: &mut Vec<HarnessImportIssue>,
    ) -> io::Result<Vec<(PathBuf, TrustedWorkspaceState, PathBuf)>> {
        let projects_dir = self.dirs.workspace_state_dir().join("projects");
        if !projects_dir.is_dir() {
            return Ok(Vec::new());
        }
        let source_key = canonical_legacy_project_key(&source.to_string_lossy());
        let entries = match fs::read_dir(&projects_dir) {
            Ok(entries) => match entries.collect::<Result<Vec<_>, _>>() {
                Ok(entries) => entries,
                Err(error) => {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        projects_dir.display().to_string(),
                        error.to_string(),
                    ));
                    return Ok(Vec::new());
                }
            },
            Err(error) => {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    projects_dir.display().to_string(),
                    error.to_string(),
                ));
                return Ok(Vec::new());
            }
        };
        let mut candidates = Vec::new();
        for entry in entries {
            let state_path = entry.path();
            let metadata = match fs::symlink_metadata(&state_path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        state_path.display().to_string(),
                        error.to_string(),
                    ));
                    continue;
                }
            };
            if metadata.file_type().is_symlink() || crate::memory::is_reparse_point(&metadata) {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    state_path.display().to_string(),
                    "legacy trusted state is a reparse point",
                ));
                continue;
            }
            if !metadata.is_file()
                || state_path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let Some(id) = state_path.file_stem().and_then(|value| value.to_str()) else {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    state_path.display().to_string(),
                    "legacy trusted state filename is not Unicode",
                ));
                continue;
            };
            if let Err(error) = normalize_workspace_id(id) {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    state_path.display().to_string(),
                    error.to_string(),
                ));
                continue;
            }
            let bytes = match read_bounded_regular_file(&state_path, MAX_FILE_BYTES) {
                Ok(bytes) => bytes,
                Err(error) => {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        state_path.display().to_string(),
                        error.to_string(),
                    ));
                    continue;
                }
            };
            let state: TrustedWorkspaceState = match serde_json::from_slice(&bytes) {
                Ok(state) => state,
                Err(error) => {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        state_path.display().to_string(),
                        format!("trusted state is corrupt: {error}"),
                    ));
                    continue;
                }
            };
            if state.id != id || state.identity_hash != id || state.project.trim().is_empty() {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    state_path.display().to_string(),
                    "trusted state identity is inconsistent",
                ));
                continue;
            }
            if canonical_legacy_project_key(&state.project) != source_key {
                continue;
            }
            let source_root = self.dirs.workspaces_dir().join(id);
            if !source_root.is_dir() {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    source_root.display().to_string(),
                    "trusted state has no legacy workspace root",
                ));
                continue;
            }
            if let Err(error) = ensure_plain_directory(&source_root) {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    source_root.display().to_string(),
                    error.to_string(),
                ));
                continue;
            }
            candidates.push((state_path, state, source_root));
        }
        Ok(candidates)
    }
    /// Return true only when the exact-root native legacy store is uniquely
    /// identifiable among all valid old native stores. A corrupt or unreadable
    /// sibling conservatively makes memory association unsafe.
    pub(crate) fn legacy_native_memory_is_unambiguous(
        &self,
        target: &NativeProject,
    ) -> io::Result<bool> {
        let projects_dir = self.dirs.workspace_state_dir().join("projects");
        if !projects_dir.is_dir() {
            return Ok(false);
        }
        let expected_id = project_id(&target.root().to_string_lossy());
        let entries = match fs::read_dir(&projects_dir) {
            Ok(entries) => entries.collect::<Result<Vec<_>, _>>()?,
            Err(_) => return Ok(false),
        };
        let mut same_name = 0usize;
        let mut exact = false;
        for entry in entries {
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => return Ok(false),
            };
            if metadata.file_type().is_symlink()
                || crate::memory::is_reparse_point(&metadata)
                || !metadata.is_file()
                || path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
                return Ok(false);
            };
            let bytes = match read_bounded_regular_file(&path, MAX_FILE_BYTES) {
                Ok(bytes) => bytes,
                Err(_) => return Ok(false),
            };
            let state: TrustedWorkspaceState = match serde_json::from_slice(&bytes) {
                Ok(state) => state,
                Err(_) => return Ok(false),
            };
            if state.id != id || state.identity_hash != id || state.project.trim().is_empty() {
                return Ok(false);
            }
            if state.project == target.manifest().name {
                same_name += 1;
                exact |= id == expected_id;
            }
        }
        Ok(exact && same_name == 1)
    }
    fn local_target_paths(
        &self,
        target_root: &Path,
    ) -> io::Result<(String, PathBuf, PathBuf, Option<Vec<u8>>)> {
        let agent_root = target_root.join(PROJECT_AGENT_DIR);
        let workspace = agent_root.join(PROJECT_WORKSPACE_DIR);
        let state_path = agent_root.join(PROJECT_STATE_DIR).join(PROJECT_STATE_FILE);
        ensure_plain_directory(target_root)?;
        fs::create_dir_all(&agent_root)?;
        ensure_plain_directory(&agent_root)?;
        let state_root = agent_root.join(PROJECT_STATE_DIR);
        fs::create_dir_all(&state_root)?;
        ensure_plain_directory(&state_root)?;
        fs::create_dir_all(&workspace)?;
        ensure_plain_directory(&workspace)?;
        let previous_state = if path_exists_any(&state_path) {
            let bytes = read_bounded_regular_file(&state_path, MAX_FILE_BYTES)?;
            let state: TrustedWorkspaceState = serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if state.version != 1
                || normalize_workspace_id(&state.id).is_err()
                || state.identity_hash != state.id
                || state.project.trim().is_empty()
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "local trusted workspace ownership state is inconsistent",
                ));
            }
            Some(bytes)
        } else {
            None
        };
        let id = previous_state
            .as_deref()
            .and_then(|bytes| serde_json::from_slice::<TrustedWorkspaceState>(bytes).ok())
            .filter(|state| !state.id.is_empty())
            .map(|state| state.id)
            .unwrap_or_else(|| project_id(&target_root.to_string_lossy()));
        Ok((id, workspace, state_path, previous_state))
    }

    fn find_legacy_candidates(
        &self,
        target: &NativeProject,
        issues: &mut Vec<HarnessImportIssue>,
    ) -> io::Result<Vec<(PathBuf, TrustedWorkspaceState, PathBuf)>> {
        let projects_dir = self.dirs.workspace_state_dir().join("projects");
        if !path_exists_any(&projects_dir) {
            return Ok(Vec::new());
        }
        if let Err(error) = ensure_plain_directory(&projects_dir) {
            issues.push(HarnessImportIssue::new(
                "workspace",
                projects_dir.display().to_string(),
                error.to_string(),
            ));
            return Ok(Vec::new());
        }
        let target_key = canonical_legacy_project_key(&target.root().to_string_lossy());
        let mut candidates = Vec::new();
        let mut entries = match fs::read_dir(&projects_dir) {
            Ok(entries) => match entries.collect::<Result<Vec<_>, _>>() {
                Ok(entries) => entries,
                Err(error) => {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        projects_dir.display().to_string(),
                        error.to_string(),
                    ));
                    return Ok(Vec::new());
                }
            },
            Err(error) => {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    projects_dir.display().to_string(),
                    error.to_string(),
                ));
                return Ok(Vec::new());
            }
        };
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let state_path = entry.path();
            let metadata = match fs::symlink_metadata(&state_path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        state_path.display().to_string(),
                        error.to_string(),
                    ));
                    continue;
                }
            };
            if metadata.file_type().is_symlink() || crate::memory::is_reparse_point(&metadata) {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    state_path.display().to_string(),
                    "legacy trusted state is a reparse point",
                ));
                continue;
            }
            if !metadata.is_file()
                || state_path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let Some(id_text) = state_path.file_stem().and_then(|value| value.to_str()) else {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    state_path.display().to_string(),
                    "legacy trusted state filename is not Unicode",
                ));
                continue;
            };
            let id = match normalize_workspace_id(id_text) {
                Ok(id) => id,
                Err(error) => {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        state_path.display().to_string(),
                        error.to_string(),
                    ));
                    continue;
                }
            };
            let bytes = match read_bounded_regular_file(&state_path, MAX_FILE_BYTES) {
                Ok(bytes) => bytes,
                Err(error) => {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        state_path.display().to_string(),
                        error.to_string(),
                    ));
                    continue;
                }
            };
            let state: TrustedWorkspaceState = match serde_json::from_slice(&bytes) {
                Ok(state) => state,
                Err(error) => {
                    issues.push(HarnessImportIssue::new(
                        "workspace",
                        state_path.display().to_string(),
                        format!("trusted state is corrupt: {error}"),
                    ));
                    continue;
                }
            };
            if state.id != id || state.identity_hash != id || state.project.trim().is_empty() {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    state_path.display().to_string(),
                    "trusted state identity is inconsistent",
                ));
                continue;
            }
            let matches_target = (id == project_id(&target.root().to_string_lossy())
                && state.project == target.manifest().name)
                || canonical_legacy_project_key(&state.project) == target_key;
            if !matches_target {
                continue;
            }
            let source_root = self.dirs.workspaces_dir().join(&id);
            if !source_root.is_dir() {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    source_root.display().to_string(),
                    "trusted state has no legacy workspace root",
                ));
                continue;
            }
            if let Err(error) = ensure_plain_directory(&source_root) {
                issues.push(HarnessImportIssue::new(
                    "workspace",
                    source_root.display().to_string(),
                    error.to_string(),
                ));
                continue;
            }
            candidates.push((state_path, state, source_root));
        }
        Ok(candidates)
    }

    /// Build the current native project's durable workspace.
    pub fn prepare_current(&self) -> Result<PreparedWorkspace, String> {
        let snapshot = native_eps_snapshot(&self.dirs)?;
        self.prepare_snapshot(&snapshot)
            .map_err(|error| error.to_string())
    }

    /// Create the durable project-local document directories and trusted state.
    pub fn prepare_snapshot(&self, snapshot: &EpsSnapshot) -> io::Result<PreparedWorkspace> {
        if snapshot.project.trim().is_empty() || snapshot.identity.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace snapshot has no project identity",
            ));
        }
        let identity_root = PathBuf::from(&snapshot.identity);
        let project_root = fs::canonicalize(&identity_root).map_err(|error| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("native project root is unavailable: {error}"),
            )
        })?;
        ensure_plain_directory(&project_root)?;
        if !project_root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native project identity is not a directory",
            ));
        }
        let agent_root = project_root.join(PROJECT_AGENT_DIR);
        match fs::symlink_metadata(&agent_root) {
            Ok(_) => ensure_plain_directory(&agent_root)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&agent_root)?,
            Err(error) => return Err(error),
        }
        let root = agent_root.join(PROJECT_WORKSPACE_DIR);
        let state_path = agent_root.join(PROJECT_STATE_DIR).join(PROJECT_STATE_FILE);
        let state_parent = state_path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "workspace state has no parent")
        })?;
        match fs::symlink_metadata(state_parent) {
            Ok(_) => ensure_plain_directory(state_parent)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(state_parent)?,
            Err(error) => return Err(error),
        }
        let previous_state = if path_exists_any(&state_path) {
            Some(read_bounded_regular_file(&state_path, MAX_FILE_BYTES)?)
        } else {
            None
        };
        let parsed_state = previous_state
            .as_deref()
            .map(|bytes| {
                serde_json::from_slice::<TrustedWorkspaceState>(bytes).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("local trusted state: {error}"),
                    )
                })
            })
            .transpose()?;
        if let Some(state) = &parsed_state {
            if state.version != 1
                || state.id.is_empty()
                || state.identity_hash != state.id
                || state.project.trim().is_empty()
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "local trusted workspace ownership state is inconsistent",
                ));
            }
        }
        let id = parsed_state
            .as_ref()
            .filter(|state| !state.id.is_empty())
            .map(|state| state.id.clone())
            .unwrap_or_else(|| project_id(&project_root.to_string_lossy()));
        normalize_workspace_id(&id)?;
        fs::create_dir_all(&root)?;
        ensure_plain_directory(&root)?;
        for directory in DOCUMENT_DIRS {
            let path = root.join(directory);
            fs::create_dir_all(&path)?;
            ensure_plain_directory(&path)?;
        }
        let mut trusted = parsed_state.unwrap_or_default();
        trusted.version = 1;
        trusted.id = id.clone();
        trusted.project = snapshot.project.clone();
        trusted.identity_hash = id.clone();
        let state_parent = state_path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "workspace state has no parent")
        })?;
        ensure_plain_directory_chain(&agent_root, state_parent)?;
        self.save_state_at(&state_path, &trusted)?;

        Ok(PreparedWorkspace {
            id,
            project: snapshot.project.clone(),
            root,
            session_id: None,
        })
    }

    /// Prepare one session-owned Codex cwd from the latest coherent native snapshot and the
    /// canonical accepted document tree.
    pub fn prepare_session_current(&self, session_id: &str) -> Result<PreparedWorkspace, String> {
        let snapshot = native_eps_snapshot(&self.dirs)?;
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

    /// Prepare an isolated document-only workspace from canonical accepted files.
    ///
    /// Background harness generation never needs a live editor snapshot. It receives
    /// accepted source changes inline and stages only `specs/`, `decisions/`, and
    /// `worklog/` updates in this isolated root.
    pub(crate) fn prepare_document_session(
        &self,
        workspace_id: &str,
        project: &str,
        session_id: &str,
    ) -> io::Result<PreparedWorkspace> {
        let workspace_id = normalize_workspace_id(workspace_id)?;
        let session_id = normalize_token(session_id, "session id")?;
        let canonical_root = self.workspace_root(&workspace_id)?;
        if !canonical_root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("canonical workspace `{workspace_id}` does not exist"),
            ));
        }
        let root = self.session_workspace_root(&workspace_id, &session_id)?;
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
        sync_documents(&canonical_root, &root)?;
        Ok(PreparedWorkspace {
            id: workspace_id,
            project: project.to_string(),
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

    pub(crate) fn document_content(
        &self,
        workspace: &PreparedWorkspace,
        relative: &str,
    ) -> io::Result<Option<String>> {
        validate_mutable_document_path(relative)?;
        let path = confined_path(&workspace.root, relative, false)?;
        optional_regular_file_bytes(&path)?
            .map(|bytes| decode_utf8(bytes, relative))
            .transpose()
    }

    /// Apply a prevalidated document batch with rollback on the first failed write.
    ///
    /// Callers compute every final file body before entering this method, so the
    /// mutation phase performs no model work and no shell commands.
    pub(crate) fn apply_document_updates(
        &self,
        workspace: &PreparedWorkspace,
        updates: &[WorkspaceDocumentUpdate],
    ) -> io::Result<()> {
        let mut seen = BTreeSet::new();
        let mut prepared = Vec::with_capacity(updates.len());
        for update in updates {
            validate_mutable_document_path(&update.path)?;
            if !seen.insert(update.path.as_str()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("duplicate document update `{}`", update.path),
                ));
            }
            if update.content.len() as u64 > MAX_FILE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "document update `{}` exceeds the file size cap",
                        update.path
                    ),
                ));
            }
            let path = confined_path(&workspace.root, &update.path, true)?;
            let previous = optional_regular_file_bytes(&path)?;
            prepared.push((path, previous, update.content.as_bytes().to_vec()));
        }

        let mut applied: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::new();
        for (path, previous, content) in &prepared {
            if let Err(error) = atomic_write(path, content) {
                for (applied_path, applied_previous) in applied.into_iter().rev() {
                    let _ = restore_optional_file_bytes(&applied_path, applied_previous.as_deref());
                }
                return Err(error);
            }
            applied.push((path.clone(), previous.clone()));
        }
        Ok(())
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

    pub fn search_files(&self, workspace_id: &str, query: &str) -> io::Result<Vec<String>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let query = query.to_lowercase();
        let root = self.workspace_root(workspace_id)?;
        let files = scan_files(&root, ScanMode::All)?;
        let mut matches = Vec::new();
        for relative in files.keys() {
            let bytes = fs::read(confined_path(&root, relative, true)?)?;
            let content = match decode_utf8(bytes, relative) {
                Ok(content) => content,
                Err(error) if error.kind() == io::ErrorKind::InvalidData => continue,
                Err(error) => return Err(error),
            };
            if relative.to_lowercase().contains(&query) || content.to_lowercase().contains(&query) {
                matches.push(relative.clone());
            }
        }
        Ok(matches)
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
        let mut pending = BTreeMap::<PathBuf, PendingWorkspacePromotion>::new();
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
            let base = workspace_snapshot_content(path, &entry.before, "before")?;
            let after = workspace_snapshot_content(path, &entry.after, "after")?;
            match pending.get_mut(&canonical_path) {
                Some(promotion) => promotion.after = after.map(str::to_string),
                None => {
                    pending.insert(
                        canonical_path,
                        PendingWorkspacePromotion {
                            relative: path.clone(),
                            base: base.map(str::to_string),
                            after: after.map(str::to_string),
                        },
                    );
                }
            }
        }

        let mut resolved = Vec::with_capacity(pending.len());
        for (path, promotion) in pending {
            let before = optional_regular_file_bytes(&path)?;
            let after = merge_workspace_content(
                &promotion.relative,
                promotion.base.as_deref(),
                promotion.after.as_deref(),
                before.as_deref(),
            )?;
            if before != after {
                resolved.push((path, before, after));
            }
        }

        let mut promoted = Vec::new();
        for (path, before, after) in resolved {
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

    pub(crate) fn workspace_root(&self, workspace_id: &str) -> io::Result<PathBuf> {
        let workspace_id = normalize_workspace_id(workspace_id)?;
        let config = self
            .dirs
            .load_config()
            .map_err(|error| io::Error::other(error.to_string()))?;
        if config.project_path.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no native project is configured",
            ));
        }
        let project_root = fs::canonicalize(&config.project_path)?;
        ensure_plain_directory(&project_root)?;
        let agent_root = project_root.join(PROJECT_AGENT_DIR);
        let state_root = agent_root.join(PROJECT_STATE_DIR);
        let root = agent_root.join(PROJECT_WORKSPACE_DIR);
        ensure_plain_directory(&agent_root)?;
        ensure_plain_directory(&state_root)?;
        ensure_plain_directory(&root)?;
        let state_path = state_root.join(PROJECT_STATE_FILE);
        let bytes = read_bounded_regular_file(&state_path, MAX_FILE_BYTES)?;
        let state: TrustedWorkspaceState = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if state.version != 1
            || state.id != workspace_id
            || state.identity_hash != workspace_id
            || state.project.trim().is_empty()
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "workspace id is not owned by the configured native project",
            ));
        }
        Ok(root)
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
        let root = self.workspace_root(workspace_id)?;
        root.parent()
            .map(|agent| agent.join(PROJECT_STATE_DIR).join(PROJECT_STATE_FILE))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "workspace has no agent root")
            })
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
        self.save_state_at(&path, state)
    }

    fn save_state_at(&self, path: &Path, state: &TrustedWorkspaceState) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "workspace state has no parent")
        })?;
        ensure_plain_directory_chain(
            parent.parent().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "workspace state has no agent root",
                )
            })?,
            parent,
        )?;
        atomic_write(path, &bytes)
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

fn path_exists_any(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn remove_empty_tree(path: &Path, first_error: &mut Option<io::Error>) {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            if first_error.is_none() {
                *first_error = Some(error);
            }
            return;
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return;
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            if first_error.is_none() {
                *first_error = Some(error);
            }
            return;
        }
    };
    for entry in entries {
        match entry {
            Ok(entry) if entry.path().is_dir() => remove_empty_tree(&entry.path(), first_error),
            Ok(_) => {}
            Err(error) if first_error.is_none() => *first_error = Some(error),
            Err(_) => {}
        }
    }
    match fs::remove_dir(path) {
        Ok(()) => {}
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                || error
                    .raw_os_error()
                    .is_some_and(|code| matches!(code, 39 | 145)) => {}
        Err(error) if first_error.is_none() => *first_error = Some(error),
        Err(_) => {}
    }
}

fn create_import_directories(
    root: &Path,
    parent: &Path,
    owned: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "path escapes import root"))?;
    let mut current = root.to_path_buf();
    ensure_plain_directory(&current)?;
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "import path is not normalized",
            ));
        };
        current.push(component);
        match fs::create_dir(&current) {
            Ok(()) => owned.push(current.clone()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                ensure_plain_directory(&current)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn canonical_legacy_project_key(value: &str) -> String {
    let normalized = RecentProjectRecord::key(value);
    let path = Path::new(value);
    let looks_absolute = path.is_absolute()
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.as_bytes().get(1).is_some_and(|byte| *byte == b':');
    if looks_absolute {
        if let Ok(canonical) = fs::canonicalize(path) {
            return RecentProjectRecord::key(&canonical.to_string_lossy());
        }
    }
    normalized
}

fn ensure_plain_directory_chain(root: &Path, target: &Path) -> io::Result<()> {
    ensure_plain_directory(root)?;
    let relative = target
        .strip_prefix(root)
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "path escapes import root"))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "import path is not normalized",
            ));
        };
        current.push(component);
        ensure_plain_directory(&current)?;
    }
    Ok(())
}
fn read_import_plan(
    root: &Path,
    relative: &str,
    file_count: usize,
    remaining_bytes: &mut u64,
) -> io::Result<Option<Vec<u8>>> {
    let path = confined_path(root, relative, false)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "legacy plan has no parent"))?;
    // The legacy plan directory itself may have been deleted; every approved
    // plan below it is then an eligible missing body. The source root must
    // still exist, and any other parent failure remains fatal.
    ensure_plain_directory(root)?;
    match fs::symlink_metadata(parent) {
        Ok(_) => ensure_plain_directory(parent)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            ensure_plain_directory_chain(root, parent)?;
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if file_count >= MAX_FILES
        && metadata.is_file()
        && !metadata.file_type().is_symlink()
        && !crate::memory::is_reparse_point(&metadata)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "legacy workspace exceeds the 2048-file budget",
        ));
    }
    let bytes = match read_bounded_regular_file(&path, MAX_FILE_BYTES.min(*remaining_bytes)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Re-check after the file read so a concurrently removed parent is
            // still reported as an ordinary source error.
            ensure_plain_directory_chain(root, parent)?;
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    validate_import_utf8(&bytes, relative)?;
    *remaining_bytes -= bytes.len() as u64;
    Ok(Some(bytes))
}

fn read_import_document(
    root: &Path,
    relative: &str,
    file_count: usize,
    remaining_bytes: &mut u64,
) -> io::Result<Vec<u8>> {
    if file_count >= MAX_FILES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "legacy workspace exceeds the 2048-file budget",
        ));
    }
    let path = confined_path(root, relative, false)?;
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "legacy document has no parent")
    })?;
    ensure_plain_directory_chain(root, parent)?;
    let bytes = read_bounded_regular_file(&path, MAX_FILE_BYTES.min(*remaining_bytes)).map_err(
        |error| {
            if error.kind() == io::ErrorKind::NotFound {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("legacy accepted document is missing: {relative}"),
                )
            } else {
                error
            }
        },
    )?;
    validate_import_utf8(&bytes, relative)?;
    *remaining_bytes -= bytes.len() as u64;
    Ok(bytes)
}

fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn project_id(identity: &str) -> String {
    sha256_hex(identity.as_bytes())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn read_bounded_regular_file(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || crate::memory::is_reparse_point(&metadata)
        || !metadata.is_file()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "legacy import path is not a regular file: {}",
                path.display()
            ),
        ));
    }
    if metadata.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "legacy import file exceeds the size cap: {}",
                path.display()
            ),
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    fs::File::open(path)?
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "legacy import file exceeds the size cap: {}",
                path.display()
            ),
        ));
    }
    Ok(bytes)
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

fn validate_import_utf8(bytes: &[u8], relative: &str) -> io::Result<()> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("legacy workspace file has a UTF-8 BOM: {relative}"),
        ));
    }
    std::str::from_utf8(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("legacy workspace file is not UTF-8 text: {relative}"),
        )
    })?;
    Ok(())
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

#[derive(Debug)]
struct PendingWorkspacePromotion {
    relative: String,
    base: Option<String>,
    after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextEdit {
    start: usize,
    end: usize,
    replacement: String,
}

fn workspace_snapshot_content<'a>(
    path: &str,
    snapshot: &'a Snapshot,
    side: &str,
) -> io::Result<Option<&'a str>> {
    match snapshot {
        Snapshot::FileContent { content } | Snapshot::DeletedFile { content, .. } => {
            Ok(Some(content))
        }
        Snapshot::Created | Snapshot::Deleted => Ok(None),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("workspace journal `{path}` has no promotable {side} snapshot"),
        )),
    }
}

fn merge_workspace_content(
    path: &str,
    base: Option<&str>,
    after: Option<&str>,
    current_bytes: Option<&[u8]>,
) -> io::Result<Option<Vec<u8>>> {
    let current = current_bytes
        .map(|bytes| {
            std::str::from_utf8(bytes).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("canonical workspace file `{path}` is not UTF-8: {error}"),
                )
            })
        })
        .transpose()?;

    if current == base {
        return Ok(after.map(|content| content.as_bytes().to_vec()));
    }
    if after == base || current == after {
        return Ok(current.map(|content| content.as_bytes().to_vec()));
    }

    match (base, after, current) {
        (Some(base), Some(after), Some(current)) => {
            merge_text_changes(path, base, after, current).map(|merged| Some(merged.into_bytes()))
        }
        _ => Err(workspace_conflict(path)),
    }
}

pub(crate) fn read_source_baseline(
    workspace_root: &Path,
    relative: &str,
) -> io::Result<Option<String>> {
    let source_root = workspace_root.join(SOURCE_DIR);
    ensure_plain_directory(&source_root)?;
    let relative = relative.strip_prefix("src/").unwrap_or(relative);
    let path = confined_path(&source_root, relative, true)?;
    optional_regular_file_bytes(&path)?
        .map(|bytes| decode_utf8(bytes, relative))
        .transpose()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactTextEdit {
    pub old_text: String,
    pub new_text: String,
}

pub(crate) fn apply_exact_text_edits(
    path: &str,
    content: &str,
    edits: &[ExactTextEdit],
) -> io::Result<String> {
    if edits.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("file_edit requires at least one edit for `{path}`"),
        ));
    }

    let mut candidate = content.to_owned();
    for (index, edit) in edits.iter().enumerate() {
        if edit.old_text.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("file_edit edit {index} for `{path}` has empty old_text"),
            ));
        }

        let mut matches = candidate
            .match_indices(&edit.old_text)
            .map(|(offset, _)| offset);
        let Some(offset) = matches.next() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("file_edit edit {index} old_text was not found in `{path}`"),
            ));
        };
        if matches.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "file_edit edit {index} old_text occurs more than once in `{path}`; include more surrounding context"
                ),
            ));
        }
        drop(matches);
        candidate.replace_range(offset..offset + edit.old_text.len(), &edit.new_text);
    }
    Ok(candidate)
}

pub(crate) fn merge_concurrent_text(
    path: &str,
    base: &str,
    ours: &str,
    current: &str,
) -> io::Result<String> {
    if current == base {
        return Ok(ours.to_string());
    }
    if ours == base || ours == current {
        return Ok(current.to_string());
    }
    merge_text_changes(path, base, ours, current)
}

fn merge_text_changes(path: &str, base: &str, ours: &str, theirs: &str) -> io::Result<String> {
    let ours = text_edits(base, ours);
    let theirs = text_edits(base, theirs);
    for ours_edit in &ours {
        for theirs_edit in &theirs {
            if ours_edit != theirs_edit && text_edits_overlap(ours_edit, theirs_edit) {
                return Err(workspace_conflict(path));
            }
        }
    }

    let mut edits = ours;
    for edit in theirs {
        if !edits.contains(&edit) {
            edits.push(edit);
        }
    }
    edits.sort_by_key(|edit| (edit.start, edit.end));

    let base_lines = split_lines(base);
    let mut merged = String::new();
    let mut cursor = 0;
    for edit in edits {
        if edit.start < cursor || edit.end > base_lines.len() {
            return Err(workspace_conflict(path));
        }
        merged.extend(base_lines[cursor..edit.start].iter().copied());
        merged.push_str(&edit.replacement);
        cursor = edit.end;
    }
    merged.extend(base_lines[cursor..].iter().copied());
    Ok(merged)
}

fn text_edits(base: &str, changed: &str) -> Vec<TextEdit> {
    let changed_lines = split_lines(changed);
    TextDiff::from_lines(base, changed)
        .ops()
        .iter()
        .filter(|operation| operation.tag() != DiffTag::Equal)
        .map(|operation| {
            let old = operation.old_range();
            let new = operation.new_range();
            TextEdit {
                start: old.start,
                end: old.end,
                replacement: changed_lines[new].concat(),
            }
        })
        .collect()
}

fn split_lines(content: &str) -> Vec<&str> {
    content.split_inclusive('\n').collect()
}

fn text_edits_overlap(left: &TextEdit, right: &TextEdit) -> bool {
    let left_insert = left.start == left.end;
    let right_insert = right.start == right.end;
    match (left_insert, right_insert) {
        (true, true) => left.start == right.start,
        (true, false) => left.start > right.start && left.start < right.end,
        (false, true) => right.start > left.start && right.start < left.end,
        (false, false) => left.start < right.end && right.start < left.end,
    }
}

fn workspace_conflict(path: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "ConcurrentWriteConflict: `{path}` changed in the same area since this session read it"
        ),
    )
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

fn validate_mutable_document_path(relative: &str) -> io::Result<String> {
    let normalized = normalize_relative_path(relative, false)?;
    let allowed = normalized.starts_with("specs/")
        || normalized.starts_with("decisions/")
        || normalized.starts_with("worklog/");
    if !allowed || !normalized.ends_with(".md") {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("harness document path is not mutable: `{normalized}`"),
        ));
    }
    Ok(normalized)
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
    if metadata.file_type().is_symlink()
        || crate::memory::is_reparse_point(&metadata)
        || !metadata.is_dir()
    {
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
    use crate::native_project::{NativeProject, ProjectManifest};
    use crate::source_snapshot::ProjectSnapshotFile as EpsSnapshotFile;
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
        let project_root = base.join("project");
        fs::create_dir_all(&project_root).unwrap();
        let mut config = dirs.load_config().unwrap();
        config.project_path = project_root.to_string_lossy().into_owned();
        dirs.save_config(&config).unwrap();
        (base, WorkspaceManager::new(dirs))
    }

    fn snapshot(manager: &WorkspaceManager) -> EpsSnapshot {
        let identity = manager.dirs.load_config().unwrap().project_path;
        EpsSnapshot {
            project: "Example".to_string(),
            identity,
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
                EpsSnapshotFile {
                    path: "python/boot.py".to_string(),
                    ftype: "CUIPy".to_string(),
                    content: Some("from eudplib import EUDVariable".to_string()),
                },
            ],
        }
    }

    fn native_target(base: &Path) -> NativeProject {
        let root = base.join("native");
        fs::create_dir_all(root.join("maps")).unwrap();
        fs::write(root.join("maps/source.scx"), b"map").unwrap();
        NativeProject::create(
            &root,
            ProjectManifest {
                schema_version: crate::native_project::PROJECT_SCHEMA_VERSION,
                name: "Native".to_string(),
                source_map: "maps/source.scx".to_string(),
                output_map: "build/output.scx".to_string(),
                main_file: "src/main.eps".to_string(),
                settings: Default::default(),
                plugins: Vec::new(),
                python_entrypoints: Vec::new(),
                python_dependencies: Vec::new(),
                python_lock: None,
                editor_compatibility: None,
            },
        )
        .unwrap()
    }

    fn write_legacy_state(
        manager: &WorkspaceManager,
        id: &str,
        project: &str,
        documents: BTreeMap<String, TrustedDocumentState>,
        approved_plans: BTreeMap<String, TrustedPlanState>,
    ) {
        let root = manager.dirs.workspaces_dir().join(id);
        fs::create_dir_all(&root).unwrap();
        let state = TrustedWorkspaceState {
            version: 1,
            id: id.to_string(),
            project: project.to_string(),
            identity_hash: id.to_string(),
            documents,
            approved_plans,
        };
        let path = manager
            .dirs
            .workspace_state_dir()
            .join("projects")
            .join(format!("{id}.json"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        atomic_write(&path, &serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    }

    #[test]
    fn exact_text_edits_apply_in_order_without_rewriting_untouched_content() {
        let source =
            "function first() {\n    oldCall();\n}\n\nfunction second() {\n    keep();\n}\n";
        let edited = apply_exact_text_edits(
            "main.eps",
            source,
            &[
                ExactTextEdit {
                    old_text: "oldCall();".into(),
                    new_text: "newCall();".into(),
                },
                ExactTextEdit {
                    old_text: "newCall();\n}".into(),
                    new_text: "newCall();\n    addedCall();\n}".into(),
                },
            ],
        )
        .unwrap();

        assert_eq!(
            edited,
            "function first() {\n    newCall();\n    addedCall();\n}\n\nfunction second() {\n    keep();\n}\n"
        );
    }

    #[test]
    fn exact_text_edits_reject_empty_missing_and_ambiguous_matches() {
        let source = "same();\nsame();\n";
        for edit in [
            ExactTextEdit {
                old_text: String::new(),
                new_text: "replacement".into(),
            },
            ExactTextEdit {
                old_text: "missing();".into(),
                new_text: "replacement".into(),
            },
            ExactTextEdit {
                old_text: "same();".into(),
                new_text: "replacement".into(),
            },
        ] {
            assert!(apply_exact_text_edits("main.eps", source, &[edit]).is_err());
        }
        assert!(apply_exact_text_edits("main.eps", source, &[]).is_err());
    }

    #[test]
    fn prepare_keeps_canonical_documents_project_local_and_session_source_machine_local() {
        let (base, manager) = manager("prepare");
        let initial = snapshot(&manager);
        let workspace = manager.prepare_snapshot(&initial).unwrap();

        for directory in DOCUMENT_DIRS {
            assert!(workspace.root.join(directory).is_dir());
        }
        assert!(!workspace.root.join(SOURCE_DIR).exists());
        let session = manager
            .prepare_session_snapshot(&initial, "source-check")
            .unwrap();
        assert_eq!(
            fs::read_to_string(session.root.join("source/main.eps")).unwrap(),
            "function onPluginStart() {}"
        );
        assert!(session
            .root
            .starts_with(manager.dirs.session_workspaces_dir()));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn search_files_matches_text_paths_and_contents_case_insensitively() {
        let (base, manager) = manager("search");
        let workspace = manager.prepare_snapshot(&snapshot(&manager)).unwrap();
        write_atomic_bytes(
            &workspace.root.join("specs/combat.md"),
            b"Confirmed behavior.",
        )
        .unwrap();
        fs::write(workspace.root.join("decisions/binary.dat"), [0xff, 0x00]).unwrap();

        assert_eq!(
            manager.search_files(&workspace.id, "COMBAT").unwrap(),
            vec!["specs/combat.md"]
        );
        assert_eq!(
            manager
                .search_files(&workspace.id, "confirmed behavior")
                .unwrap(),
            vec!["specs/combat.md"]
        );
        assert!(manager
            .search_files(&workspace.id, "PLUGINSTART")
            .unwrap()
            .is_empty());
        assert!(manager
            .search_files(&workspace.id, "eudvariable")
            .unwrap()
            .is_empty());
        assert!(manager
            .search_files(&workspace.id, "binary.dat")
            .unwrap()
            .is_empty());
        assert!(manager
            .search_files(&workspace.id, "  ")
            .unwrap()
            .is_empty());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn baseline_diff_excludes_source_and_restores_writable_files() {
        let (base, manager) = manager("diff");
        let workspace = manager.prepare_snapshot(&snapshot(&manager)).unwrap();
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
        let workspace = manager.prepare_snapshot(&snapshot(&manager)).unwrap();
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
        assert!(listed.iter().all(|file| !file.path.starts_with("source/")));

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
            .starts_with(fs::canonicalize(base.join("project/.eud-agent/state")).unwrap()));
        assert!(!manager
            .state_path(&workspace.id)
            .unwrap()
            .starts_with(base.join("roaming")));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn paths_cannot_escape_or_target_generated_dirs() {
        let (base, manager) = manager("paths");
        let workspace = manager.prepare_snapshot(&snapshot(&manager)).unwrap();

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
        let workspace = manager.prepare_snapshot(&snapshot(&manager)).unwrap();
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
        let initial = snapshot(&manager);
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
        let canonical = manager.prepare_snapshot(&snapshot(&manager)).unwrap();
        write_atomic_bytes(&canonical.root.join("specs/game.md"), b"accepted").unwrap();
        manager
            .record_plan_approval(&canonical.id, "req-approved", 1, "# Approved")
            .unwrap();
        let session = manager
            .prepare_session_snapshot(&snapshot(&manager), "session-c")
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

    #[test]
    fn concurrent_session_accepts_merge_non_overlapping_changes() {
        let (base, manager) = manager("session-merge");
        let initial = snapshot(&manager);
        let canonical = manager.prepare_snapshot(&initial).unwrap();
        write_atomic_bytes(
            &canonical.root.join("specs/game.md"),
            b"# Game\n\nalpha: old\nbeta: old\n",
        )
        .unwrap();
        let session_a = manager
            .prepare_session_snapshot(&initial, "session-a")
            .unwrap();
        let session_b = manager
            .prepare_session_snapshot(&initial, "session-b")
            .unwrap();
        let baseline_a = manager.begin_turn(&session_a, "req-merge-a").unwrap();
        let baseline_b = manager.begin_turn(&session_b, "req-merge-b").unwrap();
        write_atomic_bytes(
            &session_a.root.join("specs/game.md"),
            b"# Game\n\nalpha: session-a\nbeta: old\n",
        )
        .unwrap();
        write_atomic_bytes(
            &session_b.root.join("specs/game.md"),
            b"# Game\n\nalpha: old\nbeta: session-b\n",
        )
        .unwrap();

        let journal = JournalStore::new(base.join("roaming"));
        WorkspaceTurnRecorder::new(manager.clone(), baseline_a, journal.clone())
            .finish()
            .unwrap();
        WorkspaceTurnRecorder::new(manager.clone(), baseline_b, journal.clone())
            .finish()
            .unwrap();
        let entries_a = JournalStore::load(base.join("roaming"), "req-merge-a")
            .unwrap()
            .entries;
        let entries_b = JournalStore::load(base.join("roaming"), "req-merge-b")
            .unwrap()
            .entries;

        manager
            .record_accepted_entries("req-merge-a", &entries_a)
            .unwrap();
        manager
            .record_accepted_entries("req-merge-b", &entries_b)
            .unwrap();

        assert_eq!(
            fs::read_to_string(canonical.root.join("specs/game.md")).unwrap(),
            "# Game\n\nalpha: session-a\nbeta: session-b\n"
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn concurrent_session_accept_reports_overlapping_change_without_overwrite() {
        let (base, manager) = manager("session-conflict");
        let initial = snapshot(&manager);
        let canonical = manager.prepare_snapshot(&initial).unwrap();
        write_atomic_bytes(&canonical.root.join("specs/game.md"), b"# Game\n").unwrap();
        let session_a = manager
            .prepare_session_snapshot(&initial, "session-a")
            .unwrap();
        let session_b = manager
            .prepare_session_snapshot(&initial, "session-b")
            .unwrap();
        let baseline_a = manager.begin_turn(&session_a, "req-conflict-a").unwrap();
        let baseline_b = manager.begin_turn(&session_b, "req-conflict-b").unwrap();
        write_atomic_bytes(
            &session_a.root.join("specs/game.md"),
            b"# Game\n\nalpha: session-a\n",
        )
        .unwrap();
        write_atomic_bytes(
            &session_b.root.join("specs/game.md"),
            b"# Game\n\nalpha: session-b\n",
        )
        .unwrap();

        let journal = JournalStore::new(base.join("roaming"));
        WorkspaceTurnRecorder::new(manager.clone(), baseline_a, journal.clone())
            .finish()
            .unwrap();
        WorkspaceTurnRecorder::new(manager.clone(), baseline_b, journal.clone())
            .finish()
            .unwrap();
        let entries_a = JournalStore::load(base.join("roaming"), "req-conflict-a")
            .unwrap()
            .entries;
        let entries_b = JournalStore::load(base.join("roaming"), "req-conflict-b")
            .unwrap()
            .entries;

        manager
            .record_accepted_entries("req-conflict-a", &entries_a)
            .unwrap();
        let error = manager
            .record_accepted_entries("req-conflict-b", &entries_b)
            .unwrap_err();

        assert!(error.to_string().contains("specs/game.md"));
        assert_eq!(
            fs::read_to_string(canonical.root.join("specs/game.md")).unwrap(),
            "# Game\n\nalpha: session-a\n"
        );
        fs::remove_dir_all(base).ok();
    }
    #[test]
    fn project_local_workspace_id_and_documents_survive_folder_move() {
        let (base, manager) = manager("local-relocation");
        let target = native_target(&base);
        let original_root = target.root().to_path_buf();
        let prepared = manager
            .prepare_snapshot(&EpsSnapshot {
                project: target.manifest().name.clone(),
                identity: original_root.to_string_lossy().into_owned(),
                files: Vec::new(),
            })
            .unwrap();
        let id = prepared.id.clone();
        fs::write(prepared.root.join("specs/keep.md"), b"keep").unwrap();
        let moved_root = base.join("moved-native");
        fs::rename(&original_root, &moved_root).unwrap();
        let moved = manager
            .prepare_snapshot(&EpsSnapshot {
                project: target.manifest().name.clone(),
                identity: moved_root.to_string_lossy().into_owned(),
                files: Vec::new(),
            })
            .unwrap();
        assert_eq!(moved.id, id);
        assert_eq!(fs::read(moved.root.join("specs/keep.md")).unwrap(), b"keep");
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn e3s_import_keeps_healthy_items_and_reports_missing_siblings() {
        let (base, manager) = manager("local-mixed-import");
        let source = base.join("legacy/project.e3s");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"e3s").unwrap();
        let source = fs::canonicalize(source).unwrap();
        let source_id = project_id(&source.to_string_lossy());
        let source_root = manager.dirs.workspaces_dir().join(&source_id);
        fs::create_dir_all(source_root.join("specs")).unwrap();
        fs::write(source_root.join("specs/healthy.md"), b"healthy").unwrap();
        fs::create_dir_all(source_root.join("plans")).unwrap();
        fs::write(source_root.join("plans/approval-only.md"), b"# Approval").unwrap();
        let mut documents = BTreeMap::new();
        documents.insert(
            "specs/healthy.md".to_string(),
            TrustedDocumentState {
                revision: 1,
                state: "accepted".to_string(),
                accepted_at: 1,
                request_id: "healthy".to_string(),
            },
        );
        documents.insert(
            "specs/missing.md".to_string(),
            TrustedDocumentState {
                revision: 1,
                state: "accepted".to_string(),
                accepted_at: 1,
                request_id: "missing".to_string(),
            },
        );
        documents.insert(
            "specs/deleted.md".to_string(),
            TrustedDocumentState {
                revision: 2,
                state: "deleted".to_string(),
                accepted_at: 1,
                request_id: "deleted".to_string(),
            },
        );
        let mut approvals = BTreeMap::new();
        approvals.insert(
            "approval-only".to_string(),
            TrustedPlanState {
                revision: 1,
                approved_at: 1,
                markdown_sha256: sha256_hex(b"# Approval"),
            },
        );
        write_legacy_state(
            &manager,
            &source_id,
            &source.to_string_lossy(),
            documents,
            approvals,
        );
        let target = native_target(&base);
        let mut issues = Vec::new();
        let import = manager
            .import_legacy_harness(&source, &target, &mut issues)
            .unwrap();
        assert!(issues
            .iter()
            .any(|issue| issue.path.ends_with("specs/missing.md")));
        assert_eq!(
            fs::read(
                target
                    .root()
                    .join(PROJECT_AGENT_DIR)
                    .join(PROJECT_WORKSPACE_DIR)
                    .join("specs/healthy.md")
            )
            .unwrap(),
            b"healthy"
        );
        import.commit();
        let local_state = target
            .root()
            .join(PROJECT_AGENT_DIR)
            .join(PROJECT_STATE_DIR)
            .join(PROJECT_STATE_FILE);
        let imported_state: TrustedWorkspaceState =
            serde_json::from_slice(&fs::read(local_state).unwrap()).unwrap();
        assert_eq!(
            imported_state.documents["specs/deleted.md"].state,
            "deleted"
        );
        assert!(target
            .root()
            .join(PROJECT_AGENT_DIR)
            .join(PROJECT_WORKSPACE_DIR)
            .join("plans/approval-only.md")
            .is_file());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn import_rollback_preserves_local_conflicts_and_removes_only_new_writes() {
        let (base, manager) = manager("local-import-rollback");
        let source = base.join("legacy/project.e3s");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"e3s").unwrap();
        let source = fs::canonicalize(source).unwrap();
        let source_id = project_id(&source.to_string_lossy());
        let source_root = manager.dirs.workspaces_dir().join(&source_id);
        fs::create_dir_all(source_root.join("specs")).unwrap();
        fs::write(source_root.join("specs/new.md"), b"new").unwrap();
        fs::write(source_root.join("specs/conflict.md"), b"legacy").unwrap();
        let mut documents = BTreeMap::new();
        for path in ["specs/new.md", "specs/conflict.md"] {
            documents.insert(
                path.to_string(),
                TrustedDocumentState {
                    revision: 1,
                    state: "accepted".to_string(),
                    accepted_at: 1,
                    request_id: "req".to_string(),
                },
            );
        }
        write_legacy_state(
            &manager,
            &source_id,
            &source.to_string_lossy(),
            documents,
            BTreeMap::new(),
        );
        let target = native_target(&base);
        let local_root = target
            .root()
            .join(PROJECT_AGENT_DIR)
            .join(PROJECT_WORKSPACE_DIR);
        fs::create_dir_all(local_root.join("specs")).unwrap();
        fs::write(local_root.join("specs/conflict.md"), b"local").unwrap();
        let mut issues = Vec::new();
        let import = manager
            .import_legacy_harness(&source, &target, &mut issues)
            .unwrap();
        assert!(issues
            .iter()
            .any(|issue| issue.path.ends_with("conflict.md")));
        assert_eq!(fs::read(local_root.join("specs/new.md")).unwrap(), b"new");
        import.rollback().unwrap();
        assert_eq!(
            fs::read(local_root.join("specs/conflict.md")).unwrap(),
            b"local"
        );
        assert!(!local_root.join("specs/new.md").exists());
        fs::remove_dir_all(base).ok();
    }
}
