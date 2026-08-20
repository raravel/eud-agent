//! Agent-only epScript preflight service.
//!
//! A coherent editor snapshot is mirrored under LocalAppData, complete candidate
//! batches are overlaid in request-owned analysis directories, and a pinned Node
//! adapter is called over Content-Length framed JSON. Every analyzer/process
//! failure degrades to a structured `skipped` result; candidate/path errors remain
//! corrective tool errors.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bridge_io::{EpsSnapshot, SendOpts};
use crate::config::DataDirs;
use crate::workspace::{apply_exact_text_edits, ExactTextEdit};

pub const MAX_DIAGNOSTICS: usize = 200;
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024;
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 8 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;
const COLD_REQUEST_DEADLINE: Duration = Duration::from_secs(15);
const WARM_REQUEST_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpsCandidate {
    pub path: String,
    pub code: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EpsCandidateInput {
    pub path: String,
    pub code: Option<String>,
    pub edits: Option<Vec<ExactTextEdit>>,
}

enum ValidatedCandidateInput {
    Complete(EpsCandidate),
    Edits {
        path: String,
        edits: Vec<ExactTextEdit>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpsDiagnostic {
    pub path: String,
    pub line: u32,
    pub character: u32,
    #[serde(rename = "endLine")]
    pub end_line: u32,
    #[serde(rename = "endCharacter")]
    pub end_character: u32,
    pub severity: String,
    pub source: String,
    pub code: Option<serde_json::Value>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpsImport {
    pub from: String,
    pub module: String,
    pub to: String,
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    NodeNotFound,
    AdapterMissing,
    AdapterStartFailed,
    AdapterCrashed,
    AdapterTimeout,
    AdapterProtocolError,
    SnapshotUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EpsCheckResult {
    Diagnosed {
        project: String,
        #[serde(rename = "checkedFiles")]
        checked_files: Vec<String>,
        diagnostics: Vec<EpsDiagnostic>,
        imports: Vec<EpsImport>,
        truncated: bool,
        #[serde(rename = "omittedDiagnostics")]
        omitted_diagnostics: usize,
        #[serde(rename = "omittedMessageBytes")]
        omitted_message_bytes: usize,
    },
    Skipped {
        reason: SkipReason,
        diagnostics: Vec<EpsDiagnostic>,
        imports: Vec<EpsImport>,
    },
}

impl EpsCheckResult {
    fn skipped(reason: SkipReason) -> Self {
        Self::Skipped {
            reason,
            diagnostics: Vec::new(),
            imports: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzerSuccess {
    #[serde(rename = "checkedFiles")]
    pub checked_files: Vec<String>,
    pub diagnostics: Vec<EpsDiagnostic>,
    pub imports: Vec<EpsImport>,
    pub truncated: bool,
    #[serde(rename = "omittedDiagnostics")]
    pub omitted_diagnostics: usize,
    #[serde(rename = "omittedMessageBytes")]
    pub omitted_message_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzerError {
    pub reason: SkipReason,
    pub message: String,
}

impl AnalyzerError {
    fn new(reason: SkipReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct AnalyzerRequest {
    #[serde(rename = "projectId")]
    pub project_id: String,
    pub root: String,
    pub candidates: Vec<EpsCandidate>,
    pub unreadable: Vec<UnreadableFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnreadableFile {
    pub path: String,
    pub ftype: String,
}

/// Analyzer boundary used by the production child client and deterministic tests.
pub trait EpsAnalyzer: Send + Sync {
    fn analyze(&self, request: &AnalyzerRequest) -> Result<AnalyzerSuccess, AnalyzerError>;
    fn reset_project(&self);
}

trait SnapshotProvider: Send + Sync {
    fn snapshot(&self) -> Result<EpsSnapshot, String>;
}

#[derive(Clone)]
struct ConfiguredSnapshotProvider {
    dirs: DataDirs,
}

impl SnapshotProvider for ConfiguredSnapshotProvider {
    fn snapshot(&self) -> Result<EpsSnapshot, String> {
        let bridge = crate::ipc::bridge_from_config(&self.dirs)?;
        bridge
            .snapshot_eps(&SendOpts::default(), None)
            .map_err(|error| error.to_string())
    }
}

#[derive(Default)]
struct PreflightState {
    request_id: Option<String>,
    project: Option<String>,
    project_id: Option<String>,
    mirror_root: Option<PathBuf>,
    unreadable: Vec<UnreadableFile>,
    snapshot_ready: bool,
    suppressed: Option<SkipReason>,
}

/// Request-aware snapshot/mirror owner. Each session owns one instance; the
/// shared analyzer process provides its own serialization.
pub struct EpsPreflight {
    dirs: DataDirs,
    analyzer: Arc<dyn EpsAnalyzer>,
    snapshot_provider: Arc<dyn SnapshotProvider>,
    state: Mutex<PreflightState>,
}

impl EpsPreflight {
    pub fn new(dirs: DataDirs, analyzer: Arc<dyn EpsAnalyzer>) -> Self {
        Self {
            snapshot_provider: Arc::new(ConfiguredSnapshotProvider { dirs: dirs.clone() }),
            dirs,
            analyzer,
            state: Mutex::new(PreflightState::default()),
        }
    }

    #[cfg(test)]
    fn with_snapshot_provider(
        dirs: DataDirs,
        analyzer: Arc<dyn EpsAnalyzer>,
        snapshot_provider: Arc<dyn SnapshotProvider>,
    ) -> Self {
        Self {
            dirs,
            analyzer,
            snapshot_provider,
            state: Mutex::new(PreflightState::default()),
        }
    }

    pub fn begin_request(&self, request_id: &str) {
        let mut state = self.state.lock();
        state.request_id = Some(request_id.to_string());
        state.snapshot_ready = false;
        state.suppressed = None;
        state.unreadable.clear();
    }

    pub fn check(
        &self,
        request_id: &str,
        candidates: Vec<EpsCandidate>,
    ) -> Result<EpsCheckResult, String> {
        let candidates = validate_candidates(candidates)?
            .into_iter()
            .map(ValidatedCandidateInput::Complete)
            .collect();
        self.check_validated(request_id, candidates)
    }

    pub(crate) fn check_inputs(
        &self,
        request_id: &str,
        candidates: Vec<EpsCandidateInput>,
    ) -> Result<EpsCheckResult, String> {
        let candidates = validate_candidate_inputs(candidates)?;
        self.check_validated(request_id, candidates)
    }

    fn check_validated(
        &self,
        request_id: &str,
        candidates: Vec<ValidatedCandidateInput>,
    ) -> Result<EpsCheckResult, String> {
        let mut state = self.state.lock();
        if state.request_id.as_deref() != Some(request_id) {
            state.request_id = Some(request_id.to_string());
            state.snapshot_ready = false;
            state.suppressed = None;
            state.unreadable.clear();
        }
        if let Some(reason) = state.suppressed {
            return Ok(EpsCheckResult::skipped(reason));
        }

        if !state.snapshot_ready {
            let snapshot = match self.snapshot_provider.snapshot() {
                Ok(snapshot) => snapshot,
                Err(_) => {
                    state.suppressed = Some(SkipReason::SnapshotUnavailable);
                    return Ok(EpsCheckResult::skipped(SkipReason::SnapshotUnavailable));
                }
            };
            let refreshed = match refresh_mirror(&self.dirs, &snapshot) {
                Ok(refreshed) => refreshed,
                Err(_) => {
                    state.suppressed = Some(SkipReason::SnapshotUnavailable);
                    return Ok(EpsCheckResult::skipped(SkipReason::SnapshotUnavailable));
                }
            };
            let project_changed =
                state.project_id.as_deref() != Some(refreshed.project_id.as_str());
            if project_changed {
                self.analyzer.reset_project();
            }
            state.project = Some(snapshot.project);
            state.project_id = Some(refreshed.project_id);
            state.mirror_root = Some(refreshed.mirror_root);
            state.unreadable = refreshed.unreadable;
            state.snapshot_ready = true;
        }

        let project = state.project.clone().unwrap_or_default();
        let project_id = state.project_id.clone().unwrap_or_default();
        let mirror_root = state
            .mirror_root
            .clone()
            .ok_or_else(|| "eps preflight mirror is unavailable".to_string())?;
        let candidates = resolve_candidate_inputs(&mirror_root, candidates)?;
        let analysis_root = match prepare_analysis_root(&mirror_root, &candidates) {
            Ok(root) => root,
            Err(error) => {
                state.suppressed = Some(SkipReason::SnapshotUnavailable);
                return if error.kind() == io::ErrorKind::InvalidInput {
                    Err(error.to_string())
                } else {
                    Ok(EpsCheckResult::skipped(SkipReason::SnapshotUnavailable))
                };
            }
        };
        let Some(root) = analysis_root.to_str().map(str::to_owned) else {
            let _ = fs::remove_dir_all(&analysis_root);
            state.suppressed = Some(SkipReason::AdapterStartFailed);
            return Ok(EpsCheckResult::skipped(SkipReason::AdapterStartFailed));
        };
        let request = AnalyzerRequest {
            project_id,
            root,
            candidates,
            unreadable: state.unreadable.clone(),
        };
        let analyzed = self.analyzer.analyze(&request);
        let _ = fs::remove_dir_all(&analysis_root);

        match analyzed {
            Ok(output) => {
                if validate_analyzer_success(&output).is_err() {
                    state.suppressed = Some(SkipReason::AdapterProtocolError);
                    return Ok(EpsCheckResult::skipped(SkipReason::AdapterProtocolError));
                }
                Ok(EpsCheckResult::Diagnosed {
                    project,
                    checked_files: output.checked_files,
                    diagnostics: output.diagnostics,
                    imports: output.imports,
                    truncated: output.truncated,
                    omitted_diagnostics: output.omitted_diagnostics,
                    omitted_message_bytes: output.omitted_message_bytes,
                })
            }
            Err(error) => {
                state.suppressed = Some(error.reason);
                Ok(EpsCheckResult::skipped(error.reason))
            }
        }
    }

    pub fn write_applied(&self, request_id: &str, path: &str, code: &str) {
        let Ok(project_path) = normalize_project_path(path) else {
            return;
        };
        let mut state = self.state.lock();
        if state.request_id.as_deref() != Some(request_id) || !state.snapshot_ready {
            return;
        }
        let Some(root) = state.mirror_root.as_ref() else {
            state.snapshot_ready = false;
            return;
        };
        if atomic_write_under_root(root, &project_path, code.as_bytes()).is_err() {
            state.snapshot_ready = false;
        }
    }

    pub fn rename_applied(&self, request_id: &str, from: &str, to: &str) {
        let from = normalize_project_path(from);
        let to = normalize_project_path(to);
        let mut state = self.state.lock();
        if state.request_id.as_deref() != Some(request_id) || !state.snapshot_ready {
            return;
        }
        let (Ok(from), Ok(to), Some(root)) = (from, to, state.mirror_root.as_ref()) else {
            state.snapshot_ready = false;
            return;
        };
        let source = confined_path(root, &from);
        let target = confined_path(root, &to);
        let update = source.and_then(|source| {
            target.and_then(|target| {
                if !source.exists() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "mirror source is missing",
                    ));
                }
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::rename(source, target)
            })
        });
        if update.is_err() {
            state.snapshot_ready = false;
        }
    }

    pub fn delete_applied(&self, request_id: &str, path: &str) {
        let Ok(project_path) = normalize_project_path(path) else {
            return;
        };
        let mut state = self.state.lock();
        if state.request_id.as_deref() != Some(request_id) || !state.snapshot_ready {
            return;
        }
        let Some(root) = state.mirror_root.as_ref() else {
            state.snapshot_ready = false;
            return;
        };
        let removal = confined_path(root, &project_path).and_then(|target| {
            if target.exists() {
                fs::remove_file(target)
            } else {
                Ok(())
            }
        });
        if removal.is_err() {
            state.snapshot_ready = false;
        }
    }

    pub fn invalidate(&self, request_id: &str) {
        let mut state = self.state.lock();
        if state.request_id.as_deref() == Some(request_id) {
            state.snapshot_ready = false;
        }
    }
}

struct RefreshedMirror {
    project_id: String,
    mirror_root: PathBuf,
    unreadable: Vec<UnreadableFile>,
}

fn refresh_mirror(dirs: &DataDirs, snapshot: &EpsSnapshot) -> io::Result<RefreshedMirror> {
    if snapshot.project.is_empty() || snapshot.identity.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "snapshot project display name or identity is empty",
        ));
    }
    let workspace = dirs.lsp_workspaces_dir();
    fs::create_dir_all(&workspace)?;
    let workspace = fs::canonicalize(workspace)?;
    let project_id = hex_sha256(snapshot.identity.as_bytes());
    let project_dir = workspace.join(&project_id);
    fs::create_dir_all(&project_dir)?;
    ensure_contained_existing(&workspace, &project_dir)?;

    let staged = project_dir.join(format!("snapshot-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&staged)?;
    let mut unreadable = Vec::new();
    let mut seen = HashMap::<String, String>::new();
    let populate = (|| -> io::Result<()> {
        for file in &snapshot.files {
            let project_path = normalize_project_path(&file.path)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let key = project_path.to_lowercase();
            if let Some(previous) = seen.insert(key, project_path.clone()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "snapshot paths collide case-insensitively: {previous} and {project_path}"
                    ),
                ));
            }
            if let Some(content) = &file.content {
                atomic_write_under_root(&staged, &project_path, content.as_bytes())?;
            } else {
                unreadable.push(UnreadableFile {
                    path: project_path,
                    ftype: file.ftype.clone(),
                });
            }
        }
        Ok(())
    })();
    if let Err(error) = populate {
        let _ = fs::remove_dir_all(&staged);
        return Err(error);
    }

    let mirror = project_dir.join("mirror");
    if mirror.exists() {
        fs::remove_dir_all(&mirror)?;
    }
    fs::rename(&staged, &mirror)?;
    ensure_contained_existing(&workspace, &mirror)?;
    Ok(RefreshedMirror {
        project_id,
        mirror_root: mirror,
        unreadable,
    })
}

fn prepare_analysis_root(mirror_root: &Path, candidates: &[EpsCandidate]) -> io::Result<PathBuf> {
    let project_dir = mirror_root.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "mirror root has no project parent",
        )
    })?;
    let analysis = project_dir.join(format!("analysis-{}", uuid::Uuid::new_v4()));
    copy_directory(mirror_root, &analysis)?;
    let overlay = overlay_candidates(&analysis, candidates);
    if let Err(error) = overlay {
        let _ = fs::remove_dir_all(&analysis);
        return Err(error);
    }
    Ok(analysis)
}

fn copy_directory(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir(destination)?;
    let mut stack = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let target = to.join(entry.file_name());
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                fs::create_dir(&target)?;
                stack.push((entry.path(), target));
            } else if file_type.is_file() {
                fs::copy(entry.path(), target)?;
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "mirror contains a non-file entry",
                ));
            }
        }
    }
    Ok(())
}

fn overlay_candidates(root: &Path, candidates: &[EpsCandidate]) -> io::Result<()> {
    let mut staged = Vec::with_capacity(candidates.len());
    let staging = (|| -> io::Result<()> {
        for (index, candidate) in candidates.iter().enumerate() {
            let target = confined_path(root, &candidate.path)?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let temporary = target.with_extension(format!("eps.eud-agent-{index}.tmp"));
            fs::write(&temporary, candidate.code.as_bytes())?;
            staged.push((temporary, target));
        }
        Ok(())
    })();
    if let Err(error) = staging {
        for (temporary, _) in &staged {
            let _ = fs::remove_file(temporary);
        }
        return Err(error);
    }
    for (temporary, target) in &staged {
        if target.exists() {
            fs::remove_file(target)?;
        }
        if let Err(error) = fs::rename(temporary, target) {
            for (remaining, _) in &staged {
                let _ = fs::remove_file(remaining);
            }
            return Err(error);
        }
    }
    Ok(())
}

fn atomic_write_under_root(root: &Path, project_path: &str, bytes: &[u8]) -> io::Result<()> {
    let target = confined_path(root, project_path)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = target.with_extension("eps.eud-agent.tmp");
    fs::write(&temporary, bytes)?;
    if target.exists() {
        fs::remove_file(&target)?;
    }
    fs::rename(temporary, target)
}

fn confined_path(root: &Path, project_path: &str) -> io::Result<PathBuf> {
    let normalized = normalize_project_path(project_path)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let target = normalized
        .split('/')
        .fold(root.to_path_buf(), |path, segment| path.join(segment));
    if !target.starts_with(root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "project path escapes mirror root",
        ));
    }
    Ok(target)
}

fn ensure_contained_existing(root: &Path, target: &Path) -> io::Result<()> {
    let target = fs::canonicalize(target)?;
    if target.starts_with(root) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "resolved mirror path escapes LocalAppData workspace root",
        ))
    }
}

pub(crate) fn normalize_project_path(value: &str) -> Result<String, String> {
    if value.is_empty() || value.contains('\0') || value.contains('\\') {
        return Err("path must be a non-empty project-relative path using '/' separators".into());
    }
    let path = Path::new(value);
    if path.is_absolute() || value.starts_with('/') || has_windows_prefix(value) {
        return Err(format!("path must be project-relative: {value}"));
    }
    let segments: Vec<&str> = value.split('/').collect();
    if segments
        .iter()
        .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
    {
        return Err(format!("path contains an invalid segment: {value}"));
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("path is not normalized: {value}"));
    }
    if !value.to_lowercase().ends_with(".eps") {
        return Err(format!("path must end in .eps: {value}"));
    }
    Ok(segments.join("/"))
}

fn has_windows_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn validate_candidate_inputs(
    candidates: Vec<EpsCandidateInput>,
) -> Result<Vec<ValidatedCandidateInput>, String> {
    if candidates.is_empty() {
        return Err("eps_check requires at least one candidate file".to_string());
    }
    let mut seen = HashMap::<String, String>::new();
    let mut validated = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let path = normalize_project_path(&candidate.path)?;
        let key = path.to_lowercase();
        if let Some(previous) = seen.insert(key, path.clone()) {
            return Err(format!(
                "candidate paths collide case-insensitively: {previous} and {path}"
            ));
        }
        match (candidate.code, candidate.edits) {
            (Some(code), None) => {
                validated.push(ValidatedCandidateInput::Complete(EpsCandidate {
                    path,
                    code,
                }));
            }
            (None, Some(edits)) if !edits.is_empty() => {
                validated.push(ValidatedCandidateInput::Edits { path, edits });
            }
            (None, Some(_)) => {
                return Err(format!(
                    "eps_check edit candidate `{path}` requires at least one edit"
                ));
            }
            (None, None) => {
                return Err(format!(
                    "eps_check candidate `{path}` requires exactly one of code or edits"
                ));
            }
            (Some(_), Some(_)) => {
                return Err(format!(
                    "eps_check candidate `{path}` cannot contain both code and edits"
                ));
            }
        }
    }
    Ok(validated)
}

fn resolve_candidate_inputs(
    mirror_root: &Path,
    candidates: Vec<ValidatedCandidateInput>,
) -> Result<Vec<EpsCandidate>, String> {
    candidates
        .into_iter()
        .map(|candidate| match candidate {
            ValidatedCandidateInput::Complete(candidate) => Ok(candidate),
            ValidatedCandidateInput::Edits { path, edits } => {
                let source_path =
                    confined_path(mirror_root, &path).map_err(|error| error.to_string())?;
                let source = fs::read_to_string(&source_path).map_err(|error| {
                    format!("cannot apply eps_check edits to `{path}`: {error}")
                })?;
                let code = apply_exact_text_edits(&path, &source, &edits)
                    .map_err(|error| error.to_string())?;
                Ok(EpsCandidate { path, code })
            }
        })
        .collect()
}

fn validate_candidates(candidates: Vec<EpsCandidate>) -> Result<Vec<EpsCandidate>, String> {
    if candidates.is_empty() {
        return Err("eps_check requires at least one complete candidate file".to_string());
    }
    let mut seen = HashMap::<String, String>::new();
    let mut validated = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let path = normalize_project_path(&candidate.path)?;
        let key = path.to_lowercase();
        if let Some(previous) = seen.insert(key, path.clone()) {
            return Err(format!(
                "candidate paths collide case-insensitively: {previous} and {path}"
            ));
        }
        validated.push(EpsCandidate {
            path,
            code: candidate.code,
        });
    }
    Ok(validated)
}

fn validate_analyzer_success(output: &AnalyzerSuccess) -> Result<(), String> {
    if output.diagnostics.len() > MAX_DIAGNOSTICS {
        return Err("diagnostic cap was exceeded".to_string());
    }
    let mut message_bytes = 0usize;
    for path in &output.checked_files {
        normalize_project_path(path)?;
    }
    for diagnostic in &output.diagnostics {
        normalize_project_path(&diagnostic.path)?;
        if diagnostic.line == 0
            || diagnostic.character == 0
            || diagnostic.end_line == 0
            || diagnostic.end_character == 0
        {
            return Err("diagnostic positions must be 1-based".to_string());
        }
        if !matches!(
            diagnostic.severity.as_str(),
            "error" | "warning" | "information" | "hint"
        ) {
            return Err("diagnostic severity is invalid".to_string());
        }
        message_bytes = message_bytes.saturating_add(diagnostic.message.len());
    }
    if message_bytes > MAX_MESSAGE_BYTES {
        return Err("diagnostic message-byte cap was exceeded".to_string());
    }
    for import in &output.imports {
        normalize_project_path(&import.from)?;
        normalize_project_path(&import.to)?;
        if !matches!(
            import.status.as_str(),
            "resolved" | "missing" | "unreadable"
        ) {
            return Err("import status is invalid".to_string());
        }
    }
    Ok(())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

trait AdapterSession: Send {
    fn exchange(
        &mut self,
        id: u64,
        request: &AnalyzerRequest,
        deadline: Duration,
    ) -> Result<AnalyzerSuccess, AnalyzerError>;
    fn terminate_and_reap(&mut self);
}

trait AdapterSpawner: Send + Sync {
    fn spawn(&self) -> Result<Box<dyn AdapterSession>, AnalyzerError>;
}

struct NodeProcessSpawner {
    adapter: PathBuf,
    cwd: PathBuf,
    logs_dir: PathBuf,
}

impl AdapterSpawner for NodeProcessSpawner {
    fn spawn(&self) -> Result<Box<dyn AdapterSession>, AnalyzerError> {
        let node = which::which("node.exe")
            .map_err(|error| AnalyzerError::new(SkipReason::NodeNotFound, error.to_string()))?;
        ProcessSession::spawn(&node, &self.adapter, &self.cwd, &self.logs_dir)
            .map(|session| Box::new(session) as Box<dyn AdapterSession>)
            .map_err(|error| AnalyzerError::new(SkipReason::AdapterStartFailed, error.to_string()))
    }
}

struct AnalyzerProcessState {
    process: Option<Box<dyn AdapterSession>>,
    project_id: Option<String>,
    next_id: u64,
    cold: bool,
}

impl Default for AnalyzerProcessState {
    fn default() -> Self {
        Self {
            process: None,
            project_id: None,
            next_id: 1,
            cold: true,
        }
    }
}

/// Pinned adapter client. Construction verifies resource bytes, while Node is
/// resolved and started lazily on the first check.
pub struct NodeEpsAnalyzer {
    unavailable: Option<AnalyzerError>,
    spawner: Arc<dyn AdapterSpawner>,
    state: Mutex<AnalyzerProcessState>,
}

impl NodeEpsAnalyzer {
    pub fn from_resource(
        adapter: PathBuf,
        checksum: PathBuf,
        logs_dir: PathBuf,
        cwd: PathBuf,
    ) -> Self {
        let unavailable = verify_adapter_resource(&adapter, &checksum).err();
        Self {
            unavailable,
            spawner: Arc::new(NodeProcessSpawner {
                adapter,
                cwd,
                logs_dir,
            }),
            state: Mutex::new(AnalyzerProcessState::default()),
        }
    }
    pub fn unavailable(reason: SkipReason, message: impl Into<String>) -> Self {
        Self {
            unavailable: Some(AnalyzerError::new(reason, message)),
            spawner: Arc::new(NodeProcessSpawner {
                adapter: PathBuf::new(),
                cwd: PathBuf::new(),
                logs_dir: PathBuf::new(),
            }),
            state: Mutex::new(AnalyzerProcessState::default()),
        }
    }

    #[cfg(test)]
    fn with_spawner(spawner: Arc<dyn AdapterSpawner>) -> Self {
        Self {
            unavailable: None,
            spawner,
            state: Mutex::new(AnalyzerProcessState::default()),
        }
    }
}

impl EpsAnalyzer for NodeEpsAnalyzer {
    fn analyze(&self, request: &AnalyzerRequest) -> Result<AnalyzerSuccess, AnalyzerError> {
        if let Some(error) = &self.unavailable {
            return Err(error.clone());
        }
        let mut state = self.state.lock();
        if state.project_id.as_deref() != Some(request.project_id.as_str()) {
            terminate_process(&mut state);
            state.project_id = Some(request.project_id.clone());
        }

        let mut last_error = None;
        for attempt in 0..=1 {
            if state.process.is_none() {
                match self.spawner.spawn() {
                    Ok(process) => {
                        state.process = Some(process);
                        state.cold = true;
                    }
                    Err(error) => return Err(error),
                }
            }
            let deadline = if state.cold {
                COLD_REQUEST_DEADLINE
            } else {
                WARM_REQUEST_DEADLINE
            };
            let id = state.next_id;
            state.next_id = state.next_id.saturating_add(1);
            let result = state
                .process
                .as_mut()
                .expect("process was initialized")
                .exchange(id, request, deadline);
            let result = result.and_then(|output| {
                validate_analyzer_success(&output)
                    .map(|()| output)
                    .map_err(|message| {
                        AnalyzerError::new(
                            SkipReason::AdapterProtocolError,
                            format!("adapter returned malformed output: {message}"),
                        )
                    })
            });
            match result {
                Ok(result) => {
                    state.cold = false;
                    return Ok(result);
                }
                Err(error) => {
                    last_error = Some(error.clone());
                    terminate_process(&mut state);
                    if attempt == 1 || !retryable_adapter_failure(error.reason) {
                        return Err(error);
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            AnalyzerError::new(SkipReason::AdapterCrashed, "adapter request failed")
        }))
    }

    fn reset_project(&self) {
        let mut state = self.state.lock();
        terminate_process(&mut state);
        state.project_id = None;
    }
}

impl Drop for NodeEpsAnalyzer {
    fn drop(&mut self) {
        let mut state = self.state.lock();
        terminate_process(&mut state);
    }
}

fn retryable_adapter_failure(reason: SkipReason) -> bool {
    matches!(
        reason,
        SkipReason::AdapterStartFailed
            | SkipReason::AdapterCrashed
            | SkipReason::AdapterTimeout
            | SkipReason::AdapterProtocolError
    )
}

fn terminate_process(state: &mut AnalyzerProcessState) {
    if let Some(mut process) = state.process.take() {
        process.terminate_and_reap();
    }
    state.cold = true;
}

fn verify_adapter_resource(adapter: &Path, checksum: &Path) -> Result<(), AnalyzerError> {
    let bytes = fs::read(adapter).map_err(|error| {
        AnalyzerError::new(
            SkipReason::AdapterMissing,
            format!("adapter resource is missing: {error}"),
        )
    })?;
    let expected = fs::read_to_string(checksum).map_err(|error| {
        AnalyzerError::new(
            SkipReason::AdapterMissing,
            format!("adapter checksum is missing: {error}"),
        )
    })?;
    let expected = expected
        .split_whitespace()
        .next()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| {
            AnalyzerError::new(
                SkipReason::AdapterStartFailed,
                "adapter checksum file is malformed",
            )
        })?;
    let actual = hex_sha256(&bytes);
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(AnalyzerError::new(
            SkipReason::AdapterStartFailed,
            "adapter checksum verification failed",
        ))
    }
}

#[derive(Serialize)]
struct ProtocolRequest<'a> {
    id: u64,
    method: &'static str,
    params: &'a AnalyzerRequest,
}

#[derive(Deserialize)]
struct ProtocolResponse {
    id: u64,
    result: Option<AnalyzerSuccess>,
    error: Option<ProtocolError>,
}

#[derive(Deserialize)]
struct ProtocolError {
    message: String,
}

enum ReaderEvent {
    Frame(Vec<u8>),
    Protocol(String),
    Eof,
}

struct WriterJob {
    bytes: Vec<u8>,
    reply: Sender<Result<(), String>>,
}

struct ProcessSession {
    child: Child,
    writer: Option<Sender<WriterJob>>,
    reader: Receiver<ReaderEvent>,
    writer_thread: Option<JoinHandle<()>>,
    reader_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    reaped: bool,
    #[cfg(windows)]
    job: Option<WindowsJob>,
}

impl ProcessSession {
    fn spawn(node: &Path, adapter: &Path, cwd: &Path, logs_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(cwd)?;
        fs::create_dir_all(logs_dir)?;
        let mut command = Command::new(node);
        command
            .arg(adapter)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn()?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("adapter child pipes are unavailable"));
        };
        #[cfg(windows)]
        let job = match WindowsJob::assign(&child) {
            Ok(job) => Some(job),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };

        let (writer_tx, writer_rx) = mpsc::channel::<WriterJob>();
        let writer_thread = thread::spawn(move || writer_loop(stdin, writer_rx));
        let (reader_tx, reader_rx) = mpsc::channel::<ReaderEvent>();
        let reader_thread = thread::spawn(move || reader_loop(stdout, reader_tx));
        let log_path = logs_dir.join("epscript-lsp-agent.log");
        let stderr_thread = thread::spawn(move || stderr_loop(stderr, &log_path));

        Ok(Self {
            child,
            writer: Some(writer_tx),
            reader: reader_rx,
            writer_thread: Some(writer_thread),
            reader_thread: Some(reader_thread),
            stderr_thread: Some(stderr_thread),
            reaped: false,
            #[cfg(windows)]
            job,
        })
    }
}

impl AdapterSession for ProcessSession {
    fn exchange(
        &mut self,
        id: u64,
        request: &AnalyzerRequest,
        deadline: Duration,
    ) -> Result<AnalyzerSuccess, AnalyzerError> {
        let started = Instant::now();
        let payload = serde_json::to_vec(&ProtocolRequest {
            id,
            method: "analyze",
            params: request,
        })
        .map_err(|error| AnalyzerError::new(SkipReason::AdapterProtocolError, error.to_string()))?;
        let framed = encode_frame(&payload).map_err(|error| {
            AnalyzerError::new(SkipReason::AdapterProtocolError, error.to_string())
        })?;
        let (reply_tx, reply_rx) = mpsc::channel();
        self.writer
            .as_ref()
            .ok_or_else(|| {
                AnalyzerError::new(SkipReason::AdapterCrashed, "adapter stdin is closed")
            })?
            .send(WriterJob {
                bytes: framed,
                reply: reply_tx,
            })
            .map_err(|_| AnalyzerError::new(SkipReason::AdapterCrashed, "adapter writer exited"))?;
        let remaining = deadline.saturating_sub(started.elapsed());
        match reply_rx.recv_timeout(remaining) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(AnalyzerError::new(SkipReason::AdapterCrashed, error)),
            Err(RecvTimeoutError::Timeout) => {
                return Err(AnalyzerError::new(
                    SkipReason::AdapterTimeout,
                    "adapter request write timed out",
                ))
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(AnalyzerError::new(
                    SkipReason::AdapterCrashed,
                    "adapter writer disconnected",
                ))
            }
        }

        let remaining = deadline.saturating_sub(started.elapsed());
        let event = self
            .reader
            .recv_timeout(remaining)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => {
                    AnalyzerError::new(SkipReason::AdapterTimeout, "adapter response timed out")
                }
                RecvTimeoutError::Disconnected => {
                    AnalyzerError::new(SkipReason::AdapterCrashed, "adapter stdout disconnected")
                }
            })?;
        let payload = match event {
            ReaderEvent::Frame(payload) => payload,
            ReaderEvent::Protocol(message) => {
                return Err(AnalyzerError::new(
                    SkipReason::AdapterProtocolError,
                    message,
                ))
            }
            ReaderEvent::Eof => {
                return Err(AnalyzerError::new(
                    SkipReason::AdapterCrashed,
                    "adapter exited before returning a response",
                ))
            }
        };
        let response: ProtocolResponse = serde_json::from_slice(&payload).map_err(|error| {
            AnalyzerError::new(SkipReason::AdapterProtocolError, error.to_string())
        })?;
        if response.id != id {
            return Err(AnalyzerError::new(
                SkipReason::AdapterProtocolError,
                "adapter response id does not match the request",
            ));
        }
        if let Some(error) = response.error {
            return Err(AnalyzerError::new(
                SkipReason::AdapterProtocolError,
                error.message,
            ));
        }
        response.result.ok_or_else(|| {
            AnalyzerError::new(
                SkipReason::AdapterProtocolError,
                "adapter response contains neither result nor error",
            )
        })
    }

    fn terminate_and_reap(&mut self) {
        if self.reaped {
            return;
        }
        self.reaped = true;
        self.writer.take();
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            job.terminate();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(thread) = self.writer_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.reader_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ProcessSession {
    fn drop(&mut self) {
        self.terminate_and_reap();
    }
}

fn writer_loop(mut stdin: std::process::ChildStdin, receiver: Receiver<WriterJob>) {
    while let Ok(job) = receiver.recv() {
        let result = stdin
            .write_all(&job.bytes)
            .and_then(|()| stdin.flush())
            .map_err(|error| error.to_string());
        let failed = result.is_err();
        let _ = job.reply.send(result);
        if failed {
            break;
        }
    }
}

fn reader_loop(mut stdout: std::process::ChildStdout, sender: Sender<ReaderEvent>) {
    let mut decoder = FrameDecoder::default();
    let mut chunk = [0u8; 4096];
    loop {
        match stdout.read(&mut chunk) {
            Ok(0) => {
                let _ = sender.send(ReaderEvent::Eof);
                return;
            }
            Ok(read) => match decoder.feed(&chunk[..read]) {
                Ok(frames) => {
                    for frame in frames {
                        if sender.send(ReaderEvent::Frame(frame)).is_err() {
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = sender.send(ReaderEvent::Protocol(error));
                    return;
                }
            },
            Err(error) => {
                let _ = sender.send(ReaderEvent::Protocol(error.to_string()));
                return;
            }
        }
    }
}

fn stderr_loop(mut stderr: std::process::ChildStderr, log_path: &Path) {
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .ok();
    let mut remaining = MAX_STDERR_BYTES;
    let mut chunk = [0u8; 4096];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                let kept = read.min(remaining);
                if kept > 0 {
                    if let Some(file) = log.as_mut() {
                        let _ = file.write_all(&chunk[..kept]);
                    }
                    remaining -= kept;
                }
            }
        }
    }
}

#[derive(Default)]
struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        loop {
            let Some(separator) = find_bytes(&self.buffer, b"\r\n\r\n") else {
                if self.buffer.len() > MAX_HEADER_BYTES {
                    return Err("adapter protocol header exceeds its size limit".to_string());
                }
                break;
            };
            let header = std::str::from_utf8(&self.buffer[..separator])
                .map_err(|_| "adapter protocol header is not ASCII".to_string())?;
            let mut content_length = None;
            for line in header.split("\r\n") {
                let Some((name, value)) = line.split_once(':') else {
                    return Err("adapter protocol header is malformed".to_string());
                };
                if name.eq_ignore_ascii_case("Content-Length") {
                    if content_length.is_some() {
                        return Err("adapter protocol has duplicate Content-Length".to_string());
                    }
                    let parsed = value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| "adapter Content-Length is invalid".to_string())?;
                    if parsed == 0 || parsed > MAX_FRAME_BYTES {
                        return Err("adapter Content-Length is out of bounds".to_string());
                    }
                    content_length = Some(parsed);
                } else {
                    return Err("adapter protocol contains an unsupported header".to_string());
                }
            }
            let content_length = content_length
                .ok_or_else(|| "adapter protocol is missing Content-Length".to_string())?;
            let frame_start = separator + 4;
            let frame_end = frame_start + content_length;
            if self.buffer.len() < frame_end {
                break;
            }
            frames.push(self.buffer[frame_start..frame_end].to_vec());
            self.buffer.drain(..frame_end);
        }
        Ok(frames)
    }
}

fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, String> {
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err("adapter payload length is out of bounds".to_string());
    }
    let header = format!("Content-Length: {}\r\n\r\n", payload.len());
    let mut framed = Vec::with_capacity(header.len() + payload.len());
    framed.extend_from_slice(header.as_bytes());
    framed.extend_from_slice(payload);
    Ok(framed)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(windows)]
struct WindowsJob(isize);

#[cfg(windows)]
impl WindowsJob {
    fn assign(child: &Child) -> io::Result<Self> {
        use std::mem::size_of;
        use std::os::windows::io::AsRawHandle;
        use std::ptr;
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        let assigned = if configured != 0 {
            unsafe { AssignProcessToJobObject(handle, child.as_raw_handle() as HANDLE) }
        } else {
            0
        };
        if configured == 0 || assigned == 0 {
            let error = io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        Ok(Self(handle as isize))
    }

    fn terminate(self) {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        let handle = self.0 as HANDLE;
        unsafe {
            TerminateJobObject(handle, 1);
            CloseHandle(handle);
        }
        std::mem::forget(self);
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        unsafe { CloseHandle(self.0 as HANDLE) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_io::EpsSnapshotFile;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_dirs(tag: &str) -> DataDirs {
        let root =
            std::env::temp_dir().join(format!("eud-eps-preflight-{tag}-{}", uuid::Uuid::new_v4()));
        DataDirs::from_bases(&root.join("roaming"), &root.join("local"))
    }

    fn success(paths: &[&str]) -> AnalyzerSuccess {
        AnalyzerSuccess {
            checked_files: paths.iter().map(|path| (*path).to_string()).collect(),
            diagnostics: Vec::new(),
            imports: Vec::new(),
            truncated: false,
            omitted_diagnostics: 0,
            omitted_message_bytes: 0,
        }
    }

    #[test]
    fn content_length_decoder_accepts_fragmented_and_multiple_frames() {
        let first = encode_frame(br#"{"id":1}"#).unwrap();
        let second = encode_frame(br#"{"id":2}"#).unwrap();
        let combined = [first, second].concat();
        let mut decoder = FrameDecoder::default();
        let mut frames = Vec::new();
        for fragment in combined.chunks(3) {
            frames.extend(decoder.feed(fragment).unwrap());
        }
        assert_eq!(
            frames,
            vec![br#"{"id":1}"#.to_vec(), br#"{"id":2}"#.to_vec()]
        );
    }

    #[test]
    fn content_length_decoder_rejects_invalid_duplicate_and_oversized_lengths() {
        for invalid in [
            b"Content-Length: nope\r\n\r\n{}".as_slice(),
            b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}".as_slice(),
            b"Content-Length: 0\r\n\r\n".as_slice(),
            b"Content-Length: 999999999\r\n\r\n".as_slice(),
        ] {
            assert!(FrameDecoder::default().feed(invalid).is_err());
        }
    }

    #[test]
    fn project_paths_reject_traversal_prefixes_empty_segments_and_case_collisions() {
        for invalid in [
            "../main.eps",
            "folder/../main.eps",
            "./main.eps",
            "/main.eps",
            "C:/main.eps",
            "folder//main.eps",
            "folder\\main.eps",
            "main.txt",
            "\0main.eps",
        ] {
            assert!(
                normalize_project_path(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        assert_eq!(
            normalize_project_path("한글/main.eps").unwrap(),
            "한글/main.eps"
        );
        assert!(validate_candidates(vec![
            EpsCandidate {
                path: "Lib/A.eps".into(),
                code: String::new()
            },
            EpsCandidate {
                path: "lib/a.eps".into(),
                code: String::new()
            },
        ])
        .is_err());
    }

    #[test]
    fn adapter_resource_requires_present_checksum_matching_bytes() {
        let root = std::env::temp_dir().join(format!(
            "eud-eps-preflight-resource-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let adapter = root.join("adapter.cjs");
        let checksum = root.join("adapter.sha256");
        fs::write(&adapter, b"bundle").unwrap();
        fs::write(
            &checksum,
            format!("{}  adapter.cjs\n", hex_sha256(b"bundle")),
        )
        .unwrap();
        verify_adapter_resource(&adapter, &checksum).unwrap();

        fs::write(&checksum, format!("{}  adapter.cjs\n", "0".repeat(64))).unwrap();
        assert_eq!(
            verify_adapter_resource(&adapter, &checksum)
                .unwrap_err()
                .reason,
            SkipReason::AdapterStartFailed
        );
        fs::remove_file(&adapter).unwrap();
        assert_eq!(
            verify_adapter_resource(&adapter, &checksum)
                .unwrap_err()
                .reason,
            SkipReason::AdapterMissing
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn candidate_overlay_is_atomic_and_leaves_base_mirror_unchanged() {
        let dirs = temp_dirs("overlay");
        dirs.ensure_dirs().unwrap();
        let snapshot = EpsSnapshot {
            project: "OverlayProject".into(),
            identity: "OverlayProject\nmap.scx".into(),
            files: vec![EpsSnapshotFile {
                path: "main.eps".into(),
                ftype: "CUIEps".into(),
                content: Some("const old = 1;".into()),
            }],
        };
        let mirror = refresh_mirror(&dirs, &snapshot).unwrap();
        let analysis = prepare_analysis_root(
            &mirror.mirror_root,
            &[
                EpsCandidate {
                    path: "main.eps".into(),
                    code: "const new = 2;".into(),
                },
                EpsCandidate {
                    path: "lib/new.eps".into(),
                    code: "const value = 3;".into(),
                },
            ],
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(mirror.mirror_root.join("main.eps")).unwrap(),
            "const old = 1;"
        );
        assert_eq!(
            fs::read_to_string(analysis.join("main.eps")).unwrap(),
            "const new = 2;"
        );
        assert_eq!(
            fs::read_to_string(analysis.join("lib/new.eps")).unwrap(),
            "const value = 3;"
        );
        fs::remove_dir_all(dirs.app_data().parent().unwrap()).ok();
    }

    #[test]
    fn failed_candidate_batch_discards_every_staged_overlay() {
        let root = std::env::temp_dir().join(format!(
            "eud-eps-preflight-overlay-failure-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("main.eps"), b"old").unwrap();
        fs::write(root.join("blocked"), b"not a directory").unwrap();
        let result = overlay_candidates(
            &root,
            &[
                EpsCandidate {
                    path: "main.eps".into(),
                    code: "new".into(),
                },
                EpsCandidate {
                    path: "blocked/new.eps".into(),
                    code: "never written".into(),
                },
            ],
        );
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(root.join("main.eps")).unwrap(), "old");
        assert!(!root.join("main.eps.eud-agent-0.tmp").exists());
        fs::remove_dir_all(root).ok();
    }

    struct FakeSnapshotProvider {
        snapshots: Mutex<VecDeque<Result<EpsSnapshot, String>>>,
        calls: AtomicUsize,
    }

    impl FakeSnapshotProvider {
        fn new(snapshots: impl IntoIterator<Item = Result<EpsSnapshot, String>>) -> Self {
            Self {
                snapshots: Mutex::new(snapshots.into_iter().collect()),
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl SnapshotProvider for FakeSnapshotProvider {
        fn snapshot(&self) -> Result<EpsSnapshot, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.snapshots.lock().pop_front().unwrap()
        }
    }

    struct FakeAnalyzer {
        results: Mutex<VecDeque<Result<AnalyzerSuccess, AnalyzerError>>>,
        requests: Mutex<Vec<AnalyzerRequest>>,
        calls: AtomicUsize,
        resets: AtomicUsize,
    }

    impl FakeAnalyzer {
        fn new(results: impl IntoIterator<Item = Result<AnalyzerSuccess, AnalyzerError>>) -> Self {
            Self {
                results: Mutex::new(results.into_iter().collect()),
                requests: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                resets: AtomicUsize::new(0),
            }
        }
    }

    impl EpsAnalyzer for FakeAnalyzer {
        fn analyze(&self, request: &AnalyzerRequest) -> Result<AnalyzerSuccess, AnalyzerError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().push(request.clone());
            self.results.lock().pop_front().unwrap()
        }

        fn reset_project(&self) {
            self.resets.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn snapshot(project: &str, path: &str, code: &str) -> EpsSnapshot {
        EpsSnapshot {
            project: project.to_string(),
            identity: format!("{project}\nmap.scx"),
            files: vec![EpsSnapshotFile {
                path: path.to_string(),
                ftype: "CUIEps".to_string(),
                content: Some(code.to_string()),
            }],
        }
    }

    #[test]
    fn edit_candidates_resolve_against_the_mirror_before_analysis() {
        let dirs = temp_dirs("edit-candidate");
        dirs.ensure_dirs().unwrap();
        let snapshots = Arc::new(FakeSnapshotProvider::new([Ok(snapshot(
            "Project",
            "main.eps",
            "function main() {\n    oldCall();\n}\n",
        ))]));
        let analyzer = Arc::new(FakeAnalyzer::new([Ok(success(&["main.eps"]))]));
        let preflight =
            EpsPreflight::with_snapshot_provider(dirs, analyzer.clone(), snapshots.clone());

        preflight
            .check_inputs(
                "request",
                vec![EpsCandidateInput {
                    path: "main.eps".into(),
                    code: None,
                    edits: Some(vec![ExactTextEdit {
                        old_text: "oldCall();".into(),
                        new_text: "newCall();".into(),
                    }]),
                }],
            )
            .unwrap();

        let requests = analyzer.requests.lock();
        assert_eq!(
            requests[0].candidates[0].code,
            "function main() {\n    newCall();\n}\n"
        );
        let mirror_root = preflight.state.lock().mirror_root.clone().unwrap();
        assert_eq!(
            fs::read_to_string(mirror_root.join("main.eps")).unwrap(),
            "function main() {\n    oldCall();\n}\n",
            "candidate edits must not mutate the reusable mirror"
        );
        assert_eq!(snapshots.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn same_request_reuses_snapshot_and_project_switch_resets_analyzer() {
        let dirs = temp_dirs("reuse");
        dirs.ensure_dirs().unwrap();
        let snapshots = Arc::new(FakeSnapshotProvider::new([
            Ok(snapshot("ProjectA", "main.eps", "const a = 1;")),
            Ok(snapshot("ProjectB", "main.eps", "const b = 1;")),
        ]));
        let analyzer = Arc::new(FakeAnalyzer::new([
            Ok(success(&["main.eps"])),
            Ok(success(&["main.eps"])),
            Ok(success(&["main.eps"])),
        ]));
        let preflight =
            EpsPreflight::with_snapshot_provider(dirs, analyzer.clone(), snapshots.clone());
        preflight.begin_request("one");
        preflight
            .check(
                "one",
                vec![EpsCandidate {
                    path: "main.eps".into(),
                    code: "const a = 2;".into(),
                }],
            )
            .unwrap();
        preflight
            .check(
                "one",
                vec![EpsCandidate {
                    path: "main.eps".into(),
                    code: "const a = 3;".into(),
                }],
            )
            .unwrap();
        assert_eq!(snapshots.calls.load(Ordering::SeqCst), 1);
        preflight.begin_request("two");
        preflight
            .check(
                "two",
                vec![EpsCandidate {
                    path: "main.eps".into(),
                    code: "const b = 2;".into(),
                }],
            )
            .unwrap();
        assert_eq!(snapshots.calls.load(Ordering::SeqCst), 2);
        assert_eq!(analyzer.resets.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn snapshot_and_each_adapter_failure_return_stable_skipped_results() {
        let reasons = [
            SkipReason::NodeNotFound,
            SkipReason::AdapterMissing,
            SkipReason::AdapterStartFailed,
            SkipReason::AdapterCrashed,
            SkipReason::AdapterTimeout,
            SkipReason::AdapterProtocolError,
        ];
        for reason in reasons {
            let dirs = temp_dirs("skip");
            dirs.ensure_dirs().unwrap();
            let snapshots = Arc::new(FakeSnapshotProvider::new([Ok(snapshot(
                "Project", "main.eps", "",
            ))]));
            let analyzer = Arc::new(FakeAnalyzer::new([Err(AnalyzerError::new(reason, "fail"))]));
            let preflight = EpsPreflight::with_snapshot_provider(dirs, analyzer, snapshots);
            preflight.begin_request("request");
            assert_eq!(
                preflight
                    .check(
                        "request",
                        vec![EpsCandidate {
                            path: "main.eps".into(),
                            code: String::new()
                        }]
                    )
                    .unwrap(),
                EpsCheckResult::skipped(reason)
            );
        }

        let dirs = temp_dirs("snapshot-skip");
        dirs.ensure_dirs().unwrap();
        let snapshots = Arc::new(FakeSnapshotProvider::new([Err("offline".into())]));
        let analyzer = Arc::new(FakeAnalyzer::new([]));
        let preflight = EpsPreflight::with_snapshot_provider(dirs, analyzer, snapshots);
        preflight.begin_request("request");
        assert_eq!(
            preflight
                .check(
                    "request",
                    vec![EpsCandidate {
                        path: "main.eps".into(),
                        code: String::new()
                    }]
                )
                .unwrap(),
            EpsCheckResult::skipped(SkipReason::SnapshotUnavailable)
        );
    }

    #[derive(Clone)]
    struct FakeSpawner {
        outcomes: Arc<Mutex<VecDeque<Result<AnalyzerSuccess, AnalyzerError>>>>,
        spawns: Arc<AtomicUsize>,
        reaps: Arc<AtomicUsize>,
    }

    impl AdapterSpawner for FakeSpawner {
        fn spawn(&self) -> Result<Box<dyn AdapterSession>, AnalyzerError> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeSession {
                outcomes: self.outcomes.clone(),
                reaps: self.reaps.clone(),
                reaped: false,
            }))
        }
    }

    struct FakeSession {
        outcomes: Arc<Mutex<VecDeque<Result<AnalyzerSuccess, AnalyzerError>>>>,
        reaps: Arc<AtomicUsize>,
        reaped: bool,
    }

    impl AdapterSession for FakeSession {
        fn exchange(
            &mut self,
            _id: u64,
            _request: &AnalyzerRequest,
            _deadline: Duration,
        ) -> Result<AnalyzerSuccess, AnalyzerError> {
            self.outcomes.lock().pop_front().unwrap()
        }

        fn terminate_and_reap(&mut self) {
            if !self.reaped {
                self.reaped = true;
                self.reaps.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    #[test]
    fn crash_timeout_and_protocol_failures_restart_once_and_reap_every_child() {
        for reason in [
            SkipReason::AdapterCrashed,
            SkipReason::AdapterTimeout,
            SkipReason::AdapterProtocolError,
        ] {
            let spawner = FakeSpawner {
                outcomes: Arc::new(Mutex::new(VecDeque::from([
                    Err(AnalyzerError::new(reason, "first")),
                    Err(AnalyzerError::new(reason, "second")),
                ]))),
                spawns: Arc::new(AtomicUsize::new(0)),
                reaps: Arc::new(AtomicUsize::new(0)),
            };
            let analyzer = NodeEpsAnalyzer::with_spawner(Arc::new(spawner.clone()));
            let result = analyzer.analyze(&AnalyzerRequest {
                project_id: "project".into(),
                root: "C:/mirror".into(),
                candidates: vec![EpsCandidate {
                    path: "main.eps".into(),
                    code: String::new(),
                }],
                unreadable: Vec::new(),
            });
            assert_eq!(result.unwrap_err().reason, reason);
            assert_eq!(spawner.spawns.load(Ordering::SeqCst), 2);
            assert_eq!(spawner.reaps.load(Ordering::SeqCst), 2);
        }
    }

    #[test]
    fn malformed_output_retries_once_and_same_project_reuses_the_process() {
        let malformed = AnalyzerSuccess {
            checked_files: vec!["../escape.eps".into()],
            diagnostics: Vec::new(),
            imports: Vec::new(),
            truncated: false,
            omitted_diagnostics: 0,
            omitted_message_bytes: 0,
        };
        let spawner = FakeSpawner {
            outcomes: Arc::new(Mutex::new(VecDeque::from([
                Ok(malformed),
                Ok(success(&["main.eps"])),
                Ok(success(&["main.eps"])),
                Ok(success(&["main.eps"])),
            ]))),
            spawns: Arc::new(AtomicUsize::new(0)),
            reaps: Arc::new(AtomicUsize::new(0)),
        };
        let analyzer = NodeEpsAnalyzer::with_spawner(Arc::new(spawner.clone()));
        let mut request = AnalyzerRequest {
            project_id: "project-a".into(),
            root: "C:/mirror".into(),
            candidates: vec![EpsCandidate {
                path: "main.eps".into(),
                code: String::new(),
            }],
            unreadable: Vec::new(),
        };

        analyzer.analyze(&request).unwrap();
        assert_eq!(spawner.spawns.load(Ordering::SeqCst), 2);
        analyzer.analyze(&request).unwrap();
        assert_eq!(
            spawner.spawns.load(Ordering::SeqCst),
            2,
            "warm checks for one project must reuse the live process"
        );
        request.project_id = "project-b".into();
        analyzer.analyze(&request).unwrap();
        assert_eq!(spawner.spawns.load(Ordering::SeqCst), 3);
        assert_eq!(spawner.reaps.load(Ordering::SeqCst), 2);
        drop(analyzer);
        assert_eq!(spawner.reaps.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn successful_mutations_update_or_invalidate_request_mirror() {
        let dirs = temp_dirs("mutations");
        dirs.ensure_dirs().unwrap();
        let snapshots = Arc::new(FakeSnapshotProvider::new([
            Ok(snapshot("Project", "main.eps", "old")),
            Ok(snapshot("Project", "main.eps", "refreshed")),
        ]));
        let analyzer = Arc::new(FakeAnalyzer::new([
            Ok(success(&["main.eps"])),
            Ok(success(&["main.eps"])),
        ]));
        let preflight = EpsPreflight::with_snapshot_provider(dirs, analyzer, snapshots.clone());
        preflight.begin_request("request");
        preflight
            .check(
                "request",
                vec![EpsCandidate {
                    path: "main.eps".into(),
                    code: "candidate".into(),
                }],
            )
            .unwrap();
        preflight.write_applied("request", "main.eps", "written");
        preflight.rename_applied("request", "main.eps", "renamed.eps");
        preflight.delete_applied("request", "renamed.eps");
        preflight.invalidate("request");
        preflight
            .check(
                "request",
                vec![EpsCandidate {
                    path: "main.eps".into(),
                    code: "again".into(),
                }],
            )
            .unwrap();
        assert_eq!(snapshots.calls.load(Ordering::SeqCst), 2);
    }
}
