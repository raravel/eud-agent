//! Configured native-project runtime shared by Tauri IPC, MCP tools, map services, and builds.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::config::DataDirs;
use crate::journal::{JournalEntry, JournalStore, JournalTarget, Snapshot, WriteTool};
use crate::native_build::{
    probe_frozen_python, run_native_build_with_python_and_cancellation, DatCatalog, EuddraftLaunch,
    FrozenPythonIdentity, NativeBuildArtifacts, NativeBuildError, NativeBuildResult,
};
use crate::native_project::{
    compute_python_lock_digest, discover_manifest_files, validate_python_dependencies,
    validate_python_lock, DatScalar, DatTarget, NativeDatPatch, NativeProject, NativeProjectStatus,
    NativeSourceSnapshot, ProjectManifest, PythonLock, PythonLockTarget, PythonLockedPackage,
    MANAGED_UV_VERSION, PROJECT_MANIFEST_FILE, PYTHON_LOCK_ARCH, PYTHON_LOCK_OS,
};

pub const PROJECT_NOT_CONFIGURED: &str = "native project path not configured";
pub const EUDDRAFT_NOT_CONFIGURED: &str = "euddraft path not configured";
pub const LEGACY_PROJECT_DESCRIPTOR_FILE: &str = "project.eudproj";

const PYTHON_CANDIDATE_TTL_MILLIS: u64 = 10 * 60 * 1_000;
const PYTHON_ENV_MARKER: &str = "complete.json";
const PYTHON_SITE_PACKAGES: &str = "site-packages";
const PYTHON_RESOLVE_TIMEOUT: Duration = Duration::from_secs(300);
const PYTHON_INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_PYLOCK_BYTES: u64 = 4 * 1024 * 1024;
const MAX_WHEEL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_WHEEL_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ENVIRONMENT_FILES: usize = 200_000;
const MAX_RESOLVED_PACKAGES: usize = 512;
const MAX_PREPARE_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_ENVIRONMENT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
static PYTHON_PREPARATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonDependenciesPrepareResult {
    pub candidate_token: String,
    pub normalized_dependencies: Vec<String>,
    pub resolved_packages: Vec<PythonResolvedPackageSummary>,
    pub lock_digest: String,
    pub input_digest: String,
    pub cache_identity: String,
    pub base_revision: String,
    pub manifest_sha256: String,
    pub euddraft_fingerprint: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonResolvedPackageSummary {
    pub name: String,
    pub version: String,
    pub wheel_filename: String,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct PythonDependenciesCommit {
    pub revision: String,
    pub before_manifest: Vec<u8>,
    pub before_manifest_sha256: String,
    pub after_manifest: Vec<u8>,
    pub after_manifest_sha256: String,
    pub normalized_dependencies: Vec<String>,
    pub lock_digest: String,
}

#[derive(Debug, Clone)]
pub struct ClaimedPythonDependencies {
    session_id: String,
    project_id: String,
    project_root: PathBuf,
    cache_key: String,
    base_revision: String,
    manifest_sha256: String,
    input_digest: String,
    cache_identity: String,
    euddraft_fingerprint: String,
    python_abi: String,
    expires_at: u64,
    dependencies: Vec<String>,
    python_lock: Option<PythonLock>,
    lock_digest: String,
}

#[derive(Debug, Clone)]
struct PythonDependencyCandidate {
    session_id: String,
    project_id: String,
    project_root: PathBuf,
    cache_key: String,
    base_revision: String,
    manifest_sha256: String,
    input_digest: String,
    cache_identity: String,
    euddraft_fingerprint: String,
    python_abi: String,
    expires_at: u64,
    dependencies: Vec<String>,
    python_lock: Option<PythonLock>,
    lock_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PythonEnvironmentMarker {
    schema: String,
    project_id: String,
    dependencies: Vec<String>,
    lock: PythonLock,
    probe: FrozenPythonIdentity,
    files: Vec<PythonEnvironmentFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PythonEnvironmentFile {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PylockPackage {
    name: String,
    version: String,
    wheels: Vec<PylockWheel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PylockWheel {
    filename: String,
    url: String,
    sha256: String,
}

#[derive(Deserialize)]
struct UvPylock {
    #[serde(rename = "lock-version")]
    lock_version: String,
    #[serde(default)]
    packages: Vec<UvPylockPackage>,
}

#[derive(Deserialize)]
struct UvPylockPackage {
    name: String,
    version: String,
    #[serde(default)]
    wheels: Vec<UvPylockWheel>,
}

#[derive(Deserialize)]
struct UvPylockWheel {
    url: String,
    hashes: UvPylockHashes,
}

#[derive(Deserialize)]
struct UvPylockHashes {
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyProjectDescriptor {
    schema_version: u32,
    manifest: String,
}

struct LegacyMigrationBackup {
    root: PathBuf,
    authority_bytes: Vec<u8>,
    files: Vec<(PathBuf, Vec<u8>)>,
    canonical_existed: bool,
}

/// Resolve a launcher input to its canonical native project root.
///
/// Inputs may be a project directory, a canonical `.eap` file, or an explicit legacy
/// `project.json`/`.eudproj` import source. Legacy inputs are migrated by `open_project_path`.
pub fn resolve_project_root(input: &Path) -> Result<PathBuf, String> {
    let link_metadata = fs::symlink_metadata(input)
        .map_err(|error| format!("project path '{}' is unavailable: {error}", input.display()))?;
    if link_metadata.file_type().is_symlink() {
        return Err("project path must not be a symlink".to_string());
    }
    let metadata = fs::metadata(input)
        .map_err(|error| format!("project path '{}' is unavailable: {error}", input.display()))?;
    if metadata.is_dir() {
        return fs::canonicalize(input).map_err(|error| error.to_string());
    }
    if !metadata.is_file() {
        return Err(format!(
            "project path is not a file or directory: {}",
            input.display()
        ));
    }
    let name = input
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "project path filename is not Unicode".to_string())?;
    let extension = input.extension().and_then(|value| value.to_str());
    if extension.is_some_and(|value| value.eq_ignore_ascii_case("eap"))
        || name.eq_ignore_ascii_case(crate::native_project::LEGACY_PROJECT_MANIFEST_FILE)
    {
        return fs::canonicalize(input)
            .map_err(|error| error.to_string())?
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "project manifest has no parent directory".to_string());
    }
    if extension.is_some_and(|value| value.eq_ignore_ascii_case("eudproj")) {
        validate_legacy_descriptor(input)?;
        return fs::canonicalize(input)
            .map_err(|error| error.to_string())?
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "project descriptor has no parent directory".to_string());
    }
    Err(
        "project launcher accepts a directory, a .eap manifest, or a legacy project.json/.eudproj import"
            .to_string(),
    )
}

fn validate_legacy_descriptor(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("project.eudproj must be a regular, non-symlink file".to_string());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 4096 {
        return Err("project.eudproj descriptor exceeds 4 KiB".to_string());
    }
    let descriptor: LegacyProjectDescriptor = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid project.eudproj descriptor: {error}"))?;
    if descriptor.schema_version != 1
        || descriptor.manifest != crate::native_project::LEGACY_PROJECT_MANIFEST_FILE
    {
        return Err(
            "invalid project.eudproj descriptor: manifest must be project.json and schemaVersion must be 1"
                .to_string(),
        );
    }
    Ok(())
}

fn capture_legacy_migration(input: &Path) -> Result<Option<LegacyMigrationBackup>, String> {
    let root = resolve_project_root(input)?;
    let canonical = root.join(PROJECT_MANIFEST_FILE);
    let legacy = root.join(crate::native_project::LEGACY_PROJECT_MANIFEST_FILE);
    let canonical_existed = canonical.is_file();
    let canonical_v1 = if canonical_existed {
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&canonical).map_err(stringify_io)?)
                .map_err(|error| format!("project.eap JSON을 읽지 못했습니다: {error}"))?;
        value
            .get("schemaVersion")
            .and_then(serde_json::Value::as_u64)
            == Some(crate::native_project::LEGACY_PROJECT_SCHEMA_VERSION as u64)
    } else {
        false
    };
    if !canonical_v1 && !legacy.is_file() {
        return Ok(None);
    }
    let authority = if canonical_v1 { &canonical } else { &legacy };
    let authority_bytes = fs::read(authority).map_err(stringify_io)?;
    let mut paths = vec![authority.clone()];
    if legacy.is_file() && authority != &legacy {
        paths.push(legacy);
    }
    for descriptor in legacy_descriptor_files(&root)? {
        if !paths.contains(&descriptor) {
            paths.push(descriptor);
        }
    }
    let files = paths
        .into_iter()
        .map(|path| {
            fs::read(&path)
                .map(|bytes| (path, bytes))
                .map_err(stringify_io)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(LegacyMigrationBackup {
        root,
        authority_bytes,
        files,
        canonical_existed,
    }))
}

fn restore_legacy_migration(backup: &LegacyMigrationBackup) -> Result<(), String> {
    let canonical = backup.root.join(PROJECT_MANIFEST_FILE);
    if !backup.canonical_existed && canonical.exists() {
        fs::remove_file(&canonical).map_err(stringify_io)?;
    }
    for (path, bytes) in &backup.files {
        crate::memory::write_atomic_bytes(path, bytes).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Open a project input, migrating legacy files only when this function was explicitly called.
fn open_project_path(input: &Path) -> Result<NativeProject, String> {
    let root = resolve_project_root(input)?;
    let input_canonical = fs::canonicalize(input).map_err(|error| error.to_string())?;
    let input_is_file = input_canonical.is_file();
    let input_extension = input.extension().and_then(|value| value.to_str());
    let eap_files = discover_manifest_files(&root)?;
    if eap_files.len() > 1 {
        return Err(
            "native project has ambiguous canonical manifests (multiple .eap files)".to_string(),
        );
    }
    let legacy_descriptors = legacy_descriptor_files(&root)?;
    let legacy_manifest = root.join(crate::native_project::LEGACY_PROJECT_MANIFEST_FILE);
    let has_legacy_manifest = match fs::symlink_metadata(&legacy_manifest) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err("legacy project.json must be a regular file".to_string());
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.to_string()),
    };
    let has_legacy = has_legacy_manifest || !legacy_descriptors.is_empty();
    if input_extension.is_some_and(|value| value.eq_ignore_ascii_case("eap")) {
        if eap_files.len() != 1 || eap_files[0] != input_canonical {
            return Err("requested .eap manifest is not the sole canonical manifest".to_string());
        }
        if has_legacy {
            return Err("project has both canonical and legacy manifest authorities".to_string());
        }
        return open_explicit_manifest(&input_canonical);
    }
    if !input_is_file && eap_files.len() == 1 {
        if has_legacy {
            return Err("project has both canonical and legacy manifest authorities".to_string());
        }
        return open_explicit_manifest(&eap_files[0]);
    }
    if input_is_file && eap_files.len() == 1 {
        return Err("project has both canonical and legacy manifest authorities".to_string());
    }
    let descriptor_bytes: Vec<_> = legacy_descriptors
        .iter()
        .map(|path| {
            fs::read(path)
                .map(|bytes| (path.clone(), bytes))
                .map_err(|error| error.to_string())
        })
        .collect::<Result<_, _>>()?;
    let legacy_manifest_bytes = fs::read(&legacy_manifest).map_err(|error| error.to_string())?;
    let project = NativeProject::migrate_legacy(&root)?;
    for (index, descriptor) in legacy_descriptors.iter().enumerate() {
        if let Err(error) = fs::remove_file(descriptor) {
            let mut rollback_errors = Vec::new();
            for (path, bytes) in descriptor_bytes.iter().take(index) {
                if let Err(restore_error) = crate::memory::write_atomic_bytes(path, bytes) {
                    rollback_errors.push(format!("{}: {restore_error}", path.display()));
                }
            }
            let canonical = root.join(crate::native_project::PROJECT_MANIFEST_FILE);
            if let Err(restore_error) = fs::remove_file(&canonical) {
                rollback_errors.push(format!(
                    "생성된 project.eap을 제거하지 못했습니다: {restore_error}"
                ));
            }
            if let Err(restore_error) =
                crate::memory::write_atomic_bytes(&legacy_manifest, &legacy_manifest_bytes)
            {
                rollback_errors.push(format!(
                    "원래 project.json을 복원하지 못했습니다: {restore_error}"
                ));
            }
            let recovery = if rollback_errors.is_empty() {
                "migration rolled back; check file permissions and retry".to_string()
            } else {
                format!(
                    "migration rollback incomplete: {}",
                    rollback_errors.join("; ")
                )
            };
            return Err(format!(
                "legacy descriptor '{}' could not be removed: {error}; {recovery}",
                descriptor.display()
            ));
        }
    }
    Ok(project)
}

fn open_explicit_manifest(path: &Path) -> Result<NativeProject, String> {
    match crate::native_project::project_manifest_schema_version(path)? {
        crate::native_project::PROJECT_SCHEMA_VERSION => NativeProject::open_manifest(path),
        crate::native_project::LEGACY_PROJECT_SCHEMA_VERSION => {
            NativeProject::migrate_v1_manifest(path)
        }
        version => Err(format!(
            "지원하지 않는 project.eap schemaVersion입니다: {version}"
        )),
    }
}

fn legacy_descriptor_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut descriptors = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .path()
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("eudproj"))
        {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(format!(
                "legacy descriptor must be a regular file: {}",
                path.display()
            ));
        }
        validate_legacy_descriptor(&path)?;
        descriptors.push(path);
    }
    Ok(descriptors)
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().try_into().unwrap_or(u64::MAX)
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeFileInfo {
    pub path: String,
    pub file_type: String,
    pub settable: bool,
}

#[derive(Clone)]
pub struct NativeProjectManager {
    dirs: DataDirs,
    transaction: Arc<Mutex<()>>,
    python_candidates: Arc<Mutex<HashMap<String, PythonDependencyCandidate>>>,
}

impl NativeProjectManager {
    pub fn new(dirs: DataDirs) -> Self {
        Self {
            dirs,
            transaction: Arc::new(Mutex::new(())),
            python_candidates: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn data_dirs(&self) -> &DataDirs {
        &self.dirs
    }

    pub fn project_root(&self) -> Result<PathBuf, String> {
        let config = self.dirs.load_config().map_err(|error| error.to_string())?;
        let configured = config.project_path.trim();
        if configured.is_empty() {
            return Err(PROJECT_NOT_CONFIGURED.to_string());
        }
        Ok(PathBuf::from(configured))
    }

    pub fn open(&self) -> Result<NativeProject, String> {
        NativeProject::open(&self.project_root()?)
    }

    pub fn configure_project(
        &self,
        root: &Path,
    ) -> Result<Vec<crate::harness_import::HarnessImportIssue>, String> {
        let _transaction = self.transaction.lock();
        let migration = capture_legacy_migration(root)?;
        let project = open_project_path(root)?;
        if let Some(backup) = migration.as_ref() {
            if let Err(error) = self.archive_legacy_migration(backup, &project) {
                restore_legacy_migration(backup).map_err(|restore_error| {
                    format!(
                        "프로젝트 변환 저널 저장과 원본 복원이 모두 실패했습니다: {error}; {restore_error}"
                    )
                })?;
                return Err(format!(
                    "프로젝트 변환 저널을 저장하지 못해 원본을 복원했습니다: {error}"
                ));
            }
        }
        project.status()?;
        // Local authority moves with the project. Once initialized, never backfill
        // deleted local data from an old machine's name/path-keyed stores.
        let receipt = project
            .root()
            .join(".eud-agent/state/appdata-migration.json");
        if fs::symlink_metadata(&receipt).is_ok() {
            self.activate_project(&project)?;
            return Ok(Vec::new());
        }
        let mut issues = Vec::new();
        let workspace = crate::workspace::WorkspaceManager::new(self.dirs.clone())
            .migrate_native_harness(&project, &mut issues)
            .map_err(stringify_io)?;
        let memory_result = if workspace.native_memory_source_is_unique() {
            crate::memory::ProjectMemory::migrate_native_harness(
                &self.dirs.memory_dir(),
                &project,
                &mut issues,
            )
        } else {
            for name in [
                project.manifest().name.clone(),
                format!("'{}'", project.manifest().name),
                format!("\"{}\"", project.manifest().name),
            ] {
                let path = self
                    .dirs
                    .memory_dir()
                    .join(crate::memory::sanitize_project_name(&name));
                if fs::symlink_metadata(&path).is_ok() {
                    issues.push(crate::harness_import::HarnessImportIssue::new(
                        "memory",
                        path.to_string_lossy(),
                        "이름으로 저장된 이전 메모리가 이 프로젝트의 자료인지 유일하게 확인할 수 없어 제외했습니다.",
                    ));
                }
            }
            Ok(crate::memory::LegacyMemoryImport::empty())
        };
        let memory = match memory_result {
            Ok(memory) => memory,
            Err(error) => {
                let mut message = error.to_string();
                if let Err(rollback) = workspace.rollback() {
                    message.push_str(&format!("; workspace rollback failed: {rollback}"));
                }
                return Err(message);
            }
        };
        if let Err(mut error) = self.activate_project_with_issues(&project, &issues) {
            if let Err(rollback) = memory.rollback() {
                error.push_str(&format!("; memory rollback failed: {rollback}"));
            }
            if let Err(rollback) = workspace.rollback() {
                error.push_str(&format!("; workspace rollback failed: {rollback}"));
            }
            return Err(error);
        }
        memory.commit();
        workspace.commit();
        Ok(issues)
    }

    /// Activate a freshly created/imported project without attaching unrelated
    /// legacy data that happened to use its new name or destination path.
    pub(crate) fn activate_project(&self, project: &NativeProject) -> Result<(), String> {
        self.activate_project_with_issues(project, &[])
    }

    fn activate_project_with_issues(
        &self,
        project: &NativeProject,
        issues: &[crate::harness_import::HarnessImportIssue],
    ) -> Result<(), String> {
        let state =
            crate::harness_import::local_state_directory(project.root()).map_err(stringify_io)?;
        let receipt = state.join("appdata-migration.json");
        let created_receipt = match fs::symlink_metadata(&receipt) {
            Ok(metadata)
                if metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && !crate::memory::is_reparse_point(&metadata) =>
            {
                false
            }
            Ok(_) => return Err("하네스 이관 기록이 안전한 일반 파일이 아닙니다.".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let bytes = serde_json::to_vec_pretty(issues).map_err(|error| error.to_string())?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&receipt)
                    .map_err(stringify_io)?;
                use std::io::Write;
                if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
                    drop(file);
                    return match fs::remove_file(&receipt) {
                        Ok(()) => Err(error.to_string()),
                        Err(cleanup) => Err(format!(
                            "{error}; migration receipt cleanup failed: {cleanup}"
                        )),
                    };
                }
                true
            }
            Err(error) => return Err(error.to_string()),
        };
        let result = (|| {
            let _config_guard = crate::config::PROJECT_CONFIG_LOCK.lock();
            let mut config = self.dirs.load_config().map_err(|error| error.to_string())?;
            config.project_path = project.root().to_string_lossy().into_owned();
            config.record_recent_project(
                project.manifest().name.clone(),
                config.project_path.clone(),
                unix_millis(),
            );
            self.dirs
                .save_config(&config)
                .map_err(|error| error.to_string())
        })();
        if let Err(mut error) = result {
            if created_receipt {
                if let Err(cleanup) = fs::remove_file(&receipt) {
                    error.push_str(&format!("; migration receipt cleanup failed: {cleanup}"));
                }
            }
            return Err(error);
        }
        self.prepare_git(project);
        Ok(())
    }

    /// Give the opened project the repository that the app rolls back with.
    ///
    /// A git problem never fails an open: the project's authoring state is on
    /// disk either way, and refusing to open it would cost the user more than
    /// the missing history does.
    fn prepare_git(&self, project: &NativeProject) {
        let state = crate::git::prepare(project.root());
        if let Some(warning) = state.warning.as_deref() {
            eprintln!("eud-agent: {warning}");
        }
    }

    fn archive_legacy_migration(
        &self,
        backup: &LegacyMigrationBackup,
        project: &NativeProject,
    ) -> Result<(), String> {
        let after_manifest = fs::read(project.manifest_path()).map_err(stringify_io)?;
        let request_id = format!("project-migration-{}", uuid::Uuid::new_v4().simple());
        let journal = JournalStore::new(self.dirs.app_data());
        journal
            .record(
                &request_id,
                JournalEntry {
                    id: "project-manifest-migrate-1".to_string(),
                    seq: 1,
                    tool: WriteTool::ProjectManifestMigrate,
                    target: JournalTarget::ProjectManifest {
                        path: PROJECT_MANIFEST_FILE.to_string(),
                    },
                    before: Snapshot::ManifestBytes {
                        bytes: backup.authority_bytes.clone(),
                        manifest_sha256: sha256_bytes(&backup.authority_bytes),
                    },
                    after: Snapshot::ManifestBytes {
                        manifest_sha256: sha256_bytes(&after_manifest),
                        bytes: after_manifest,
                    },
                    ts: unix_millis() / 1_000,
                },
            )
            .map_err(|error| error.to_string())?;
        journal
            .persist(&request_id)
            .map_err(|error| error.to_string())?;
        if let Err(error) = journal.archive(&request_id) {
            let _ = fs::remove_file(
                self.dirs
                    .app_data()
                    .join("journal")
                    .join(format!("{request_id}.json")),
            );
            return Err(error.to_string());
        }
        Ok(())
    }

    pub fn create_project(
        &self,
        root: &Path,
        manifest: ProjectManifest,
    ) -> Result<NativeProjectStatus, String> {
        let _transaction = self.transaction.lock();
        if root.exists()
            && fs::read_dir(root)
                .map_err(|error| error.to_string())?
                .next()
                .is_some()
        {
            return Err(format!(
                "project destination must be empty: {}",
                root.display()
            ));
        }
        let project = NativeProject::create(root, manifest)?;
        let status = project.status()?;
        self.activate_project(&project)?;
        Ok(status)
    }
    pub fn status(&self) -> Result<NativeProjectStatus, String> {
        self.open()?.status()
    }
    pub fn source_snapshot(&self) -> Result<NativeSourceSnapshot, String> {
        self.open()?.source_snapshot()
    }

    pub fn list_files(&self) -> Result<Vec<NativeFileInfo>, String> {
        Ok(self
            .open()?
            .list_source_files()?
            .into_iter()
            .map(|path| {
                let extension = Path::new(&path)
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default();
                let (file_type, settable) = if extension.eq_ignore_ascii_case("eps") {
                    ("CUIEps", true)
                } else if extension.eq_ignore_ascii_case("py") {
                    ("CUIPy", true)
                } else {
                    ("RawText", false)
                };
                NativeFileInfo {
                    settable,
                    file_type: file_type.to_string(),
                    path,
                }
            })
            .collect())
    }

    pub fn read_source(&self, path: &str) -> Result<String, String> {
        self.open()?.read_source(path)
    }

    /// Render the `[project map]` prompt section: MainFile, ordered Python
    /// entrypoints, every `src/**` file with its byte length and first
    /// meaningful line, plugin sections, sparse DAT override counts, and the
    /// accepted spec index. It is hashed by the context cursor, so it costs a
    /// turn only when something in it changes.
    pub fn render_project_map(&self) -> Result<String, String> {
        const MAX_SPEC_INDEX_BYTES: usize = 8 * 1024;
        const MAX_FILES: usize = 400;
        let project = self.open()?;
        let manifest = project.manifest();
        let snapshot = project.source_snapshot()?;
        let mut out = format!(
            "[project map]
mainFile={}
sourceMap={}
revision={}
",
            manifest.main_file, manifest.source_map, snapshot.revision
        );
        if !manifest.python_entrypoints.is_empty() {
            out.push_str(&format!(
                "pythonEntrypoints={}
",
                manifest.python_entrypoints.join(", ")
            ));
        }
        out.push_str(&format!(
            "files ({}):
",
            snapshot.files.len()
        ));
        for file in snapshot.files.iter().take(MAX_FILES) {
            let first_line = file
                .content
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default();
            let first_line = first_line.chars().take(96).collect::<String>();
            out.push_str(&format!(
                "- {} ({} bytes) {}
",
                file.path,
                file.content.len(),
                first_line
            ));
        }
        if snapshot.files.len() > MAX_FILES {
            out.push_str(&format!(
                "- … {} more files
",
                snapshot.files.len() - MAX_FILES
            ));
        }
        if !manifest.plugins.is_empty() {
            out.push_str(&format!(
                "plugins: {}
",
                manifest
                    .plugins
                    .iter()
                    .map(|plugin| plugin.section.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let dat = project.dat();
        let numeric = |tables: &crate::native_project::NumericTables| -> usize {
            tables
                .values()
                .map(|objects| objects.values().map(|fields| fields.len()).sum::<usize>())
                .sum()
        };
        let overrides = [
            ("standard", numeric(&dat.standard.tables)),
            ("xdat", numeric(&dat.xdat.tables)),
            ("tbl", dat.tbl.values.len()),
            (
                "requirements",
                dat.requirements
                    .tables
                    .values()
                    .map(|objects| objects.len())
                    .sum::<usize>(),
            ),
            ("buttons", dat.buttons.values.len()),
        ];
        out.push_str(&format!(
            "dat overrides: {}
",
            overrides
                .iter()
                .map(|(family, count)| format!("{family}={count}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let workspace_root = project.root().join(".eud-agent").join("workspace");
        if let Ok(index) = std::fs::read_to_string(workspace_root.join("specs").join("index.md")) {
            let mut index = index.trim().to_string();
            if index.len() > MAX_SPEC_INDEX_BYTES {
                let mut cut = MAX_SPEC_INDEX_BYTES;
                while !index.is_char_boundary(cut) {
                    cut -= 1;
                }
                index.truncate(cut);
                index.push_str(
                    "
…",
                );
            }
            if !index.is_empty() {
                out.push_str(&format!(
                    "specs/index.md:
{index}
"
                ));
            }
        }
        if let Ok(entries) = std::fs::read_dir(workspace_root.join("worklog")) {
            let mut worklogs = entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| name.ends_with(".md"))
                .collect::<Vec<_>>();
            worklogs.sort();
            if !worklogs.is_empty() {
                let recent = worklogs.iter().rev().take(10).cloned().collect::<Vec<_>>();
                out.push_str(&format!(
                    "worklog: {}
",
                    recent.join(", ")
                ));
            }
        }
        Ok(out)
    }

    pub fn write_source(&self, path: &str, content: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.write_source(path, content)
    }

    pub fn create_source(&self, path: &str, content: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.create_source(path, content)
    }

    pub fn create_source_dir(&self, path: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.create_source_dir(path)
    }

    pub fn move_source(&self, from: &str, to: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.move_source(from, to)
    }

    pub fn delete_source(&self, path: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.delete_source(path)
    }

    pub fn set_main_file(&self, path: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.set_main_file(path)
    }

    pub fn set_project_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.set_project_setting(key, value)
    }

    pub fn plugin_add(&self, index: i64, raw_text: &str) -> Result<usize, String> {
        let _transaction = self.transaction.lock();
        self.open()?.plugin_add(index, raw_text)
    }

    pub fn plugin_edit(&self, index: usize, raw_text: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.plugin_edit(index, raw_text)
    }

    pub fn plugin_remove(&self, index: usize) -> Result<crate::native_project::EdsPlugin, String> {
        let _transaction = self.transaction.lock();
        self.open()?.plugin_remove(index)
    }

    pub fn plugin_move(&self, from: usize, to: usize) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.plugin_move(from, to)
    }

    /// What the build refuses before euddraft starts.
    ///
    /// The tool schemas used to be the only gate on these rules. An edit made
    /// straight to the project tree goes around them, so the build checks them
    /// again at the boundary where it still costs nothing to say no.
    ///
    /// `NativeProject::open` has already validated the manifest, the path
    /// rules, and that MainFile and the source map exist; what is left is what
    /// opening cannot see.
    fn build_preflight(&self, project: &NativeProject) -> Result<Vec<NativeBuildError>, String> {
        let mut refusals = Vec::new();
        let catalog = DatCatalog::load(&self.dirs.native_assets_dir())?;
        for entry in project.dat_overrides() {
            match baseline_value(&catalog, &entry.target) {
                Ok(stock) if stock == entry.before => {}
                Ok(stock) => refusals.push(preflight_error(
                    entry.file,
                    format!(
                        "{}의 before가 원본 값과 다릅니다 (원본 {}, 기록된 before {}). 생성기는 after - before를 런타임 델타로 내보내므로 이대로 빌드하면 맵이 잘못된 값으로 돕니다. before를 원본 값으로 고치거나 이 override를 지우세요.",
                        entry.label,
                        scalar_text(&stock),
                        scalar_text(&entry.before)
                    ),
                )),
                Err(reason) => refusals.push(preflight_error(
                    entry.file,
                    format!("{}를 원본 카탈로그에서 찾을 수 없습니다: {reason}", entry.label),
                )),
            }
        }
        for path in project.list_source_files()? {
            if !Path::new(&path)
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    value.eq_ignore_ascii_case("eps") || value.eq_ignore_ascii_case("py")
                })
            {
                continue;
            }
            if let Err(reason) = crate::native_project::require_editable_source_path(&path) {
                refusals.push(preflight_error(
                    path.clone(),
                    format!("이 소스 파일은 프로젝트가 쓸 수 없는 이름입니다: {reason}"),
                ));
            }
        }
        Ok(refusals)
    }

    pub fn dat_values(
        &self,
        targets: &[DatTarget],
    ) -> Result<BTreeMap<DatTarget, DatScalar>, String> {
        let project = self.open()?;
        let catalog = DatCatalog::load(&self.dirs.native_assets_dir())?;
        let mut values = BTreeMap::new();
        for target in targets {
            let value = match project.current_dat_value(target) {
                Some(value) => value,
                None => baseline_value(&catalog, target)?,
            };
            values.insert(target.clone(), value);
        }
        Ok(values)
    }

    /// The inclusive object-id range one DAT field covers in the version-matched
    /// catalog. A caller that has to sweep a whole table (which images use an
    /// iscript, say) takes the range from the catalog instead of hardcoding a
    /// table size that the compatibility assets own.
    pub fn dat_field_range(&self, dat: &str, field: &str) -> Result<(u32, u32), String> {
        let catalog = DatCatalog::load(&self.dirs.native_assets_dir())?;
        let meta = catalog.field(dat, field)?;
        Ok((meta.var_start, meta.var_end))
    }

    pub fn apply_dat_patch(&self, patch: &NativeDatPatch) -> Result<String, String> {
        let _transaction = self.transaction.lock();
        let mut project = self.open()?;
        let targets: Vec<_> = patch.changes.iter().map(|change| change.target()).collect();
        let catalog = DatCatalog::load(&self.dirs.native_assets_dir())?;
        let mut baseline = BTreeMap::new();
        for target in targets {
            if project.current_dat_value(&target).is_none() {
                baseline.insert(target.clone(), baseline_value(&catalog, &target)?);
            }
        }
        project
            .apply_dat_patch(patch, &baseline)
            .map(|result| result.revision)
    }

    pub fn restore_manifest_bytes(
        &self,
        expected_manifest_sha256: &str,
        bytes: &[u8],
    ) -> Result<String, String> {
        let _transaction = self.transaction.lock();
        self.open()?
            .restore_manifest_bytes(expected_manifest_sha256, bytes)
    }

    pub fn prepare_python_dependencies(
        &self,
        session_id: &str,
        project_id: &str,
        dependencies: Vec<String>,
    ) -> Result<PythonDependenciesPrepareResult, String> {
        self.prepare_python_dependencies_with_cancellation(
            session_id,
            project_id,
            dependencies,
            None,
        )
    }

    pub fn prepare_python_dependencies_with_cancellation(
        &self,
        session_id: &str,
        project_id: &str,
        dependencies: Vec<String>,
        cancellation: Option<&ProcessCancellation>,
    ) -> Result<PythonDependenciesPrepareResult, String> {
        let preparation_lock = PYTHON_PREPARATION_LOCK.get_or_init(|| Mutex::new(()));
        let _preparation = loop {
            if cancellation.is_some_and(ProcessCancellation::is_cancelled) {
                return Err("Python 의존성 준비가 취소되었습니다.".to_string());
            }
            if let Some(guard) = preparation_lock.try_lock() {
                break guard;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        if cancellation.is_some_and(ProcessCancellation::is_cancelled) {
            return Err("Python 의존성 준비가 취소되었습니다.".to_string());
        }
        if dependencies.len() > 128 {
            return Err("Python 직접 의존성은 최대 128개까지 준비할 수 있습니다.".to_string());
        }
        if project_id.is_empty() || project_id.len() > 512 {
            return Err("프로젝트 식별자가 올바르지 않습니다.".to_string());
        }
        if session_id.is_empty() || session_id.len() > 256 {
            return Err("세션 ID가 올바르지 않습니다.".to_string());
        }
        let dependencies = validate_python_dependencies(&dependencies)?;
        let input_digest = python_input_digest(&dependencies)?;
        let project = self.open()?;
        let project_root = fs::canonicalize(project.root()).map_err(stringify_io)?;
        let cache_key = python_project_cache_key(&project_root);
        let base_revision = project.revision()?;
        let manifest_path = project.manifest_path().to_path_buf();
        let manifest_bytes = fs::read(&manifest_path).map_err(stringify_io)?;
        let manifest_sha256 = sha256_bytes(&manifest_bytes);
        // Managed uv and the win_amd64 wheel policy target the Windows frozen runtime.
        if !cfg!(windows) {
            return Err("Python 의존성 준비는 현재 Windows에서만 지원됩니다.".to_string());
        }
        let euddraft = self.frozen_euddraft()?;
        let current_euddraft_fingerprint = euddraft.frozen_fingerprint()?;
        {
            let mut candidates = self.python_candidates.lock();
            let now = unix_millis();
            candidates.retain(|_, candidate| candidate.expires_at >= now);
            if let Some((token, candidate)) = candidates.iter().find(|(_, candidate)| {
                candidate.session_id == session_id
                    && candidate.project_id == project_id
                    && candidate.project_root == project_root
                    && candidate.base_revision == base_revision
                    && candidate.manifest_sha256 == manifest_sha256
                    && candidate.input_digest == input_digest
                    && candidate.euddraft_fingerprint == current_euddraft_fingerprint
            }) {
                let resolved_packages = candidate
                    .python_lock
                    .as_ref()
                    .map(|lock| {
                        lock.packages
                            .iter()
                            .map(|package| PythonResolvedPackageSummary {
                                name: package.name.clone(),
                                version: package.version.clone(),
                                wheel_filename: package.wheel_filename.clone(),
                                sha256: package.sha256.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let result = PythonDependenciesPrepareResult {
                    candidate_token: token.clone(),
                    normalized_dependencies: candidate.dependencies.clone(),
                    resolved_packages,
                    lock_digest: candidate.lock_digest.clone(),
                    input_digest: candidate.input_digest.clone(),
                    cache_identity: candidate.cache_identity.clone(),
                    base_revision: candidate.base_revision.clone(),
                    manifest_sha256: candidate.manifest_sha256.clone(),
                    euddraft_fingerprint: candidate.euddraft_fingerprint.clone(),
                    expires_at: candidate.expires_at,
                };
                return Ok(result);
            }
        }
        let uv = crate::bootstrap::managed_uv_path(&self.dirs).map_err(|error| {
            format!("검증된 관리형 uv {MANAGED_UV_VERSION}을 찾지 못했습니다: {error}")
        })?;
        let project_env_root = self.dirs.python_envs_dir().join(&cache_key);
        ensure_plain_directory_path(&self.dirs.python_envs_dir(), &project_env_root)?;

        let probe_root =
            project_env_root.join(format!(".runtime-probe-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&probe_root).map_err(stringify_io)?;
        let identity_result =
            probe_frozen_python(&project, &euddraft, &probe_root, None, &[], cancellation);
        let _ = fs::remove_dir_all(&probe_root);
        let identity = identity_result?;
        let target = PythonLockTarget {
            os: PYTHON_LOCK_OS.to_string(),
            arch: PYTHON_LOCK_ARCH.to_string(),
            python_abi: identity.python_abi.clone(),
            euddraft_fingerprint: identity.euddraft_fingerprint.clone(),
            uv_version: MANAGED_UV_VERSION.to_string(),
        };

        let prepared = self.find_cached_python_environment(&cache_key, &dependencies, &identity)?;
        let (lock, cache_identity) = match prepared {
            Some((marker, cache_identity, site_packages)) => {
                probe_cached_environment(
                    &project,
                    &euddraft,
                    &project_env_root,
                    &site_packages,
                    &marker,
                    cancellation,
                )?;
                (marker.lock, cache_identity)
            }
            None => self.resolve_python_environment(
                &uv,
                &project,
                &euddraft,
                &dependencies,
                &target,
                cancellation,
            )?,
        };
        let lock_digest = lock.digest.clone();
        let python_lock = (!dependencies.is_empty()).then_some(lock.clone());
        if let Some(lock) = python_lock.as_ref() {
            validate_python_lock(&dependencies, Some(lock))?;
        }

        let current = self.open()?;
        let current_root = fs::canonicalize(current.root()).map_err(stringify_io)?;
        let current_manifest = fs::read(current.manifest_path()).map_err(stringify_io)?;
        if current_root != project_root
            || current.revision()? != base_revision
            || sha256_bytes(&current_manifest) != manifest_sha256
        {
            return Err(
                "준비 중 프로젝트가 변경되었습니다. 의존성을 다시 준비해 주세요.".to_string(),
            );
        }

        let expires_at = unix_millis().saturating_add(PYTHON_CANDIDATE_TTL_MILLIS);
        let token = format!("pydep-{}", uuid::Uuid::new_v4().simple());
        let candidate = PythonDependencyCandidate {
            session_id: session_id.to_string(),
            project_id: project_id.to_string(),
            project_root,
            cache_key,
            base_revision,
            manifest_sha256,
            input_digest,
            cache_identity,
            euddraft_fingerprint: identity.euddraft_fingerprint.clone(),
            python_abi: identity.python_abi.clone(),
            expires_at,
            dependencies: dependencies.clone(),
            python_lock,
            lock_digest: lock_digest.clone(),
        };
        let mut candidates = self.python_candidates.lock();
        let now = unix_millis();
        candidates.retain(|_, candidate| candidate.expires_at >= now);
        let result = PythonDependenciesPrepareResult {
            candidate_token: token.clone(),
            normalized_dependencies: dependencies,
            resolved_packages: lock
                .packages
                .iter()
                .map(|package| PythonResolvedPackageSummary {
                    name: package.name.clone(),
                    version: package.version.clone(),
                    wheel_filename: package.wheel_filename.clone(),
                    sha256: package.sha256.clone(),
                })
                .collect(),
            lock_digest,
            input_digest: candidate.input_digest.clone(),
            cache_identity: candidate.cache_identity.clone(),
            base_revision: candidate.base_revision.clone(),
            manifest_sha256: candidate.manifest_sha256.clone(),
            euddraft_fingerprint: identity.euddraft_fingerprint,
            expires_at,
        };
        if serde_json::to_vec(&result)
            .map_err(|error| error.to_string())?
            .len()
            > MAX_PREPARE_RESPONSE_BYTES
        {
            return Err("Python 의존성 준비 결과가 응답 크기 제한을 초과했습니다.".to_string());
        }
        candidates.insert(token, candidate);
        Ok(result)
    }

    pub fn claim_python_dependencies(
        &self,
        token: &str,
        session_id: &str,
        project_id: &str,
    ) -> Result<ClaimedPythonDependencies, String> {
        if token.len() != 38 || !token.starts_with("pydep-") {
            return Err("Python 의존성 후보 토큰이 올바르지 않습니다.".to_string());
        }
        let mut candidates = self.python_candidates.lock();
        let candidate = candidates
            .get(token)
            .ok_or_else(|| "Python 의존성 후보 토큰이 없거나 이미 사용되었습니다.".to_string())?;
        if candidate.session_id != session_id || candidate.project_id != project_id {
            return Err(
                "Python 의존성 후보가 현재 세션 또는 프로젝트에 속하지 않습니다.".to_string(),
            );
        }
        if candidate.expires_at < unix_millis() {
            candidates.remove(token);
            return Err(
                "Python 의존성 후보 토큰이 만료되었습니다. 다시 준비해 주세요.".to_string(),
            );
        }
        let candidate = candidates
            .remove(token)
            .expect("validated Python dependency candidate must still exist");
        Ok(ClaimedPythonDependencies {
            session_id: candidate.session_id,
            project_id: candidate.project_id,
            project_root: candidate.project_root,
            cache_key: candidate.cache_key,
            base_revision: candidate.base_revision,
            manifest_sha256: candidate.manifest_sha256,
            input_digest: candidate.input_digest,
            cache_identity: candidate.cache_identity,
            euddraft_fingerprint: candidate.euddraft_fingerprint,
            python_abi: candidate.python_abi,
            expires_at: candidate.expires_at,
            dependencies: candidate.dependencies,
            python_lock: candidate.python_lock,
            lock_digest: candidate.lock_digest,
        })
    }

    pub fn revoke_python_dependency_candidates(&self, session_id: &str) {
        self.python_candidates
            .lock()
            .retain(|_, candidate| candidate.session_id != session_id);
    }

    pub fn commit_python_dependencies(
        &self,
        claimed: ClaimedPythonDependencies,
        session_id: &str,
        project_id: &str,
    ) -> Result<PythonDependenciesCommit, String> {
        if claimed.session_id != session_id || claimed.project_id != project_id {
            return Err("Python 의존성 후보 바인딩이 현재 요청과 다릅니다.".to_string());
        }
        if claimed.expires_at < unix_millis() {
            return Err(
                "Python 의존성 후보 토큰이 만료되었습니다. 다시 준비해 주세요.".to_string(),
            );
        }
        if python_input_digest(&claimed.dependencies)? != claimed.input_digest {
            return Err("Python 의존성 후보 입력 지문이 손상되었습니다.".to_string());
        }
        let _transaction = self.transaction.lock();
        let mut project = self.open()?;
        let root = fs::canonicalize(project.root()).map_err(stringify_io)?;
        let manifest_path = project.manifest_path().to_path_buf();
        let before_manifest = fs::read(&manifest_path).map_err(stringify_io)?;
        if root != claimed.project_root
            || project.revision()? != claimed.base_revision
            || sha256_bytes(&before_manifest) != claimed.manifest_sha256
        {
            return Err(
                "프로젝트가 준비 시점 이후 변경되었습니다. 의존성을 다시 준비해 주세요."
                    .to_string(),
            );
        }
        let current_fingerprint = self.frozen_euddraft()?.frozen_fingerprint()?;
        if current_fingerprint != claimed.euddraft_fingerprint {
            return Err("frozen euddraft.exe 지문이 의존성 준비 이후 변경되었습니다.".to_string());
        }
        match claimed.python_lock.as_ref() {
            Some(lock) => {
                if lock.target.python_abi != claimed.python_abi
                    || lock.target.euddraft_fingerprint != claimed.euddraft_fingerprint
                {
                    return Err("Python 의존성 후보 런타임 지문이 잠금과 다릅니다.".to_string());
                }
                if lock.digest != claimed.lock_digest {
                    return Err("Python 의존성 후보 잠금 지문이 손상되었습니다.".to_string());
                }
                validate_python_lock(&claimed.dependencies, Some(lock))?;
                let (_, cache_identity) = validate_python_environment(
                    &self.dirs,
                    &claimed.cache_key,
                    &claimed.dependencies,
                    lock,
                )?;
                if cache_identity != claimed.cache_identity {
                    return Err("준비된 Python 환경 식별자가 변경되었습니다.".to_string());
                }
            }
            None => {
                if !claimed.dependencies.is_empty() {
                    return Err("Python 의존성 후보에 잠금 정보가 없습니다.".to_string());
                }
                let marker_lock = empty_python_environment_lock(
                    &self.dirs,
                    &claimed.cache_key,
                    &claimed.lock_digest,
                )?;
                if marker_lock.0.target.python_abi != claimed.python_abi
                    || marker_lock.0.target.euddraft_fingerprint != claimed.euddraft_fingerprint
                {
                    return Err("준비된 빈 Python 환경의 런타임 지문이 변경되었습니다.".to_string());
                }
                if marker_lock.1 != claimed.cache_identity {
                    return Err("준비된 빈 Python 환경 식별자가 변경되었습니다.".to_string());
                }
            }
        }
        if claimed.expires_at < unix_millis() {
            return Err(
                "Python 의존성 후보 토큰이 검증 중 만료되었습니다. 다시 준비해 주세요.".to_string(),
            );
        }
        let before_manifest_sha256 = sha256_bytes(&before_manifest);
        let (revision, after_manifest) = project
            .replace_python_dependencies(claimed.dependencies.clone(), claimed.python_lock)?;
        let after_manifest_sha256 = sha256_bytes(&after_manifest);
        Ok(PythonDependenciesCommit {
            revision,
            before_manifest,
            before_manifest_sha256,
            after_manifest,
            after_manifest_sha256,
            normalized_dependencies: claimed.dependencies,
            lock_digest: claimed.lock_digest,
        })
    }

    fn frozen_euddraft(&self) -> Result<EuddraftLaunch, String> {
        let config = self.dirs.load_config().map_err(|error| error.to_string())?;
        let configured = config.euddraft_path.trim();
        if configured.is_empty() {
            return Err(EUDDRAFT_NOT_CONFIGURED.to_string());
        }
        let path = Path::new(configured);
        let executable = if path.is_file()
            && path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .eq_ignore_ascii_case(crate::native_build::EUDDRAFT_EXECUTABLE_NAME)
            }) {
            path.to_path_buf()
        } else if path.is_dir()
            && path
                .join(crate::native_build::EUDDRAFT_EXECUTABLE_NAME)
                .is_file()
        {
            path.join(crate::native_build::EUDDRAFT_EXECUTABLE_NAME)
        } else {
            return Err(
                "직접 Python 실행에는 설정된 frozen euddraft.exe가 필요합니다.".to_string(),
            );
        };
        Ok(EuddraftLaunch::Executable(executable))
    }

    fn find_cached_python_environment(
        &self,
        project_id: &str,
        dependencies: &[String],
        identity: &FrozenPythonIdentity,
    ) -> Result<Option<(PythonEnvironmentMarker, String, PathBuf)>, String> {
        let root = self.dirs.python_envs_dir().join(project_id);
        let mut entries = fs::read_dir(&root)
            .map_err(stringify_io)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(stringify_io)?;
        if entries.len() > 1_024 {
            return Err("Python 환경 캐시 항목이 허용 범위를 초과했습니다.".to_string());
        }
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.len() != 64 {
                continue;
            }
            let marker = match read_python_environment_marker(&entry.path()) {
                Ok(marker) => marker,
                Err(_) => continue,
            };
            if marker.project_id != project_id
                || marker.dependencies != dependencies
                || marker.probe != *identity
                || marker.lock.digest != name
            {
                continue;
            }
            let (site_packages, cache_identity) = validate_python_environment_marker(
                &self.dirs,
                project_id,
                dependencies,
                &marker.lock,
                &entry.path(),
            )?;
            return Ok(Some((marker, cache_identity, site_packages)));
        }
        Ok(None)
    }

    fn resolve_python_environment(
        &self,
        uv: &Path,
        project: &NativeProject,
        euddraft: &EuddraftLaunch,
        dependencies: &[String],
        target: &PythonLockTarget,
        cancellation: Option<&ProcessCancellation>,
    ) -> Result<(PythonLock, String), String> {
        let cache_key =
            python_project_cache_key(&fs::canonicalize(project.root()).map_err(stringify_io)?);
        let project_id = cache_key.as_str();
        let project_env_root = self.dirs.python_envs_dir().join(project_id);
        let staging = project_env_root.join(format!(".pending-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&staging).map_err(stringify_io)?;
        let result = (|| {
            let requirements = staging.join("requirements.in");
            let mut requirement_bytes = dependencies.join("\n").into_bytes();
            if !requirement_bytes.is_empty() {
                requirement_bytes.push(b'\n');
            }
            crate::memory::write_atomic_bytes(&requirements, &requirement_bytes)
                .map_err(|error| error.to_string())?;
            let python_version = python_version_from_abi(&target.python_abi)?;
            let packages = if dependencies.is_empty() {
                Vec::new()
            } else {
                let pylock_path = staging.join("pylock.toml");
                run_managed_uv(
                    uv,
                    &staging,
                    &self.dirs.python_downloads_dir().join("uv-cache"),
                    [
                        "--no-config",
                        "--no-python-downloads",
                        "pip",
                        "compile",
                        "--quiet",
                        "--python-platform",
                        "x86_64-pc-windows-msvc",
                        "--python-version",
                        python_version.as_str(),
                        "--default-index",
                        "https://pypi.org/simple",
                        "--index-strategy",
                        "first-index",
                        "--only-binary",
                        ":all:",
                        "--format",
                        "pylock.toml",
                        "--output-file",
                        "pylock.toml",
                        "requirements.in",
                    ],
                    PYTHON_RESOLVE_TIMEOUT,
                    cancellation,
                )
                .map_err(|error| format!("uv 의존성 잠금 생성 실패: {error}"))?;
                let metadata = fs::metadata(&pylock_path).map_err(stringify_io)?;
                if metadata.len() > MAX_PYLOCK_BYTES {
                    return Err("uv 잠금 결과가 허용 크기를 초과했습니다.".to_string());
                }
                let pylock = fs::read_to_string(&pylock_path)
                    .map_err(|error| format!("uv 잠금 결과를 읽지 못했습니다: {error}"))?;
                select_locked_wheels(&parse_pylock(&pylock)?, &target.python_abi)?
            };
            let digest = compute_python_lock_digest(dependencies, target, &packages)?;
            let lock = PythonLock {
                digest: digest.clone(),
                target: target.clone(),
                packages,
            };
            if dependencies.is_empty() {
                if !lock.packages.is_empty() {
                    return Err("빈 의존성 해석 결과에 패키지가 포함되었습니다.".to_string());
                }
            } else {
                validate_python_lock(dependencies, Some(&lock))?;
            }
            let wheel_paths = download_locked_wheels(&self.dirs, &lock.packages, cancellation)?;
            let final_path = project_env_root.join(&digest);
            if final_path.exists() {
                let (site_packages, cache_identity) = validate_python_environment_marker(
                    &self.dirs,
                    project_id,
                    dependencies,
                    &lock,
                    &final_path,
                )?;
                let marker = read_python_environment_marker(&final_path)?;
                probe_cached_environment(
                    project,
                    euddraft,
                    &project_env_root,
                    &site_packages,
                    &marker,
                    cancellation,
                )?;
                return Ok((lock, cache_identity));
            }
            let environment_staging =
                project_env_root.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
            fs::create_dir(&environment_staging).map_err(stringify_io)?;
            let environment_result = (|| {
                let site_packages = environment_staging.join(PYTHON_SITE_PACKAGES);
                fs::create_dir(&site_packages).map_err(stringify_io)?;
                if !wheel_paths.is_empty() {
                    let mut command = Command::new(uv);
                    configure_managed_uv_command(
                        &mut command,
                        &environment_staging,
                        &self.dirs.python_downloads_dir().join("uv-cache"),
                    );
                    command
                        .arg("--no-config")
                        .arg("--no-python-downloads")
                        .arg("pip")
                        .arg("install")
                        .arg("--quiet")
                        .arg("--no-index")
                        .arg("--no-deps")
                        .arg("--only-binary")
                        .arg(":all:")
                        .arg("--python-platform")
                        .arg("x86_64-pc-windows-msvc")
                        .arg("--python-version")
                        .arg(&python_version)
                        .arg("--target")
                        .arg(&site_packages);
                    for wheel in &wheel_paths {
                        command.arg(wheel);
                    }
                    run_managed_uv_command(command, PYTHON_INSTALL_TIMEOUT, cancellation)
                        .map_err(|error| format!("uv wheel 설치 실패: {error}"))?;
                }
                let site_packages_for_python =
                    fs::canonicalize(&site_packages).map_err(stringify_io)?;
                let probe_root = environment_staging.join("probe");
                fs::create_dir(&probe_root).map_err(stringify_io)?;
                let package_names = lock
                    .packages
                    .iter()
                    .map(|package| package.name.clone())
                    .collect::<Vec<_>>();
                let probe = probe_frozen_python(
                    project,
                    euddraft,
                    &probe_root,
                    Some(&site_packages_for_python),
                    &package_names,
                    cancellation,
                )
                .map_err(|error| {
                    format!(
                        "Python 의존성 호환성 검사 실패 ({}): {error}",
                        dependencies.join(", ")
                    )
                })?;
                fs::remove_dir_all(&probe_root).map_err(stringify_io)?;
                if probe.python_abi != lock.target.python_abi
                    || probe.euddraft_fingerprint != lock.target.euddraft_fingerprint
                {
                    return Err(
                        "설치된 Python 환경의 frozen euddraft 검사 지문이 잠금과 다릅니다."
                            .to_string(),
                    );
                }
                let files = collect_python_environment_files(&site_packages)?;
                let marker = PythonEnvironmentMarker {
                    schema: "eud-agent/python-environment/1".to_string(),
                    project_id: project_id.to_string(),
                    dependencies: dependencies.to_vec(),
                    lock: lock.clone(),
                    probe,
                    files,
                };
                let marker_bytes =
                    serde_json::to_vec(&marker).map_err(|error| error.to_string())?;
                crate::memory::write_atomic_bytes(
                    &environment_staging.join(PYTHON_ENV_MARKER),
                    &marker_bytes,
                )
                .map_err(|error| error.to_string())?;
                match fs::rename(&environment_staging, &final_path) {
                    Ok(()) => {}
                    Err(_) if final_path.exists() => {
                        fs::remove_dir_all(&environment_staging).map_err(stringify_io)?;
                    }
                    Err(error) => {
                        return Err(format!("Python 환경 캐시를 게시하지 못했습니다: {error}"))
                    }
                }
                let (_, cache_identity) = validate_python_environment_marker(
                    &self.dirs,
                    project_id,
                    dependencies,
                    &lock,
                    &final_path,
                )?;
                Ok(cache_identity)
            })();
            if environment_result.is_err() {
                let _ = fs::remove_dir_all(&environment_staging);
            }
            environment_result.map(|cache_identity| (lock, cache_identity))
        })();
        let _ = fs::remove_dir_all(&staging);
        result
    }

    pub fn build(&self, project_id: &str) -> Result<NativeBuildResult, String> {
        self.build_with_cancellation(project_id, None)
    }

    pub fn build_with_cancellation(
        &self,
        project_id: &str,
        cancellation: Option<&ProcessCancellation>,
    ) -> Result<NativeBuildResult, String> {
        if project_id.is_empty() {
            return Err("프로젝트 식별자가 비어 있습니다.".to_string());
        }
        let _transaction = self.transaction.lock();
        let project = self.open()?;
        let refusals = self.build_preflight(&project)?;
        if !refusals.is_empty() {
            return Ok(preflight_refusal(refusals));
        }
        let cache_key =
            python_project_cache_key(&fs::canonicalize(project.root()).map_err(stringify_io)?);
        let marker = project.root().join("build/.building");
        let _marker = BuildMarker::create(&marker)?;
        let config = self.dirs.load_config().map_err(|error| error.to_string())?;
        let configured = config.euddraft_path.trim();
        if configured.is_empty() {
            return Err(EUDDRAFT_NOT_CONFIGURED.to_string());
        }
        let launch = if project.has_direct_python()? {
            self.frozen_euddraft()?
        } else {
            EuddraftLaunch::resolve(Path::new(configured))?
        };
        let Some(lock) = project.manifest().python_lock.as_ref() else {
            return run_native_build_with_python_and_cancellation(
                &project,
                &self.dirs.native_assets_dir(),
                &launch,
                None,
                cancellation,
            );
        };
        validate_python_lock(&project.manifest().python_dependencies, Some(lock))?;
        let fingerprint = launch.frozen_fingerprint()?;
        if fingerprint != lock.target.euddraft_fingerprint {
            return Err("설정된 frozen euddraft.exe 지문이 Python 잠금과 다릅니다.".to_string());
        }
        let environment_path = self
            .dirs
            .python_envs_dir()
            .join(&cache_key)
            .join(&lock.digest);
        if !environment_path.is_dir() {
            let missing = lock
                .packages
                .iter()
                .map(|package| format!("{}=={}", package.name, package.version))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "검증된 Python 환경 캐시가 없습니다. python_dependencies_prepare를 실행하세요. 네트워크가 필요할 수 있습니다. 누락 패키지: {missing}"
            ));
        }
        let (site_packages, _) = validate_python_environment(
            &self.dirs,
            &cache_key,
            &project.manifest().python_dependencies,
            lock,
        )?;
        run_native_build_with_python_and_cancellation(
            &project,
            &self.dirs.native_assets_dir(),
            &launch,
            Some(&site_packages),
            cancellation,
        )
    }

    pub fn is_building(&self) -> bool {
        self.project_root()
            .ok()
            .is_some_and(|root| root.join("build/.building").is_file())
    }

    pub fn source_map_path(&self) -> Result<PathBuf, String> {
        self.open()?.source_map_path()
    }

    pub fn output_map_path(&self) -> Result<PathBuf, String> {
        self.open()?.output_map_path()
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn python_project_cache_key(project_root: &Path) -> String {
    let identity = project_root.to_string_lossy().replace('\\', "/");
    let identity = if cfg!(windows) {
        identity.to_lowercase()
    } else {
        identity
    };
    let mut hasher = Sha256::new();
    hasher.update(b"eud-agent/python-project-cache/v1\0");
    hasher.update(identity.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn python_input_digest(dependencies: &[String]) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hasher.update(b"eud-agent/python-dependency-input/v1\0");
    hasher.update(serde_json::to_vec(dependencies).map_err(|error| error.to_string())?);
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_cache_component(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || matches!(value, "." | "..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || value.ends_with(['.', ' '])
    {
        return Err(format!("{label}가 안전한 캐시 경로 구성요소가 아닙니다."));
    }
    Ok(())
}

fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x0000_0400 != 0
    }
    #[cfg(not(windows))]
    false
}

fn require_plain_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(stringify_io)?;
    if !metadata.file_type().is_dir() || metadata_is_reparse(&metadata) {
        return Err(format!(
            "일반 디렉터리가 아닌 Python 캐시 경로입니다: {}",
            path.display()
        ));
    }
    Ok(())
}

fn ensure_plain_directory_path(base: &Path, path: &Path) -> Result<(), String> {
    if !path.starts_with(base) {
        return Err("Python 캐시 경로가 LocalAppData 루트를 벗어났습니다.".to_string());
    }
    fs::create_dir_all(base).map_err(stringify_io)?;
    require_plain_directory(base)?;
    let mut current = base.to_path_buf();
    for component in path
        .strip_prefix(base)
        .map_err(|error| error.to_string())?
        .components()
    {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if !metadata.file_type().is_dir() || metadata_is_reparse(&metadata) {
                    return Err(format!(
                        "일반 디렉터리가 아닌 Python 캐시 경로입니다: {}",
                        current.display()
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(stringify_io)?;
                require_plain_directory(&current)?;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn read_python_environment_marker(path: &Path) -> Result<PythonEnvironmentMarker, String> {
    require_plain_directory(path)?;
    let marker_path = path.join(PYTHON_ENV_MARKER);
    let metadata = fs::symlink_metadata(&marker_path).map_err(stringify_io)?;
    if !metadata.file_type().is_file()
        || metadata_is_reparse(&metadata)
        || metadata.len() > MAX_PYLOCK_BYTES
    {
        return Err("Python 환경 완료 표식이 없거나 올바른 일반 파일이 아닙니다.".to_string());
    }
    serde_json::from_slice(&fs::read(marker_path).map_err(stringify_io)?)
        .map_err(|error| format!("Python 환경 완료 표식이 올바르지 않습니다: {error}"))
}

fn validate_python_environment(
    dirs: &DataDirs,
    project_id: &str,
    dependencies: &[String],
    lock: &PythonLock,
) -> Result<(PathBuf, String), String> {
    validate_cache_component(project_id, "프로젝트 ID")?;
    validate_python_lock(dependencies, Some(lock))?;
    let path = dirs.python_envs_dir().join(project_id).join(&lock.digest);
    validate_python_environment_marker(dirs, project_id, dependencies, lock, &path)
}

fn empty_python_environment_lock(
    dirs: &DataDirs,
    project_id: &str,
    lock_digest: &str,
) -> Result<(PythonLock, String), String> {
    let path = dirs.python_envs_dir().join(project_id).join(lock_digest);
    let marker = read_python_environment_marker(&path)?;
    if !marker.dependencies.is_empty()
        || !marker.lock.packages.is_empty()
        || marker.lock.digest != lock_digest
    {
        return Err("준비된 빈 Python 환경 표식이 후보와 다릅니다.".to_string());
    }
    let (_, cache_identity) =
        validate_python_environment_marker(dirs, project_id, &[], &marker.lock, &path)?;
    Ok((marker.lock, cache_identity))
}

fn validate_python_environment_marker(
    dirs: &DataDirs,
    project_id: &str,
    dependencies: &[String],
    lock: &PythonLock,
    path: &Path,
) -> Result<(PathBuf, String), String> {
    let expected_root = dirs.python_envs_dir().join(project_id);
    if path.parent() != Some(expected_root.as_path())
        || path.file_name().and_then(|value| value.to_str()) != Some(lock.digest.as_str())
    {
        return Err("Python 환경 캐시 경로가 잠금 지문과 다릅니다.".to_string());
    }
    require_plain_directory(&dirs.python_envs_dir())?;
    require_plain_directory(&expected_root)?;
    let marker = read_python_environment_marker(path)?;
    let expected_digest =
        compute_python_lock_digest(dependencies, &marker.lock.target, &marker.lock.packages)?;
    if marker.lock.digest != expected_digest
        || marker.lock.target.os != PYTHON_LOCK_OS
        || marker.lock.target.arch != PYTHON_LOCK_ARCH
        || marker.lock.target.uv_version != MANAGED_UV_VERSION
        || python_version_from_abi(&marker.lock.target.python_abi).is_err()
        || marker.lock.target.euddraft_fingerprint.len() != 64
        || !marker
            .lock
            .target
            .euddraft_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("Python 환경 완료 표식의 잠금 대상 또는 지문이 올바르지 않습니다.".to_string());
    }
    if marker.schema != "eud-agent/python-environment/1"
        || marker.project_id != project_id
        || marker.dependencies != dependencies
        || marker.lock != *lock
        || marker.probe.python_abi != lock.target.python_abi
        || marker.probe.euddraft_fingerprint != lock.target.euddraft_fingerprint
    {
        return Err("Python 환경 완료 표식이 현재 프로젝트 잠금과 다릅니다.".to_string());
    }
    let mut root_entries = fs::read_dir(path)
        .map_err(stringify_io)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(stringify_io)?;
    root_entries.sort_by_key(|entry| entry.file_name());
    if root_entries.len() != 2
        || !root_entries
            .iter()
            .any(|entry| entry.file_name().to_string_lossy() == PYTHON_ENV_MARKER)
        || !root_entries
            .iter()
            .any(|entry| entry.file_name().to_string_lossy() == PYTHON_SITE_PACKAGES)
    {
        return Err("Python 환경 캐시 루트에 완료 표식 밖의 항목이 있습니다.".to_string());
    }
    let site_packages = path.join(PYTHON_SITE_PACKAGES);
    require_plain_directory(&site_packages)?;
    let files = collect_python_environment_files(&site_packages)?;
    if files != marker.files {
        return Err("Python 환경 캐시 파일이 완료 표식 이후 변경되었습니다.".to_string());
    }
    let marker_bytes = fs::read(path.join(PYTHON_ENV_MARKER)).map_err(stringify_io)?;
    let site_packages = fs::canonicalize(site_packages).map_err(stringify_io)?;
    Ok((site_packages, sha256_bytes(&marker_bytes)))
}

fn collect_python_environment_files(root: &Path) -> Result<Vec<PythonEnvironmentFile>, String> {
    require_plain_directory(root)?;
    let canonical_root = fs::canonicalize(root).map_err(stringify_io)?;
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(stringify_io)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(stringify_io)?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(stringify_io)?;
            if metadata_is_reparse(&metadata) {
                return Err(format!(
                    "Python 환경에 링크 또는 리파스 지점이 있습니다: {}",
                    path.display()
                ));
            }
            if metadata.file_type().is_dir() {
                let canonical = fs::canonicalize(&path).map_err(stringify_io)?;
                if !canonical.starts_with(&canonical_root) {
                    return Err("Python 환경 디렉터리가 캐시 루트를 벗어났습니다.".to_string());
                }
                pending.push(path);
                continue;
            }
            if !metadata.file_type().is_file() {
                return Err(format!(
                    "Python 환경에 일반 파일이 아닌 항목이 있습니다: {}",
                    path.display()
                ));
            }
            if files.len() >= MAX_ENVIRONMENT_FILES {
                return Err("Python 환경 파일 수가 허용 범위를 초과했습니다.".to_string());
            }
            total_bytes = total_bytes
                .checked_add(metadata.len())
                .ok_or_else(|| "Python 환경 파일 크기가 넘쳤습니다.".to_string())?;
            if total_bytes > MAX_ENVIRONMENT_BYTES {
                return Err("Python 환경 전체 크기가 허용 범위를 초과했습니다.".to_string());
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            files.push(PythonEnvironmentFile {
                path: relative,
                bytes: metadata.len(),
                sha256: crate::bootstrap::sha256_file(&path).map_err(|error| {
                    format!("Python 환경 파일 해시를 계산하지 못했습니다: {error}")
                })?,
            });
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn probe_cached_environment(
    project: &NativeProject,
    euddraft: &EuddraftLaunch,
    project_env_root: &Path,
    site_packages: &Path,
    marker: &PythonEnvironmentMarker,
    cancellation: Option<&ProcessCancellation>,
) -> Result<(), String> {
    let probe_root =
        project_env_root.join(format!(".cache-probe-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir(&probe_root).map_err(stringify_io)?;
    let package_names = marker
        .lock
        .packages
        .iter()
        .map(|package| package.name.clone())
        .collect::<Vec<_>>();
    let result = probe_frozen_python(
        project,
        euddraft,
        &probe_root,
        Some(site_packages),
        &package_names,
        cancellation,
    );
    let _ = fs::remove_dir_all(&probe_root);
    let probe = result?;
    if probe != marker.probe {
        return Err(
            "캐시된 Python 환경의 frozen euddraft 검사 결과가 표식과 다릅니다.".to_string(),
        );
    }
    Ok(())
}

fn python_version_from_abi(abi: &str) -> Result<String, String> {
    let digits = abi
        .strip_prefix("cp")
        .ok_or_else(|| "frozen Python ABI가 CPython 형식이 아닙니다.".to_string())?;
    if !(2..=4).contains(&digits.len()) || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("frozen Python ABI 버전이 올바르지 않습니다.".to_string());
    }
    Ok(format!("{}.{}", &digits[..1], &digits[1..]))
}

fn configure_managed_uv_command(command: &mut Command, cwd: &Path, cache: &Path) {
    command.current_dir(cwd).env_clear();
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("UV_CACHE_DIR", cache)
        .env("UV_NO_CONFIG", "1")
        .env("UV_NO_PYTHON_DOWNLOADS", "1")
        .env("UV_KEYRING_PROVIDER", "disabled");
}

fn run_managed_uv<'a>(
    uv: &Path,
    cwd: &Path,
    cache: &Path,
    arguments: impl IntoIterator<Item = &'a str>,
    timeout: Duration,
    cancellation: Option<&ProcessCancellation>,
) -> Result<(), String> {
    ensure_plain_directory_path(cache.parent().unwrap_or(cache), cache)?;
    let mut command = Command::new(uv);
    configure_managed_uv_command(&mut command, cwd, cache);
    command.args(arguments);
    run_managed_uv_command(command, timeout, cancellation)
}

fn run_managed_uv_command(
    command: Command,
    timeout: Duration,
    cancellation: Option<&ProcessCancellation>,
) -> Result<(), String> {
    let output = crate::bootstrap::process_tree::run_process_tree(command, timeout, cancellation)?;
    match output.end {
        crate::bootstrap::process_tree::ProcessEnd::Exited(0) => Ok(()),
        crate::bootstrap::process_tree::ProcessEnd::Exited(code) => Err(format!(
            "관리형 uv 작업이 실패했습니다 (종료 코드 0x{code:08X}): {}",
            bounded_process_output(&output.stdout_lossy(), &output.stderr_lossy())
        )),
        crate::bootstrap::process_tree::ProcessEnd::TimedOut => Err(format!(
            "관리형 uv 작업이 {}초 안에 끝나지 않아 종료되었습니다.",
            timeout.as_secs()
        )),
        crate::bootstrap::process_tree::ProcessEnd::Cancelled => {
            Err("관리형 uv 작업이 취소되었습니다.".to_string())
        }
    }
}

fn bounded_process_output(stdout: &str, stderr: &str) -> String {
    let combined = match (stdout.trim(), stderr.trim()) {
        ("", "") => "출력 없음".to_string(),
        (stdout, "") => stdout.to_string(),
        ("", stderr) => stderr.to_string(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    };
    let mut chars = combined.chars().rev().take(8_000).collect::<Vec<_>>();
    chars.reverse();
    chars.into_iter().collect()
}

fn parse_pylock(text: &str) -> Result<Vec<PylockPackage>, String> {
    let lock: UvPylock = toml::from_str(text)
        .map_err(|error| format!("uv pylock.toml을 해석하지 못했습니다: {error}"))?;
    if lock.lock_version != "1.0" {
        return Err(format!(
            "지원하지 않는 pylock 버전입니다: {}",
            lock.lock_version
        ));
    }
    let mut packages = Vec::with_capacity(lock.packages.len());
    for package in lock.packages {
        if package.wheels.is_empty() {
            return Err(format!(
                "{}=={}에 wheel 배포 파일이 없습니다.",
                package.name, package.version
            ));
        }
        let mut wheels = Vec::with_capacity(package.wheels.len());
        for wheel in package.wheels {
            let path = wheel
                .url
                .strip_prefix("https://files.pythonhosted.org/packages/")
                .ok_or_else(|| {
                    format!("PyPI 이외의 wheel URL은 허용되지 않습니다: {}", wheel.url)
                })?;
            let filename = path
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty() && !name.contains(['?', '#', '\\']))
                .ok_or_else(|| "pylock wheel URL에 안전한 파일명이 없습니다.".to_string())?;
            if wheel.hashes.sha256.len() != 64
                || !wheel
                    .hashes
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err("pylock wheel SHA-256이 lowercase 64자리 hex가 아닙니다.".to_string());
            }
            wheels.push(PylockWheel {
                filename: filename.to_string(),
                url: wheel.url,
                sha256: wheel.hashes.sha256,
            });
        }
        packages.push(PylockPackage {
            name: package.name,
            version: package.version,
            wheels,
        });
    }
    Ok(packages)
}

fn select_locked_wheels(
    packages: &[PylockPackage],
    python_abi: &str,
) -> Result<Vec<PythonLockedPackage>, String> {
    let supported = supported_windows_tags(python_abi)?;
    let ranks = supported
        .iter()
        .enumerate()
        .map(|(index, tag)| (tag.as_str(), index))
        .collect::<HashMap<_, _>>();
    if packages.len() > MAX_RESOLVED_PACKAGES {
        return Err(format!(
            "해석된 Python 패키지가 최대 {MAX_RESOLVED_PACKAGES}개를 초과했습니다."
        ));
    }
    let mut selected = Vec::with_capacity(packages.len());
    let mut names = HashMap::new();
    for package in packages {
        let normalized =
            validate_python_dependencies(&[format!("{}=={}", package.name, package.version)])?;
        let (name, version) = normalized[0]
            .split_once("==")
            .expect("validated exact dependency");
        if name.len() > 128 || version.len() > 128 {
            return Err("해석된 Python 패키지 이름 또는 버전이 너무 깁니다.".to_string());
        }
        if names
            .insert(name.to_string(), version.to_string())
            .is_some()
        {
            return Err(format!("pylock에 중복 패키지가 있습니다: {name}"));
        }
        let mut choices = Vec::new();
        for wheel in &package.wheels {
            let tags = wheel_tags_from_filename(&wheel.filename)?;
            if let Some(rank) = tags.iter().filter_map(|tag| ranks.get(tag.as_str())).min() {
                choices.push((*rank, wheel.filename.as_str(), wheel, tags));
            }
        }
        choices.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
        let Some((_, _, wheel, tags)) = choices.into_iter().next() else {
            return Err(format!(
                "{}=={}에는 frozen Python {} / Windows x64와 호환되는 wheel이 없습니다.",
                name, version, python_abi
            ));
        };
        if wheel.filename.len() > 512 {
            return Err("해석된 wheel 파일명이 너무 깁니다.".to_string());
        }
        selected.push(PythonLockedPackage {
            name: name.to_string(),
            version: version.to_string(),
            wheel_filename: wheel.filename.clone(),
            wheel_tags: tags,
            artifact_url: wheel.url.clone(),
            sha256: wheel.sha256.clone(),
        });
    }
    selected.sort_by(|left, right| {
        (&left.name, &left.version, &left.wheel_filename).cmp(&(
            &right.name,
            &right.version,
            &right.wheel_filename,
        ))
    });
    Ok(selected)
}

fn wheel_tags_from_filename(filename: &str) -> Result<Vec<String>, String> {
    if filename.contains(['/', '\\']) || !filename.ends_with(".whl") {
        return Err(format!("올바르지 않은 wheel 파일명입니다: {filename}"));
    }
    let parts = filename
        .trim_end_matches(".whl")
        .split('-')
        .collect::<Vec<_>>();
    if parts.len() < 5 {
        return Err(format!("wheel 태그가 없는 파일명입니다: {filename}"));
    }
    let mut tags = Vec::new();
    for python in parts[parts.len() - 3].split('.') {
        for abi in parts[parts.len() - 2].split('.') {
            for platform in parts[parts.len() - 1].split('.') {
                if [python, abi, platform].iter().any(|part| {
                    part.is_empty()
                        || !part
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                }) {
                    return Err(format!("wheel 태그 형식이 올바르지 않습니다: {filename}"));
                }
                tags.push(format!("{python}-{abi}-{platform}"));
            }
        }
    }
    tags.sort();
    tags.dedup();
    Ok(tags)
}

fn supported_windows_tags(python_abi: &str) -> Result<Vec<String>, String> {
    let digits = python_abi
        .strip_prefix("cp")
        .ok_or_else(|| "CPython ABI 태그가 아닙니다.".to_string())?;
    if digits.len() < 2 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("CPython ABI 버전이 올바르지 않습니다.".to_string());
    }
    let major = digits[..1]
        .parse::<u32>()
        .map_err(|_| "CPython 주 버전이 올바르지 않습니다.".to_string())?;
    let minor = digits[1..]
        .parse::<u32>()
        .map_err(|_| "CPython 부 버전이 올바르지 않습니다.".to_string())?;
    if major != 3 {
        return Err("frozen euddraft의 CPython 3만 지원합니다.".to_string());
    }
    let mut tags = vec![
        format!("{python_abi}-{python_abi}-win_amd64"),
        format!("{python_abi}-abi3-win_amd64"),
        format!("{python_abi}-none-win_amd64"),
    ];
    for compatible_minor in (2..minor).rev() {
        tags.push(format!("cp3{compatible_minor}-abi3-win_amd64"));
    }
    tags.push(format!("py3{minor}-none-win_amd64"));
    tags.push("py3-none-win_amd64".to_string());
    for compatible_minor in (0..minor).rev() {
        tags.push(format!("py3{compatible_minor}-none-win_amd64"));
    }
    tags.push(format!("{python_abi}-none-any"));
    tags.push(format!("py3{minor}-none-any"));
    tags.push("py3-none-any".to_string());
    for compatible_minor in (0..minor).rev() {
        tags.push(format!("py3{compatible_minor}-none-any"));
    }
    Ok(tags)
}

async fn wait_for_process_cancellation(cancellation: Option<&ProcessCancellation>) {
    loop {
        if cancellation.is_some_and(ProcessCancellation::is_cancelled) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn download_locked_wheels(
    dirs: &DataDirs,
    packages: &[PythonLockedPackage],
    cancellation: Option<&ProcessCancellation>,
) -> Result<Vec<PathBuf>, String> {
    let root = dirs.python_downloads_dir();
    ensure_plain_directory_path(&root, &root)?;
    let requested = packages.to_vec();
    let cancellation = cancellation.cloned();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("wheel 다운로드 런타임을 만들지 못했습니다: {error}"))?;
        runtime.block_on(async move {
            let client = reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(300))
                .user_agent(concat!("eud-agent/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|error| {
                    format!("wheel 다운로드 클라이언트를 만들지 못했습니다: {error}")
                })?;
            let mut total = 0_u64;
            let mut paths = Vec::with_capacity(requested.len());
            for package in requested {
                if cancellation
                    .as_ref()
                    .is_some_and(ProcessCancellation::is_cancelled)
                {
                    return Err("wheel 다운로드가 취소되었습니다.".to_string());
                }
                let directory = root.join(&package.sha256);
                ensure_plain_directory_path(&root, &directory)?;
                let destination = directory.join(&package.wheel_filename);
                if destination.exists() {
                    let metadata = fs::symlink_metadata(&destination).map_err(stringify_io)?;
                    if !metadata.file_type().is_file()
                        || metadata_is_reparse(&metadata)
                        || crate::bootstrap::sha256_file(&destination)
                            .map_err(|error| error.to_string())?
                            != package.sha256
                    {
                        return Err(format!(
                            "캐시된 wheel이 손상되었습니다: {}",
                            destination.display()
                        ));
                    }
                    paths.push(destination);
                    continue;
                }
                let request = client.get(&package.artifact_url).send();
                tokio::pin!(request);
                let cancelled = wait_for_process_cancellation(cancellation.as_ref());
                tokio::pin!(cancelled);
                let mut response = tokio::select! {
                    response = &mut request => response.map_err(|error| {
                        format!("{} wheel을 다운로드하지 못했습니다: {error}", package.name)
                    })?,
                    () = &mut cancelled => return Err("wheel 다운로드가 취소되었습니다.".to_string()),
                };
                if !response.status().is_success() {
                    return Err(format!(
                        "{} wheel 다운로드가 HTTP {}로 실패했습니다.",
                        package.name,
                        response.status()
                    ));
                }
                if response.url().as_str() != package.artifact_url {
                    return Err(
                        "wheel 다운로드 URL이 잠금의 PyPI URL에서 변경되었습니다.".to_string()
                    );
                }
                if response
                    .content_length()
                    .is_some_and(|bytes| bytes > MAX_WHEEL_BYTES)
                {
                    return Err(format!(
                        "{} wheel이 허용 크기를 초과했습니다.",
                        package.name
                    ));
                }
                let mut bytes = Vec::new();
                loop {
                    let chunk = response.chunk();
                    tokio::pin!(chunk);
                    let cancelled = wait_for_process_cancellation(cancellation.as_ref());
                    tokio::pin!(cancelled);
                    let chunk = tokio::select! {
                        chunk = &mut chunk => chunk.map_err(|error| {
                            format!("{} wheel 다운로드가 중단되었습니다: {error}", package.name)
                        })?,
                        () = &mut cancelled => return Err("wheel 다운로드가 취소되었습니다.".to_string()),
                    };
                    let Some(chunk) = chunk else {
                        break;
                    };
                    let next_len =
                        u64::try_from(bytes.len().saturating_add(chunk.len())).unwrap_or(u64::MAX);
                    if next_len > MAX_WHEEL_BYTES
                        || total.saturating_add(next_len) > MAX_WHEEL_TOTAL_BYTES
                    {
                        return Err(
                            "wheel 다운로드 전체 크기가 허용 범위를 초과했습니다.".to_string()
                        );
                    }
                    bytes.extend_from_slice(&chunk);
                }
                let bytes_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                total = total.saturating_add(bytes_len);
                if sha256_bytes(&bytes) != package.sha256 {
                    return Err(format!(
                        "{} wheel SHA-256이 PyPI 잠금과 다릅니다.",
                        package.name
                    ));
                }
                let temporary = directory.join(format!(
                    ".{}.download-{}",
                    package.wheel_filename,
                    uuid::Uuid::new_v4().simple()
                ));
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)
                    .map_err(stringify_io)?;
                file.write_all(&bytes).map_err(stringify_io)?;
                file.flush().map_err(stringify_io)?;
                file.sync_all().map_err(stringify_io)?;
                match fs::rename(&temporary, &destination) {
                    Ok(()) => {}
                    Err(_) if destination.exists() => {
                        let _ = fs::remove_file(&temporary);
                        if crate::bootstrap::sha256_file(&destination)
                            .map_err(|error| error.to_string())?
                            != package.sha256
                        {
                            return Err("동시에 게시된 wheel 캐시가 잠금과 다릅니다.".to_string());
                        }
                    }
                    Err(error) => {
                        let _ = fs::remove_file(&temporary);
                        return Err(format!("wheel 캐시를 게시하지 못했습니다: {error}"));
                    }
                }
                paths.push(destination);
            }
            Ok(paths)
        })
    })
    .join()
    .map_err(|_| "wheel 다운로드 스레드가 비정상 종료되었습니다.".to_string())?
}

fn stringify_io(error: std::io::Error) -> String {
    error.to_string()
}

struct BuildMarker {
    path: PathBuf,
}

impl BuildMarker {
    fn create(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| format!("another native build is active: {error}"))?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

impl Drop for BuildMarker {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

/// A build that never started, carrying only why it was refused. It is a build
/// failure and not a tool error so the model reads it the way it reads a
/// compiler error, and so a repeated identical refusal counts as no progress.
fn preflight_refusal(errors: Vec<NativeBuildError>) -> NativeBuildResult {
    NativeBuildResult {
        ok: false,
        errors,
        warnings: Vec::new(),
        raw_status: 0,
        stdout: String::new(),
        stderr: String::new(),
        log_path: String::new(),
        artifacts: NativeBuildArtifacts::default(),
    }
}

fn preflight_error(file: impl Into<String>, message: String) -> NativeBuildError {
    let file = file.into();
    NativeBuildError {
        source: "preflight".to_string(),
        raw: format!("{file}: {message}"),
        file,
        line: 0,
        message,
        count: 1,
    }
}

fn scalar_text(value: &DatScalar) -> String {
    match value {
        DatScalar::Number(number) => number.to_string(),
        DatScalar::Text(text) => format!("\"{text}\""),
    }
}

pub(crate) fn baseline_value(
    catalog: &DatCatalog,
    target: &DatTarget,
) -> Result<DatScalar, String> {
    match target {
        DatTarget::Dat {
            dat,
            object_id,
            field,
        } => catalog
            .numeric_value(dat, *object_id, field)
            .map(DatScalar::Number),
        DatTarget::Xdat {
            dat,
            object_id,
            field,
        } => catalog
            .xdat_value(dat, *object_id, field)
            .map(DatScalar::Number),
        DatTarget::Tbl(index) => catalog
            .tbl_value(*index)
            .map(|value| DatScalar::Text(value.to_string())),
        DatTarget::Requirement { dat, object_id } => catalog
            .requirement_payload(dat, *object_id)
            .map(DatScalar::Text),
        DatTarget::Button(set_id) => catalog.button_csv(*set_id).map(DatScalar::Text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::native_build::sync_compat_assets;
    use crate::native_project::{NativeDatChange, ProjectSettings};
    use std::fs;

    fn roots(tag: &str) -> (PathBuf, DataDirs) {
        let base = std::env::temp_dir().join(format!(
            "eud-agent-native-runtime-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        sync_compat_assets(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/eud-editor-compat"),
            &dirs.native_assets_dir(),
        )
        .unwrap();
        (base, dirs)
    }

    fn manager(tag: &str) -> (PathBuf, NativeProjectManager) {
        let (base, dirs) = roots(tag);
        let project_root = base.join("project");
        let manager = NativeProjectManager::new(dirs.clone());
        manager
            .create_project(
                &project_root,
                ProjectManifest {
                    schema_version: crate::native_project::PROJECT_SCHEMA_VERSION,
                    name: "Runtime Demo".to_string(),
                    source_map: "maps/source.scx".to_string(),
                    output_map: "build/output.scx".to_string(),
                    main_file: "src/main.eps".to_string(),
                    settings: ProjectSettings::default(),
                    plugins: Vec::new(),
                    python_entrypoints: Vec::new(),
                    python_dependencies: Vec::new(),
                    python_lock: None,
                    editor_compatibility: None,
                },
            )
            .unwrap();
        fs::create_dir_all(project_root.join("maps")).unwrap();
        fs::write(project_root.join("maps/source.scx"), b"map").unwrap();
        manager
            .write_source("src/main.eps", "function onPluginStart() {}\n")
            .unwrap();
        let mut config = dirs.load_config().unwrap_or_else(|_| Config::default());
        config.project_path = project_root.to_string_lossy().into_owned();
        dirs.save_config(&config).unwrap();
        (base, manager)
    }

    /// Free CRUD lets a model edit `dat/*.json` without passing `dat_patch`.
    /// The generator emits `after - before` as a runtime delta, so a wrong
    /// `before` would not fail anything — it would silently run the map on the
    /// wrong numbers. The build has to be the one that says no.
    #[test]
    fn a_hand_edited_dat_override_with_a_wrong_before_fails_the_build() {
        let (base, manager) = manager("preflight-dat");
        manager
            .apply_dat_patch(&NativeDatPatch {
                changes: vec![NativeDatChange::Dat {
                    dat: "units".to_string(),
                    object_id: 15,
                    field: "Hit Points".to_string(),
                    before: 10240,
                    after: 20480,
                }],
            })
            .unwrap();
        let document = manager.project_root().unwrap().join("dat/standard.json");
        let text = fs::read_to_string(&document).unwrap();
        fs::write(&document, text.replace("10240", "1")).unwrap();

        let result = manager.build("project").unwrap();

        assert!(!result.ok);
        let error = result
            .errors
            .iter()
            .find(|error| error.file == "dat/standard.json")
            .unwrap_or_else(|| panic!("a preflight error names the document: {:?}", result.errors));
        assert_eq!(error.source, "preflight");
        assert!(error.message.contains("before"), "{}", error.message);
        // The stock value and the claimed one are both named, so the model can
        // repair the file without another read.
        assert!(error.message.contains("10240"), "{}", error.message);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn a_source_file_the_project_cannot_use_fails_the_build_instead_of_being_skipped() {
        let (base, manager) = manager("preflight-source");
        let root = manager.project_root().unwrap();
        // `[`/`]` cannot appear in an EDS section header, so every source
        // listing silently drops this file. Silence is the wrong answer once
        // the model can create it directly.
        fs::write(root.join("src/bad[1].eps"), "const a = 1;").unwrap();

        let result = manager.build("project").unwrap();

        assert!(!result.ok);
        let error = result
            .errors
            .iter()
            .find(|error| error.file.contains("bad[1].eps"))
            .unwrap_or_else(|| panic!("a preflight error names the file: {:?}", result.errors));
        assert_eq!(error.source, "preflight");
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn a_correct_project_passes_the_preflight_and_reaches_the_compiler() {
        let (base, manager) = manager("preflight-clean");
        manager
            .apply_dat_patch(&NativeDatPatch {
                changes: vec![NativeDatChange::Dat {
                    dat: "units".to_string(),
                    object_id: 15,
                    field: "Hit Points".to_string(),
                    before: 10240,
                    after: 20480,
                }],
            })
            .unwrap();

        // No euddraft is configured in a test, so reaching that refusal is what
        // proves the preflight let the build through.
        let error = manager.build("project").unwrap_err();

        assert_eq!(error, EUDDRAFT_NOT_CONFIGURED);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn opening_a_project_gives_it_the_repository_the_app_rolls_back_with() {
        if !crate::git::available() {
            eprintln!("skipping: git is not installed on this machine");
            return;
        }
        let (base, manager) = manager("git-prepare");
        let root = manager.project_root().unwrap();

        // Activation prepared the repository, so a turn boundary may commit.
        assert!(crate::git::auto_commit_ready(&root));
        assert!(root.join(".gitignore").is_file());
        assert_eq!(crate::git::log(&root, 10).unwrap().len(), 1);

        // Everything written after the open is work a boundary still records.
        let record = crate::git::commit_turn(&root, "초기 소스")
            .unwrap()
            .expect("a commit");
        assert!(record.files >= 1);
        assert!(crate::git::dirty_paths(&root).unwrap().is_empty());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn native_harness_migration_is_local_portable_and_never_replays_old_data() {
        use sha2::{Digest, Sha256};
        let (base, manager) = manager("local-harness");
        let project = manager.open().unwrap();
        let dirs = manager.data_dirs();
        fs::remove_file(
            project
                .root()
                .join(".eud-agent/state/appdata-migration.json"),
        )
        .unwrap();
        let id = format!(
            "{:x}",
            Sha256::digest(project.root().to_string_lossy().as_bytes())
        );
        let old_root = dirs.workspaces_dir().join(&id);
        fs::create_dir_all(old_root.join("specs")).unwrap();
        fs::write(old_root.join("specs/kept.md"), "승인된 동작").unwrap();
        let state = serde_json::to_vec(&serde_json::json!({
            "version": 1, "id": id, "identityHash": id, "project": project.manifest().name,
            "documents": {
                "specs/kept.md": {"revision": 1, "state": "accepted", "acceptedAt": 1, "requestId": "legacy"}
            },
            "approvedPlans": {},
        })).unwrap();
        let old_state = dirs
            .workspace_state_dir()
            .join("projects")
            .join(format!("{id}.json"));
        fs::create_dir_all(old_state.parent().unwrap()).unwrap();
        fs::write(&old_state, &state).unwrap();
        let old_memory =
            crate::memory::ProjectMemory::new(dirs.memory_dir(), &project.manifest().name);
        assert!(old_memory.write("resources", "Switch 9 = phase").ok);

        assert!(manager
            .configure_project(project.root())
            .unwrap()
            .is_empty());
        let workspaces = crate::workspace::WorkspaceManager::new(dirs.clone());
        let prepared = workspaces.prepare_current().unwrap();
        assert_eq!(
            workspaces
                .read_file(&prepared.id, ".eud-agent/workspace/specs/kept.md")
                .unwrap()
                .content
                .as_deref(),
            Some("승인된 동작")
        );
        let memory = crate::memory::ProjectMemory::current(dirs).unwrap();
        assert_eq!(memory.read("resources"), "Switch 9 = phase");
        assert_eq!(fs::read(&old_state).unwrap(), state);
        assert_eq!(
            fs::read_to_string(old_root.join("specs/kept.md")).unwrap(),
            "승인된 동작"
        );
        assert_eq!(old_memory.read("resources"), "Switch 9 = phase");

        let moved_root = base.join("moved-project");
        fs::rename(project.root(), &moved_root).unwrap();
        manager.configure_project(&moved_root).unwrap();
        let moved = workspaces.prepare_current().unwrap();
        assert_eq!(moved.id, prepared.id);
        assert_eq!(
            workspaces
                .read_file(&moved.id, ".eud-agent/workspace/specs/kept.md")
                .unwrap()
                .content
                .as_deref(),
            Some("승인된 동작")
        );
        let moved_memory = crate::memory::ProjectMemory::current(dirs).unwrap();
        assert_eq!(moved_memory.read("resources"), "Switch 9 = phase");
        fs::remove_file(moved_memory.store_dir().unwrap().join("resources.md")).unwrap();
        manager.configure_project(&moved_root).unwrap();
        assert_eq!(
            crate::memory::ProjectMemory::current(dirs)
                .unwrap()
                .read("resources"),
            ""
        );
        assert_eq!(old_memory.read("resources"), "Switch 9 = phase");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn native_open_reports_unbound_name_memory_without_blocking_project() {
        let (base, manager) = manager("unbound-memory");
        let project = manager.open().unwrap();
        fs::remove_file(
            project
                .root()
                .join(".eud-agent/state/appdata-migration.json"),
        )
        .unwrap();
        let old_memory = crate::memory::ProjectMemory::new(
            manager.data_dirs().memory_dir(),
            &project.manifest().name,
        );
        assert!(
            old_memory
                .write("resources", "다른 프로젝트일 수 있는 자료")
                .ok
        );
        let issues = manager.configure_project(project.root()).unwrap();
        assert!(issues.iter().any(|issue| issue.scope == "memory"));
        assert_eq!(manager.open().unwrap().root(), project.root());
        assert_eq!(
            crate::memory::ProjectMemory::current(manager.data_dirs())
                .unwrap()
                .read("resources"),
            ""
        );
        assert_eq!(old_memory.read("resources"), "다른 프로젝트일 수 있는 자료");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[ignore = "requires EUD_AGENT_EUDDRAFT, EUD_AGENT_BUILD_MAP, and network"]
    fn real_python_dependencies_prepare_probe_commit_build_and_reject_crash() {
        #[derive(Default)]
        struct SilentEmitter;
        impl crate::bootstrap::ProgressEmitter for SilentEmitter {
            fn emit(&self, _stage: &str, _pct: u8, _detail: &str) {}
        }

        let (base, manager) = manager("pydep");
        let root = manager.project_root().unwrap();
        fs::copy(
            std::env::var("EUD_AGENT_BUILD_MAP").unwrap(),
            root.join("maps/source.scx"),
        )
        .unwrap();
        let mut config = manager.data_dirs().load_config().unwrap();
        config.euddraft_path = std::env::var("EUD_AGENT_EUDDRAFT").unwrap();
        manager.data_dirs().save_config(&config).unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(crate::bootstrap::ensure_managed_uv(
                manager.data_dirs(),
                &SilentEmitter,
            ))
            .unwrap();

        manager
            .create_source("src/direct.py", "import six\n")
            .unwrap();
        let mut manifest = manager.open().unwrap().manifest().clone();
        manifest.python_entrypoints = vec!["src/direct.py".to_string()];
        let canonical_manifest = root.join(PROJECT_MANIFEST_FILE);
        crate::memory::write_atomic_bytes(
            &canonical_manifest,
            &serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let manifest_path = root.join("custom.eap");
        fs::rename(&canonical_manifest, &manifest_path).unwrap();

        let project_id = "b".repeat(64);
        let (prepared, concurrent) = std::thread::scope(|scope| {
            let first_manager = manager.clone();
            let first_project = project_id.clone();
            let first = scope.spawn(move || {
                first_manager.prepare_python_dependencies(
                    "real-python-session",
                    &first_project,
                    vec!["six==1.17.0".to_string()],
                )
            });
            let second_manager = manager.clone();
            let second_project = project_id.clone();
            let second = scope.spawn(move || {
                second_manager.prepare_python_dependencies(
                    "real-python-session",
                    &second_project,
                    vec!["six==1.17.0".to_string()],
                )
            });
            (
                first.join().unwrap().unwrap(),
                second.join().unwrap().unwrap(),
            )
        });
        assert_eq!(prepared.normalized_dependencies, vec!["six==1.17.0"]);
        assert_eq!(prepared.lock_digest, concurrent.lock_digest);
        assert_eq!(prepared.cache_identity, concurrent.cache_identity);
        assert_eq!(
            prepared.candidate_token, concurrent.candidate_token,
            "identical normalized dependency input must reuse the active prepared candidate"
        );
        assert_eq!(prepared.input_digest, concurrent.input_digest);
        let cache_key = python_project_cache_key(&fs::canonicalize(&root).unwrap());
        let completed = fs::read_dir(manager.data_dirs().python_envs_dir().join(&cache_key))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.path().is_dir() && entry.file_name().to_string_lossy().len() == 64
            })
            .count();
        assert_eq!(completed, 1);
        let stale = manager
            .claim_python_dependencies(
                &concurrent.candidate_token,
                "real-python-session",
                &project_id,
            )
            .unwrap();
        manager
            .write_source("src/direct.py", "import six\n# changed after prepare\n")
            .unwrap();
        assert!(manager
            .commit_python_dependencies(stale, "real-python-session", &project_id)
            .unwrap_err()
            .contains("변경"));
        let prepared = manager
            .prepare_python_dependencies(
                "real-python-session",
                &project_id,
                vec!["six==1.17.0".to_string()],
            )
            .unwrap();
        assert_ne!(
            prepared.candidate_token, concurrent.candidate_token,
            "a changed project revision must not reuse a stale candidate"
        );
        let claimed = manager
            .claim_python_dependencies(
                &prepared.candidate_token,
                "real-python-session",
                &project_id,
            )
            .unwrap();
        manager
            .commit_python_dependencies(claimed, "real-python-session", &project_id)
            .unwrap();
        let result = manager.build(&project_id).unwrap();
        assert!(result.ok, "{:?}", result.errors);
        fs::remove_dir_all(manager.data_dirs().python_downloads_dir()).unwrap();
        assert!(
            manager.build(&project_id).unwrap().ok,
            "a verified environment must build without the wheel/download cache"
        );
        let committed_manifest = fs::read(manifest_path.clone()).unwrap();
        let committed = manager.open().unwrap();
        let committed_lock = committed.manifest().python_lock.as_ref().unwrap();
        let cache_key = python_project_cache_key(&fs::canonicalize(committed.root()).unwrap());
        let six_module = manager
            .data_dirs()
            .python_envs_dir()
            .join(cache_key)
            .join(&committed_lock.digest)
            .join(PYTHON_SITE_PACKAGES)
            .join("six.py");
        fs::write(&six_module, b"corrupt\n").unwrap();
        assert!(manager.build(&project_id).is_err());
        assert_eq!(
            fs::read(manifest_path.clone()).unwrap(),
            committed_manifest,
            "cache corruption must not mutate canonical dependency state"
        );

        manager
            .write_source("src/direct.py", "import regex\n")
            .unwrap();
        let regex = manager
            .prepare_python_dependencies(
                "real-python-session",
                &project_id,
                vec!["regex==2024.11.6".to_string()],
            )
            .unwrap();
        let claimed = manager
            .claim_python_dependencies(&regex.candidate_token, "real-python-session", &project_id)
            .unwrap();
        manager
            .commit_python_dependencies(claimed, "real-python-session", &project_id)
            .unwrap();
        assert!(manager.build(&project_id).unwrap().ok);
        let stable_manifest = fs::read(manifest_path.clone()).unwrap();
        let regex_project = manager.open().unwrap();
        let regex_lock = regex_project
            .manifest()
            .python_lock
            .as_ref()
            .unwrap()
            .clone();
        let cache_key = python_project_cache_key(&fs::canonicalize(regex_project.root()).unwrap());
        let (regex_site, regex_identity) = validate_python_environment(
            manager.data_dirs(),
            &cache_key,
            &regex_project.manifest().python_dependencies,
            &regex_lock,
        )
        .unwrap();
        let regex_environment = regex_site.parent().unwrap().to_path_buf();
        let offline_missing = regex_environment.with_extension("offline-missing");
        fs::rename(&regex_environment, &offline_missing).unwrap();
        let missing_error = manager.build(&project_id).unwrap_err();
        assert!(missing_error.contains("네트워크"), "got: {missing_error}");
        assert!(
            missing_error.contains("regex==2024.11.6"),
            "got: {missing_error}"
        );
        assert_eq!(fs::read(manifest_path.clone()).unwrap(), stable_manifest);
        fs::rename(&offline_missing, &regex_environment).unwrap();

        manager.write_source("src/direct.py", "pass\n").unwrap();
        let error = manager
            .prepare_python_dependencies(
                "real-python-session",
                &project_id,
                vec!["orjson==3.10.18".to_string()],
            )
            .unwrap_err();
        assert!(
            error.contains("orjson") || error.contains("0xC0000005"),
            "got: {error}"
        );
        let current: serde_json::Value =
            serde_json::from_slice(&fs::read(manifest_path.clone()).unwrap()).unwrap();
        let stable: serde_json::Value = serde_json::from_slice(&stable_manifest).unwrap();
        assert_eq!(current["pythonDependencies"], stable["pythonDependencies"]);
        assert_eq!(current["pythonLock"], stable["pythonLock"]);
        let regex_project = manager.open().unwrap();
        let (_, identity_after_crash) = validate_python_environment(
            manager.data_dirs(),
            &cache_key,
            &regex_project.manifest().python_dependencies,
            &regex_lock,
        )
        .unwrap();
        assert_eq!(identity_after_crash, regex_identity);
        assert!(manager.build(&project_id).unwrap().ok);
        let cache_root = manager.data_dirs().python_envs_dir().join(&cache_key);
        assert!(!fs::read_dir(cache_root)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with(".pending-") || name.starts_with(".staging-")
            }));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn reads_stock_baseline_and_applies_one_batch_patch() {
        let (base, manager) = manager("dat");
        let target = DatTarget::Dat {
            dat: "units".to_string(),
            object_id: 15,
            field: "Hit Points".to_string(),
        };
        let values = manager.dat_values(std::slice::from_ref(&target)).unwrap();
        assert_eq!(values[&target], DatScalar::Number(10240));
        let revision = manager
            .apply_dat_patch(&NativeDatPatch {
                changes: vec![NativeDatChange::Dat {
                    dat: "units".to_string(),
                    object_id: 15,
                    field: "Hit Points".to_string(),
                    before: 10240,
                    after: 20480,
                }],
            })
            .unwrap();
        assert_eq!(revision.len(), 64);
        assert_eq!(
            manager.dat_values(std::slice::from_ref(&target)).unwrap()[&target],
            DatScalar::Number(20480)
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn malformed_manifest_refuses_reconfigure_and_preserves_config() {
        let (base, manager) = manager("bad-manifest");
        let before = manager.data_dirs().load_config().unwrap().project_path;
        let root = PathBuf::from(&before);
        fs::write(
            root.join(crate::native_project::PROJECT_MANIFEST_FILE),
            br#"{"schemaVersion":2,"name":"broken"}"#,
        )
        .unwrap();

        assert!(manager.configure_project(&root).is_err());
        assert_eq!(
            manager.data_dirs().load_config().unwrap().project_path,
            before
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn ambiguous_manifests_are_rejected() {
        let (base, manager) = manager("ambiguous-manifest");
        let root = PathBuf::from(manager.data_dirs().load_config().unwrap().project_path);
        fs::write(
            root.join("alternate.eap"),
            fs::read(root.join(crate::native_project::PROJECT_MANIFEST_FILE)).unwrap(),
        )
        .unwrap();
        assert!(manager.configure_project(&root).is_err());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn explicit_legacy_open_migrates_only_after_validation() {
        let (base, _dirs) = roots("legacy-migration");
        let root = base.join("legacy");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.eps"), b"function onPluginStart() {}\n").unwrap();
        let manifest = serde_json::json!({
            "schemaVersion": 1,
            "name": "Legacy",
            "sourceMap": "maps/source.scx",
            "outputMap": "build/output.scx",
            "mainFile": "src/main.eps",
            "settings": ProjectSettings::default(),
            "plugins": [],
        });
        fs::create_dir_all(root.join("maps")).unwrap();
        fs::write(root.join("maps/source.scx"), b"map").unwrap();
        fs::write(
            root.join(crate::native_project::LEGACY_PROJECT_MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("custom.eudproj"),
            br#"{"schemaVersion":1,"manifest":"project.json"}"#,
        )
        .unwrap();
        let project = open_project_path(&root).unwrap();
        assert_eq!(project.manifest().name, "Legacy");
        assert!(root
            .join(crate::native_project::PROJECT_MANIFEST_FILE)
            .is_file());
        assert!(!root
            .join(crate::native_project::LEGACY_PROJECT_MANIFEST_FILE)
            .exists());
        assert!(!root.join("custom.eudproj").exists());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn invalid_legacy_open_does_not_create_canonical_manifest() {
        let (base, _dirs) = roots("legacy-invalid");
        let root = base.join("legacy");
        fs::create_dir_all(&root).unwrap();
        let legacy = root.join(crate::native_project::LEGACY_PROJECT_MANIFEST_FILE);
        fs::write(&legacy, br#"{"schemaVersion":1,"name":"broken"}"#).unwrap();
        assert!(open_project_path(&legacy).is_err());
        assert!(legacy.is_file());
        assert!(!root
            .join(crate::native_project::PROJECT_MANIFEST_FILE)
            .exists());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn legacy_and_canonical_authorities_conflict_without_overwrite() {
        let (base, manager) = manager("legacy-conflict");
        let root = PathBuf::from(manager.data_dirs().load_config().unwrap().project_path);
        let legacy = root.join(crate::native_project::LEGACY_PROJECT_MANIFEST_FILE);
        fs::write(
            &legacy,
            fs::read(root.join(crate::native_project::PROJECT_MANIFEST_FILE)).unwrap(),
        )
        .unwrap();
        let canonical_before =
            fs::read(root.join(crate::native_project::PROJECT_MANIFEST_FILE)).unwrap();
        assert!(open_project_path(&root).is_err());
        assert_eq!(
            fs::read(root.join(crate::native_project::PROJECT_MANIFEST_FILE)).unwrap(),
            canonical_before
        );
        assert!(legacy.is_file());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn cancelled_dependency_prepare_does_not_wait_for_global_lock() {
        let (base, manager) = manager("cancelled-prepare-lock");
        let _held = PYTHON_PREPARATION_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock();
        let cancellation = ProcessCancellation::default();
        cancellation.cancel();
        let started = std::time::Instant::now();
        let error = manager
            .prepare_python_dependencies_with_cancellation(
                "cancelled-session",
                "project",
                Vec::new(),
                Some(&cancellation),
            )
            .unwrap_err();
        assert!(error.contains("취소"));
        assert!(started.elapsed() < Duration::from_secs(1));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn managed_uv_command_drops_ambient_resolver_configuration() {
        let mut command = Command::new("uv.exe");
        command
            .env("UV_CONSTRAINT", "hostile.txt")
            .env("UV_OVERRIDE", "override.txt")
            .env("PIP_INDEX_URL", "https://packages.example/simple");
        let cwd = std::env::temp_dir();
        let cache = cwd.join("eud-agent-uv-cache-test");
        configure_managed_uv_command(&mut command, &cwd, &cache);
        let configured = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_os_string()),
                )
            })
            .collect::<HashMap<_, _>>();
        assert!(!configured.contains_key("UV_CONSTRAINT"));
        assert!(!configured.contains_key("UV_OVERRIDE"));
        assert!(!configured.contains_key("PIP_INDEX_URL"));
        assert_eq!(
            configured
                .get("UV_CACHE_DIR")
                .and_then(Option::as_ref)
                .map(|value| value.as_os_str()),
            Some(cache.as_os_str())
        );
    }

    #[test]
    fn pylock_parser_selects_one_deterministic_compatible_wheel() {
        let pylock = r#"
lock-version = "1.0"
created-by = "uv"

[[packages]]
name = "Example_Pkg"
version = "1.02"
wheels = [
  { url = "https://files.pythonhosted.org/packages/aa/example_pkg-1.2-py3-none-any.whl", hashes = { sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" } },
  { url = "https://files.pythonhosted.org/packages/bb/example_pkg-1.2-cp311-cp311-win_amd64.whl", hashes = { sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" } },
]
"#;
        let parsed = parse_pylock(pylock).unwrap();
        let selected = select_locked_wheels(&parsed, "cp311").unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "example-pkg");
        assert_eq!(selected[0].version, "1.2");
        assert_eq!(
            selected[0].wheel_filename,
            "example_pkg-1.2-cp311-cp311-win_amd64.whl"
        );
        assert_eq!(selected[0].wheel_tags, vec!["cp311-cp311-win_amd64"]);
    }

    #[test]
    fn pylock_parser_rejects_non_pypi_and_sdist_only_records() {
        let alternate = r#"
lock-version = "1.0"
[[packages]]
name = "demo"
version = "1.0"
wheels = [{ url = "https://packages.example/demo-1.0-py3-none-any.whl", hashes = { sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" } }]
"#;
        assert!(parse_pylock(alternate).unwrap_err().contains("PyPI"));

        let sdist_only = r#"

lock-version = "1.0"
[[packages]]
name = "demo"
version = "1.0"
sdist = { url = "https://files.pythonhosted.org/packages/demo-1.0.tar.gz", hashes = { sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" } }
"#;
        assert!(parse_pylock(sdist_only).unwrap_err().contains("wheel"));
    }
    #[test]
    fn dependency_input_digest_covers_the_complete_normalized_set() {
        let a = validate_python_dependencies(&[
            "Requests==2.32.0".to_string(),
            "typing_extensions==4.12.2".to_string(),
        ])
        .unwrap();
        let same = validate_python_dependencies(&[
            "requests==2.32.0".to_string(),
            "typing-extensions==4.12.2".to_string(),
        ])
        .unwrap();
        let different = validate_python_dependencies(&[
            "requests==2.32.1".to_string(),
            "typing-extensions==4.12.2".to_string(),
        ])
        .unwrap();

        assert_eq!(
            python_input_digest(&a).unwrap(),
            python_input_digest(&same).unwrap()
        );
        assert_ne!(
            python_input_digest(&a).unwrap(),
            python_input_digest(&different).unwrap()
        );
    }

    #[test]
    fn dependency_candidate_is_owner_bound_expiring_and_single_use() {
        let (base, manager) = manager("python-candidate");
        let root = fs::canonicalize(manager.project_root().unwrap()).unwrap();
        let candidate = PythonDependencyCandidate {
            session_id: "session-a".to_string(),
            project_id: "project-a".to_string(),
            project_root: root,
            cache_key: "0".repeat(64),
            base_revision: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
            input_digest: "c".repeat(64),
            cache_identity: "d".repeat(64),
            euddraft_fingerprint: "f".repeat(64),
            python_abi: "cp311".to_string(),
            expires_at: unix_millis().saturating_add(60_000),
            dependencies: Vec::new(),
            python_lock: None,
            lock_digest: "e".repeat(64),
        };
        let token = format!("pydep-{}", uuid::Uuid::new_v4().simple());
        manager
            .python_candidates
            .lock()
            .insert(token.clone(), candidate.clone());
        assert!(manager
            .claim_python_dependencies(&token, "session-b", "project-a")
            .unwrap_err()
            .contains("세션"));
        assert!(manager
            .claim_python_dependencies(&token, "session-a", "project-b")
            .unwrap_err()
            .contains("프로젝트"));
        let expired_token = format!("pydep-{}", uuid::Uuid::new_v4().simple());
        let mut expired = candidate.clone();
        expired.expires_at = 0;
        manager
            .python_candidates
            .lock()
            .insert(expired_token.clone(), expired);
        assert!(manager
            .claim_python_dependencies(&expired_token, "session-a", "project-a")
            .unwrap_err()
            .contains("만료"));
        assert!(manager
            .claim_python_dependencies(&expired_token, "session-a", "project-a")
            .is_err());
        manager
            .claim_python_dependencies(&token, "session-a", "project-a")
            .unwrap();
        assert!(manager
            .claim_python_dependencies(&token, "session-a", "project-a")
            .unwrap_err()
            .contains("이미 사용"));
        let revoked = format!("pydep-{}", uuid::Uuid::new_v4().simple());
        manager
            .python_candidates
            .lock()
            .insert(revoked.clone(), candidate);
        manager.revoke_python_dependency_candidates("session-a");
        assert!(manager
            .claim_python_dependencies(&revoked, "session-a", "project-a")
            .is_err());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn environment_marker_detects_exact_file_corruption() {
        let (base, dirs) = roots("python-environment-marker");
        let project_id = "project-a";
        let target = PythonLockTarget {
            os: PYTHON_LOCK_OS.to_string(),
            arch: PYTHON_LOCK_ARCH.to_string(),
            python_abi: "cp311".to_string(),
            euddraft_fingerprint: "a".repeat(64),
            uv_version: MANAGED_UV_VERSION.to_string(),
        };
        let digest = compute_python_lock_digest(&[], &target, &[]).unwrap();
        let lock = PythonLock {
            digest: digest.clone(),
            target: target.clone(),
            packages: Vec::new(),
        };
        let environment = dirs.python_envs_dir().join(project_id).join(&digest);
        let site_packages = environment.join(PYTHON_SITE_PACKAGES);
        fs::create_dir_all(&site_packages).unwrap();
        fs::write(site_packages.join("module.py"), b"VALUE = 1\n").unwrap();
        let marker = PythonEnvironmentMarker {
            schema: "eud-agent/python-environment/1".to_string(),
            project_id: project_id.to_string(),
            dependencies: Vec::new(),
            lock: lock.clone(),
            probe: FrozenPythonIdentity {
                python_abi: target.python_abi,
                euddraft_fingerprint: target.euddraft_fingerprint,
            },
            files: collect_python_environment_files(&site_packages).unwrap(),
        };
        fs::write(
            environment.join(PYTHON_ENV_MARKER),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        validate_python_environment_marker(&dirs, project_id, &[], &lock, &environment).unwrap();
        fs::write(site_packages.join("module.py"), b"VALUE = 2\n").unwrap();
        assert!(
            validate_python_environment_marker(&dirs, project_id, &[], &lock, &environment)
                .unwrap_err()
                .contains("변경")
        );
        fs::remove_dir_all(base).ok();
    }

    #[cfg(windows)]
    #[test]
    fn locked_legacy_descriptor_rolls_back_without_losing_manifest() {
        use std::os::windows::fs::OpenOptionsExt;

        let (base, manager) = manager("legacy-delete-lock");
        let root = manager.project_root().unwrap();
        let canonical = root.join(crate::native_project::PROJECT_MANIFEST_FILE);
        let legacy = root.join(crate::native_project::LEGACY_PROJECT_MANIFEST_FILE);
        let original: ProjectManifest =
            serde_json::from_slice(&fs::read(&canonical).unwrap()).unwrap();
        let legacy_manifest = serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": 1,
            "name": original.name.clone(),
            "sourceMap": original.source_map.clone(),
            "outputMap": original.output_map.clone(),
            "mainFile": original.main_file.clone(),
            "settings": original.settings.clone(),
            "plugins": original.plugins.clone(),
        }))
        .unwrap();
        fs::remove_file(&canonical).unwrap();
        fs::write(&legacy, &legacy_manifest).unwrap();
        let descriptor = root.join(LEGACY_PROJECT_DESCRIPTOR_FILE);
        let bytes = br#"{"schemaVersion":1,"manifest":"project.json"}"#;
        fs::write(&descriptor, bytes).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ, without FILE_SHARE_DELETE.
            .open(&descriptor)
            .unwrap();
        let config = manager.data_dirs().load_config().unwrap();

        assert!(manager.configure_project(&descriptor).is_err());
        assert_eq!(fs::read(&legacy).unwrap(), legacy_manifest);
        assert_eq!(fs::read(&descriptor).unwrap(), bytes);
        assert!(!canonical.exists());
        assert_eq!(manager.data_dirs().load_config().unwrap(), config);

        drop(lock);
        manager.configure_project(&descriptor).unwrap();
        let migrated: ProjectManifest =
            serde_json::from_slice(&fs::read(&canonical).unwrap()).unwrap();
        assert_eq!(
            migrated.schema_version,
            crate::native_project::PROJECT_SCHEMA_VERSION
        );
        assert_eq!(migrated.name, original.name);
        assert_eq!(migrated.source_map, original.source_map);
        assert_eq!(migrated.output_map, original.output_map);
        assert_eq!(migrated.main_file, original.main_file);
        assert_eq!(migrated.settings, original.settings);
        assert_eq!(migrated.plugins, original.plugins);
        assert!(migrated.python_entrypoints.is_empty());
        assert!(migrated.python_dependencies.is_empty());
        assert!(migrated.python_lock.is_none());
        assert!(!legacy.exists());
        assert!(!descriptor.exists());
        let accepted = manager.data_dirs().app_data().join("journal/accepted");
        assert!(fs::read_dir(accepted).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("project-migration-")
        }));
        fs::remove_dir_all(base).unwrap();
    }
}
