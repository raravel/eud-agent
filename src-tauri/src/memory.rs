//! Per-project memory store.
//!
//! Live memory is rooted at `<native project>/.eud-agent/memory`.  The
//! `new` constructor remains only for explicit legacy fixtures/import lookup;
//! normal callers use `for_project` or `current`.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::wiki::Ledger;

/// The four codex/panel-editable markdown files, in render order.
pub const MEMORY_FILES: [&str; 4] = ["resources", "structure", "conventions", "lessons"];

/// Per-file write cap, in UTF-8 bytes (over-budget writes are rejected).
pub const CONTENT_CAP_BYTES: usize = 8192;

/// Rendered `[project memory]` section cap, in characters.
pub const SECTION_CAP_CHARS: usize = 40000;

/// Suffix appended to the `## structure` heading when the LIST hash drifted.
pub const STALE_SUFFIX: &str = "(may be outdated — project files changed since last memory update)";

/// Rendered body when the store is disabled or unreadable.
pub const NO_MEMORY: &str = "(no project memory)";

/// Marker appended after section-cap truncation.
pub const TRUNCATED_MARKER: &str = "memory section truncated";
const IMPORT_META_CAP_BYTES: usize = 64 * 1024;
const IMPORT_WIKI_CAP_BYTES: usize = 4 * 1024 * 1024;

/// Transactional import guard for accepted memory and wiki data from a legacy
/// E3S-backed memory store.
#[derive(Debug)]
pub(crate) struct LegacyMemoryImport {
    owned: Vec<OwnedImportFile>,
    committed: bool,
}

#[derive(Debug)]
struct OwnedImportFile {
    path: PathBuf,
    bytes: Vec<u8>,
}

impl LegacyMemoryImport {
    pub(crate) fn empty() -> Self {
        empty_import()
    }

    /// Commit the imported store, retaining files when the guard is dropped.
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }

    /// Remove only files created by this import, and only if they are unchanged.
    pub(crate) fn rollback(mut self) -> io::Result<()> {
        self.committed = true;
        rollback_owned(&mut self.owned)
    }
}

impl Drop for LegacyMemoryImport {
    fn drop(&mut self) {
        if !self.committed {
            let _ = rollback_owned(&mut self.owned);
        }
    }
}

fn rollback_owned(owned: &mut Vec<OwnedImportFile>) -> io::Result<()> {
    let mut first_error = None;
    for file in owned.drain(..).rev() {
        match fs::read(&file.path) {
            Ok(bytes) if bytes == file.bytes => {
                if let Err(error) = fs::remove_file(&file.path) {
                    if error.kind() != io::ErrorKind::NotFound && first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn empty_import() -> LegacyMemoryImport {
    LegacyMemoryImport {
        owned: Vec::new(),
        committed: true,
    }
}

/// Import accepted memory/wiki files from the legacy E3S-keyed store.
///
/// Every recognized source item is validated independently. A bad item is
/// reported and skipped while healthy siblings are still copied. Source data
/// and existing project-local data are never modified.
impl ProjectMemory {
    pub(crate) fn import_legacy_harness(
        memory_root: &Path,
        source_project: &str,
        target: &crate::native_project::NativeProject,
        issues: &mut Vec<crate::harness_import::HarnessImportIssue>,
    ) -> io::Result<LegacyMemoryImport> {
        let target_dir = target_store_dir(target);
        if let Err(error) = validate_existing_components(memory_root) {
            report_issue(
                issues,
                "memory",
                memory_root,
                format!("legacy memory root is unsafe or unavailable: {error}"),
            );
            return Ok(empty_import());
        }
        let source = match find_legacy_source(memory_root, source_project) {
            Ok(source) => source,
            Err(error) => {
                report_issue(
                    issues,
                    "memory",
                    memory_root,
                    format!("legacy memory source is ambiguous or unavailable: {error}"),
                );
                return Ok(empty_import());
            }
        };
        let Some(source) = source else {
            return Ok(empty_import());
        };
        if let Ok(metadata) = fs::symlink_metadata(&source) {
            if !is_safe_directory_metadata(&metadata) {
                report_issue(
                    issues,
                    "memory",
                    &source,
                    "legacy memory source is not a regular directory",
                );
                return Ok(empty_import());
            }
        } else {
            report_issue(
                issues,
                "memory",
                &source,
                "legacy memory source disappeared",
            );
            return Ok(empty_import());
        }
        let files = collect_import_files(&source, issues);
        let guard = publish_import_files(&target_dir, &files, issues)?;
        Ok(guard)
    }

    /// Migrate the old native-name store once, preserving the origin.
    pub(crate) fn migrate_native_harness(
        memory_root: &Path,
        target: &crate::native_project::NativeProject,
        issues: &mut Vec<crate::harness_import::HarnessImportIssue>,
    ) -> io::Result<LegacyMemoryImport> {
        Self::import_legacy_harness(memory_root, target.manifest().name.as_str(), target, issues)
    }
}

fn target_store_dir(target: &crate::native_project::NativeProject) -> PathBuf {
    target.root().join(".eud-agent").join("memory")
}

fn report_issue(
    issues: &mut Vec<crate::harness_import::HarnessImportIssue>,
    scope: &str,
    path: &Path,
    reason: impl Into<String>,
) {
    issues.push(crate::harness_import::HarnessImportIssue::new(
        scope,
        path.to_string_lossy().into_owned(),
        reason,
    ));
}

fn find_legacy_source(memory_root: &Path, source_project: &str) -> io::Result<Option<PathBuf>> {
    let names = legacy_source_names(source_project);
    let mut matches = Vec::new();
    let entries = match fs::read_dir(memory_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if names.iter().any(|candidate| candidate == &name) {
            matches.push(entry.path());
        }
    }
    if matches.len() > 1 {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "multiple legacy memory stores match the source E3S project",
        ));
    }
    Ok(matches.pop())
}

fn legacy_source_names(source_project: &str) -> Vec<String> {
    let normalized = crate::config::RecentProjectRecord::key(source_project);
    let mut names = Vec::with_capacity(4);
    for name in [
        sanitize_project_name(source_project),
        sanitize_project_name(&normalized),
    ] {
        if name.is_empty() {
            continue;
        }
        let name = name.to_lowercase();
        if !names.contains(&name) {
            names.push(name.clone());
        }
        let quoted = format!("'{name}'");
        if !names.contains(&quoted) {
            names.push(quoted);
        }
    }
    names
}

#[derive(Debug)]
struct ImportFile {
    relative: PathBuf,
    bytes: Vec<u8>,
}

fn collect_import_files(
    source: &Path,
    issues: &mut Vec<crate::harness_import::HarnessImportIssue>,
) -> Vec<ImportFile> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(source) {
        Ok(entries) => entries,
        Err(error) => {
            report_issue(
                issues,
                "memory",
                source,
                format!("cannot read legacy store: {error}"),
            );
            return files;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report_issue(
                    issues,
                    "memory",
                    source,
                    format!("cannot inspect legacy item: {error}"),
                );
                continue;
            }
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                report_issue(
                    issues,
                    "memory",
                    &path,
                    format!("cannot inspect legacy item: {error}"),
                );
                continue;
            }
        };
        if name == "wiki" {
            if !is_safe_directory_metadata(&metadata) {
                report_issue(
                    issues,
                    "memory",
                    &path,
                    "legacy wiki path is not a regular directory",
                );
            } else {
                collect_wiki_file(&path, &mut files, issues);
            }
            continue;
        }
        if name == META_FILE {
            collect_meta_file(&path, &metadata, &mut files, issues);
            continue;
        }
        let Some(relative) = name
            .strip_suffix(".md")
            .filter(|name| MEMORY_FILES.contains(name))
        else {
            continue;
        };
        if !is_safe_regular_file_metadata(&metadata) {
            report_issue(
                issues,
                "memory",
                &path,
                "legacy memory file is not a regular file",
            );
            continue;
        }
        match read_bounded_file(&path, CONTENT_CAP_BYTES)
            .and_then(|bytes| validate_utf8_without_bom(&bytes).map(|_| bytes))
        {
            Ok(bytes) => files.push(ImportFile {
                relative: PathBuf::from(format!("{relative}.md")),
                bytes,
            }),
            Err(error) => report_issue(issues, "memory", &path, error.to_string()),
        }
    }
    files
}

fn collect_meta_file(
    path: &Path,
    metadata: &fs::Metadata,
    files: &mut Vec<ImportFile>,
    issues: &mut Vec<crate::harness_import::HarnessImportIssue>,
) {
    if !is_safe_regular_file_metadata(metadata) {
        report_issue(
            issues,
            "memory",
            path,
            "legacy memory metadata is not a regular file",
        );
        return;
    }
    let bytes = match read_bounded_file(path, IMPORT_META_CAP_BYTES)
        .and_then(|bytes| validate_utf8_without_bom(&bytes).map(|_| bytes))
        .and_then(|bytes| match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(_)) => Ok(bytes),
            Ok(_) => Err(invalid_source(
                "legacy memory metadata must be a JSON object",
            )),
            Err(error) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("legacy memory metadata is invalid JSON: {error}"),
            )),
        }) {
        Ok(bytes) => bytes,
        Err(error) => {
            report_issue(issues, "memory", path, error.to_string());
            return;
        }
    };
    files.push(ImportFile {
        relative: PathBuf::from(META_FILE),
        bytes,
    });
}

fn collect_wiki_file(
    path: &Path,
    files: &mut Vec<ImportFile>,
    issues: &mut Vec<crate::harness_import::HarnessImportIssue>,
) {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            report_issue(
                issues,
                "memory",
                path,
                format!("cannot read legacy wiki: {error}"),
            );
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report_issue(
                    issues,
                    "memory",
                    path,
                    format!("cannot inspect legacy wiki: {error}"),
                );
                continue;
            }
        };
        if entry.file_name() != "ledger.json" {
            continue;
        }
        let child = entry.path();
        let metadata = match fs::symlink_metadata(&child) {
            Ok(metadata) => metadata,
            Err(error) => {
                report_issue(
                    issues,
                    "memory",
                    &child,
                    format!("cannot inspect wiki ledger: {error}"),
                );
                continue;
            }
        };
        if !is_safe_regular_file_metadata(&metadata) {
            report_issue(
                issues,
                "memory",
                &child,
                "legacy wiki ledger is not a regular file",
            );
            continue;
        }
        let bytes = match read_bounded_file(&child, IMPORT_WIKI_CAP_BYTES)
            .and_then(|bytes| validate_utf8_without_bom(&bytes).map(|_| bytes))
            .and_then(|bytes| {
                serde_json::from_slice::<Ledger>(&bytes)
                    .map(|_| bytes)
                    .map_err(|error| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("legacy wiki ledger is invalid: {error}"),
                        )
                    })
            }) {
            Ok(bytes) => bytes,
            Err(error) => {
                report_issue(issues, "memory", &child, error.to_string());
                continue;
            }
        };
        files.push(ImportFile {
            relative: PathBuf::from("wiki").join("ledger.json"),
            bytes,
        });
    }
}

fn publish_import_files(
    target: &Path,
    files: &[ImportFile],
    issues: &mut Vec<crate::harness_import::HarnessImportIssue>,
) -> io::Result<LegacyMemoryImport> {
    if let Err(error) = validate_existing_components(target) {
        report_issue(
            issues,
            "memory",
            target,
            format!("local memory destination is unsafe or unavailable: {error}"),
        );
        return Ok(empty_import());
    }
    if let Err(error) = fs::create_dir_all(target) {
        report_issue(
            issues,
            "memory",
            target,
            format!("cannot create local memory destination: {error}"),
        );
        return Ok(empty_import());
    }
    let mut owned = Vec::new();
    for file in files {
        let destination = target.join(&file.relative);
        match fs::symlink_metadata(&destination) {
            Ok(metadata) => {
                let reason = if is_safe_regular_file_metadata(&metadata) {
                    "destination already contains local data"
                } else {
                    "destination is not a safe regular file"
                };
                report_issue(issues, "memory", &destination, reason);
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                report_issue(
                    issues,
                    "memory",
                    &destination,
                    format!("cannot inspect local destination: {error}"),
                );
                continue;
            }
        }
        if let Some(parent) = destination.parent() {
            if let Err(error) =
                validate_existing_components(parent).and_then(|_| fs::create_dir_all(parent))
            {
                report_issue(
                    issues,
                    "memory",
                    &destination,
                    format!("local destination is unavailable: {error}"),
                );
                continue;
            }
        }
        match write_import_no_replace(&destination, &file.bytes) {
            Ok(tmp) => {
                owned.push(OwnedImportFile {
                    path: destination,
                    bytes: file.bytes.clone(),
                });
                if let Err(error) = remove_import_temp(&tmp) {
                    return abort_publish(&mut owned, error);
                }
            }
            Err(ImportPublishError::Cleanup(error)) => {
                return abort_publish(&mut owned, error);
            }
            Err(ImportPublishError::Item(error)) => {
                let reason = if error.kind() == io::ErrorKind::AlreadyExists {
                    "destination was occupied during import".to_string()
                } else {
                    format!("cannot publish imported item: {error}")
                };
                report_issue(issues, "memory", &destination, reason);
            }
        }
    }
    Ok(LegacyMemoryImport {
        owned,
        committed: false,
    })
}
enum ImportPublishError {
    Item(io::Error),
    Cleanup(io::Error),
}

fn write_import_no_replace(path: &Path, bytes: &[u8]) -> Result<PathBuf, ImportPublishError> {
    let tmp = tmp_path(path);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(ImportPublishError::Item)?;
    use std::io::Write;
    let written = file.write_all(bytes);
    drop(file);
    match written.and_then(|_| crate::harness_import::publish_staged_file(&tmp, path)) {
        Ok(()) => Ok(tmp),
        Err(error) => match remove_import_temp(&tmp) {
            Ok(()) => Err(ImportPublishError::Item(error)),
            Err(cleanup) => Err(ImportPublishError::Cleanup(io::Error::other(format!(
                "{error}; temporary import cleanup failed: {cleanup}"
            )))),
        },
    }
}

fn remove_import_temp(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!(
                "cannot remove import staging file '{}': {error}",
                path.display()
            ),
        )),
    }
}

fn abort_publish(
    owned: &mut Vec<OwnedImportFile>,
    error: io::Error,
) -> io::Result<LegacyMemoryImport> {
    if let Err(rollback) = rollback_owned(owned) {
        return Err(io::Error::other(format!(
            "legacy memory import cleanup failed: {error}; rollback failed: {rollback}"
        )));
    }
    Err(error)
}

fn read_bounded_file(path: &Path, cap: usize) -> io::Result<Vec<u8>> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > cap as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("legacy import file exceeds {cap}-byte limit"),
        ));
    }
    let file = fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(cap as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > cap {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("legacy import file exceeds {cap}-byte limit"),
        ));
    }
    Ok(bytes)
}

fn validate_utf8_without_bom(bytes: &[u8]) -> io::Result<()> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(invalid_source(
            "legacy import files must be UTF-8 without BOM",
        ));
    }
    std::str::from_utf8(bytes)
        .map(|_| ())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub(crate) fn validate_existing_components(path: &Path) -> io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        // A Windows volume prefix (notably \\?\C:) is not a directory until
        // its root separator or first relative component has been appended.
        if matches!(component, std::path::Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if is_untrusted_link(&metadata) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "memory path contains a symlink/reparse point",
                    ));
                }
                // A trusted system link must still resolve to a directory.
                let is_dir = if metadata.file_type().is_symlink() {
                    fs::metadata(&current).is_ok_and(|target| target.is_dir())
                } else {
                    metadata.is_dir()
                };
                if !is_dir {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "memory path contains a non-directory component",
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn safe_regular_file(path: &Path) -> bool {
    if let Some(parent) = path.parent() {
        if validate_existing_components(parent).is_err() {
            return false;
        }
    }
    fs::symlink_metadata(path)
        .map(|metadata| is_safe_regular_file_metadata(&metadata))
        .unwrap_or(false)
}

fn is_safe_directory_metadata(metadata: &fs::Metadata) -> bool {
    metadata.is_dir() && !metadata.file_type().is_symlink() && !is_reparse_point(metadata)
}

fn is_safe_regular_file_metadata(metadata: &fs::Metadata) -> bool {
    metadata.is_file() && !metadata.file_type().is_symlink() && !is_reparse_point(metadata)
}

#[cfg(windows)]
pub(crate) fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
pub(crate) fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

/// A path-ancestor link an unprivileged user could have planted. Root-owned Unix
/// system links (macOS `/var` -> `/private/var`, `/tmp`) are trusted ancestors;
/// on Windows every symlink or reparse point is rejected.
#[cfg(unix)]
pub(crate) fn is_untrusted_link(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.file_type().is_symlink() && metadata.uid() != 0
}

#[cfg(not(unix))]
pub(crate) fn is_untrusted_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink() || is_reparse_point(metadata)
}

fn invalid_source(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

const META_FILE: &str = "meta.json";
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
const INSTRUCTION_BLOCK: &str = concat!(
    "This is accepted durable project memory: resource allocations, file roles, ",
    "naming/trigger conventions, and user corrections. Treat it as context, but ",
    "do not edit it during foreground implementation. The post-acceptance harness ",
    "synchronizes non-derivable durable facts after code approval."
);

/// Outcome of a [`ProjectMemory::write`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteResult {
    pub ok: bool,
    pub reason: String,
}

impl WriteResult {
    fn ok() -> Self {
        Self {
            ok: true,
            reason: String::new(),
        }
    }

    fn rejected(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            reason: reason.into(),
        }
    }
}

/// Sanitize a bridge project name into a Windows-safe directory name.
///
/// Characters invalid in Windows file names (`<>:"/\|?*` and control chars) are replaced
/// with `_`, and trailing dots/spaces are stripped. An empty or whitespace-only name, or a
/// name that collapses to empty after stripping, returns `""` and disables the store.
pub fn sanitize_project_name(name: &str) -> String {
    if name.trim().is_empty() {
        return String::new();
    }

    let mut cleaned = String::with_capacity(name.len());
    for ch in name.chars() {
        if is_invalid_windows_filename_char(ch) {
            cleaned.push('_');
        } else {
            cleaned.push(ch);
        }
    }

    cleaned.trim_end_matches(['.', ' ']).to_string()
}

/// Return the sha256 hex digest of a bridge LIST reply.
pub fn list_hash(list_reply: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(list_reply.as_bytes());
    hex_lower(&hasher.finalize())
}

#[derive(Debug, Clone)]
pub struct ProjectMemory {
    memory_root: PathBuf,
    project_name: String,
    sanitized: String,
    local_store: bool,
}

impl ProjectMemory {
    /// Construct an explicit legacy lookup store. Live callers must use
    /// [`ProjectMemory::for_project`] or [`ProjectMemory::current`].
    pub fn new(memory_root: impl Into<PathBuf>, project_name: impl Into<String>) -> Self {
        let project_name = project_name.into();
        let sanitized = sanitize_project_name(&project_name);
        Self {
            memory_root: memory_root.into(),
            project_name,
            sanitized,
            local_store: false,
        }
    }

    /// Construct the project-local store at `<root>/.eud-agent/memory`.
    pub fn for_project(project: &crate::native_project::NativeProject) -> Self {
        Self {
            memory_root: project.root().join(".eud-agent").join("memory"),
            project_name: project.manifest().name.clone(),
            sanitized: sanitize_project_name(&project.manifest().name),
            local_store: true,
        }
    }

    /// Resolve and validate the configured native project.
    pub fn current(dirs: &crate::config::DataDirs) -> Result<Self, String> {
        let config = dirs.load_config().map_err(|error| error.to_string())?;
        let configured = config.project_path.trim();
        if configured.is_empty() {
            return Err("no project is open; memory is disabled".to_string());
        }
        let project = crate::native_project::NativeProject::open(Path::new(configured))?;
        Ok(Self::for_project(&project))
    }
    /// Disabled provider used when no project can be resolved.
    pub fn disabled() -> Self {
        Self {
            memory_root: PathBuf::new(),
            project_name: String::new(),
            sanitized: String::new(),
            local_store: true,
        }
    }

    /// True when a non-empty project name yields a usable store.
    pub fn enabled(&self) -> bool {
        !self.sanitized.is_empty()
    }

    /// The store directory, or `None` when the store is disabled.
    pub fn store_dir(&self) -> Option<PathBuf> {
        self.enabled().then(|| {
            if self.local_store {
                self.memory_root.clone()
            } else {
                self.memory_root.join(&self.sanitized)
            }
        })
    }
    /// Raw project name supplied at construction time or read from the manifest.
    pub fn project_name(&self) -> &str {
        &self.project_name
    }
    /// Return a markdown file's content, or `""` when absent/disabled/unreadable.
    ///
    /// A read never creates the store dir and never errors.
    pub fn read(&self, name: &str) -> String {
        let Some(path) = self.file_path(name) else {
            return String::new();
        };
        if !safe_regular_file(&path) {
            return String::new();
        }
        fs::read_to_string(path).unwrap_or_default()
    }

    /// Atomically write a markdown file (full replacement); return the outcome.
    ///
    /// Rejected writes do not touch disk when disabled, `name` is unknown, or `content`
    /// exceeds the UTF-8 byte cap. Successful writes are UTF-8 bytes without BOM.
    pub fn write(&self, name: &str, content: &str) -> WriteResult {
        let Some(store) = self.store_dir() else {
            return WriteResult::rejected("no project is open; memory is disabled");
        };
        if !MEMORY_FILES.contains(&name) {
            return WriteResult::rejected(format!(
                "unknown memory file '{name}'; expected one of {}",
                MEMORY_FILES.join(", ")
            ));
        }

        let encoded = content.as_bytes();
        if encoded.len() > CONTENT_CAP_BYTES {
            return WriteResult::rejected(format!(
                "content is {} bytes, over the {CONTENT_CAP_BYTES}-byte budget; condense it.",
                encoded.len()
            ));
        }

        match write_atomic_bytes(&store.join(format!("{name}.md")), encoded) {
            Ok(()) => WriteResult::ok(),
            Err(err) => WriteResult::rejected(err.to_string()),
        }
    }

    /// Return `meta.json` as an object, or `{}` when absent/disabled/malformed.
    pub fn read_meta(&self) -> Map<String, Value> {
        let Some(path) = self.meta_path() else {
            return Map::new();
        };
        if !safe_regular_file(&path) {
            return Map::new();
        }
        let Ok(bytes) = fs::read(path) else {
            return Map::new();
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(meta)) => meta,
            _ => Map::new(),
        }
    }

    /// Atomically write `meta.json` (UTF-8 no BOM); no-op when disabled.
    pub fn write_meta(&self, meta: &Map<String, Value>) -> anyhow::Result<()> {
        let Some(path) = self.meta_path() else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(meta)?;
        write_atomic_bytes(&path, &bytes)?;
        Ok(())
    }

    /// Record the current LIST reply's hash and an epoch-second timestamp in `meta.json`.
    pub fn update_list_hash(&self, list_reply: &str) -> anyhow::Result<()> {
        let mut meta = self.read_meta();
        meta.insert("version".to_string(), Value::from(1));
        meta.insert("list_hash".to_string(), Value::from(list_hash(list_reply)));
        meta.insert("list_hash_ts".to_string(), Value::from(epoch_seconds()));
        self.write_meta(&meta)
    }

    /// True when the stored `list_hash` differs from the current LIST reply.
    ///
    /// A store with no recorded hash is treated as stale.
    pub fn is_stale(&self, list_reply: &str) -> bool {
        let meta = self.read_meta();
        match meta.get("list_hash").and_then(Value::as_str) {
            Some(stored) => stored != list_hash(list_reply),
            None => true,
        }
    }

    /// Build the `[project memory]` prompt section.
    pub fn render_section(&self, list_reply: Option<&str>) -> String {
        if !self.enabled() {
            return no_memory_section();
        }

        match self.render_enabled(list_reply) {
            Ok(section) => section,
            Err(_) => no_memory_section(),
        }
    }

    fn render_enabled(&self, list_reply: Option<&str>) -> anyhow::Result<String> {
        let mut files = Vec::with_capacity(MEMORY_FILES.len());
        for name in MEMORY_FILES {
            files.push((name, self.read_for_render(name)?));
        }

        let stale = list_reply.is_some_and(|reply| self.is_stale(reply));
        let mut body_parts = vec![INSTRUCTION_BLOCK.to_string()];
        body_parts.extend(render_file_blocks(&files, stale, None));

        let section = section_from_parts(&body_parts);
        if section.chars().count() <= SECTION_CAP_CHARS {
            return Ok(section);
        }

        Ok(render_with_truncated_lessons(&files, stale))
    }

    fn read_for_render(&self, name: &str) -> anyhow::Result<String> {
        let Some(path) = self.file_path(name) else {
            return Ok(String::new());
        };
        if !safe_regular_file(&path) {
            return Ok(String::new());
        }
        Ok(fs::read_to_string(path)?)
    }

    fn file_path(&self, name: &str) -> Option<PathBuf> {
        self.store_dir()
            .map(|store| store.join(format!("{name}.md")))
    }

    fn meta_path(&self) -> Option<PathBuf> {
        self.store_dir().map(|store| store.join(META_FILE))
    }
}

fn is_invalid_windows_filename_char(ch: char) -> bool {
    matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (ch as u32) <= 0x1f
}

/// Atomically write `bytes` to `path` (temp + rename) as raw bytes (no BOM).
///
pub(crate) fn write_atomic_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        validate_existing_components(parent)?;
        fs::create_dir_all(parent)?;
        validate_existing_components(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || is_reparse_point(&metadata)
                || !metadata.is_file()
            {
                anyhow::bail!("refusing to write through an unsafe memory path");
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let tmp = tmp_path(path);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    use std::io::Write;
    if let Err(error) = file.write_all(bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(error.into());
    }
    drop(file);
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(err.into());
    }
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{file_name}.{pid}.{nanos}.{seq}.tmp"))
}

fn render_file_blocks(
    files: &[(&str, String)],
    stale: bool,
    lessons_override: Option<&str>,
) -> Vec<String> {
    let mut blocks = Vec::new();
    for (name, body) in files {
        let body = if *name == "lessons" {
            lessons_override.unwrap_or(body.trim())
        } else {
            body.trim()
        };
        if body.is_empty() {
            continue;
        }

        let mut heading = format!("## {name}");
        if *name == "structure" && stale {
            heading = format!("{heading} {STALE_SUFFIX}");
        }
        blocks.push(format!("{heading}\n{body}"));
    }
    blocks
}

fn render_with_truncated_lessons(files: &[(&str, String)], stale: bool) -> String {
    let mut fixed_parts = vec![INSTRUCTION_BLOCK.to_string()];
    for (name, body) in files {
        if *name == "lessons" {
            continue;
        }
        let body = body.trim();
        if body.is_empty() {
            continue;
        }

        let mut heading = format!("## {name}");
        if *name == "structure" && stale {
            heading = format!("{heading} {STALE_SUFFIX}");
        }
        fixed_parts.push(format!("{heading}\n{body}"));
    }

    let lessons_body = files
        .iter()
        .find_map(|(name, body)| {
            if *name == "lessons" {
                Some(body.trim())
            } else {
                None
            }
        })
        .unwrap_or("");

    let frame_parts = [
        fixed_parts.clone(),
        vec!["## lessons\n".to_string(), TRUNCATED_MARKER.to_string()],
    ]
    .concat();
    let frame_len = section_from_parts(&frame_parts).chars().count();
    let budget = SECTION_CAP_CHARS.saturating_sub(frame_len);
    let head = take_chars(lessons_body, budget);

    let mut parts = fixed_parts;
    if !head.is_empty() {
        parts.push(format!("## lessons\n{head}"));
    }
    parts.push(TRUNCATED_MARKER.to_string());

    clamp_chars(&section_from_parts(&parts), SECTION_CAP_CHARS)
}

fn section_from_parts(parts: &[String]) -> String {
    format!("[project memory]\n{}", parts.join("\n\n"))
}

fn no_memory_section() -> String {
    format!("[project memory]\n{NO_MEMORY}")
}

fn take_chars(s: &str, limit: usize) -> String {
    s.chars().take(limit).collect()
}

fn clamp_chars(s: &str, limit: usize) -> String {
    s.chars().take(limit).collect()
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DataDirs;
    use serde_json::json;

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-memory-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn memory_root(tag: &str) -> (PathBuf, PathBuf) {
        let base = unique_temp_dir(tag);
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        (base, dirs.memory_dir())
    }

    #[test]
    fn write_then_read_round_trips_under_memory_root() {
        let (base, root) = memory_root("round-trip");
        let memory = ProjectMemory::new(root.clone(), "My<Project>");

        let result = memory.write("resources", "Switch 12 = boss phase");

        assert!(result.ok, "{result:?}");
        assert_eq!(memory.read("resources"), "Switch 12 = boss phase");
        assert_eq!(
            fs::read_to_string(root.join("My_Project_").join("resources.md")).unwrap(),
            "Switch 12 = boss phase"
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn atomic_write_uses_unique_temp_paths_and_leaves_no_tmp_files() {
        let base = unique_temp_dir("atomic-temp");
        let target = base.join("resources.md");

        let first_tmp = tmp_path(&target);
        let second_tmp = tmp_path(&target);
        assert_ne!(first_tmp, second_tmp);
        assert_eq!(first_tmp.parent(), target.parent());
        assert_eq!(second_tmp.parent(), target.parent());

        let first_name = first_tmp.file_name().unwrap().to_string_lossy();
        let second_name = second_tmp.file_name().unwrap().to_string_lossy();
        assert!(first_name.starts_with("resources.md."));
        assert!(second_name.starts_with("resources.md."));
        assert!(first_name.ends_with(".tmp"));
        assert!(second_name.ends_with(".tmp"));

        write_atomic_bytes(&target, b"first").unwrap();
        write_atomic_bytes(&target, b"second").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "second");
        let tmp_files: Vec<PathBuf> = fs::read_dir(&base)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".tmp"))
            })
            .collect();
        assert!(tmp_files.is_empty(), "leftover temp files: {tmp_files:?}");

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn write_rejects_over_cap_and_unknown_name_without_touching_prior_content() {
        let (base, root) = memory_root("rejects");
        let memory = ProjectMemory::new(root, "Project");
        assert!(memory.write("lessons", "prior").ok);

        let over = "x".repeat(CONTENT_CAP_BYTES + 1);
        let result = memory.write("lessons", &over);
        assert!(!result.ok);
        assert_eq!(
            result.reason,
            format!(
                "content is {} bytes, over the {CONTENT_CAP_BYTES}-byte budget; condense it.",
                CONTENT_CAP_BYTES + 1
            )
        );
        assert_eq!(memory.read("lessons"), "prior");

        let result = memory.write("unknown", "new");
        assert!(!result.ok);
        assert_eq!(
            result.reason,
            "unknown memory file 'unknown'; expected one of resources, structure, conventions, lessons"
        );
        assert_eq!(memory.read("lessons"), "prior");
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn sanitize_invalid_chars_trailing_dots_and_disabled_empty_names() {
        assert_eq!(sanitize_project_name(r#"a<>:"/\|?*b"#), "a_________b");
        assert_eq!(sanitize_project_name("inner. dot .  "), "inner. dot");
        assert_eq!(sanitize_project_name("   "), "");
        assert_eq!(sanitize_project_name("..."), "");

        let (base, root) = memory_root("disabled");
        let memory = ProjectMemory::new(root.clone(), "   ");
        assert!(!memory.enabled());
        assert_eq!(memory.store_dir(), None);
        assert_eq!(memory.read("resources"), "");

        let result = memory.write("resources", "content");
        assert!(!result.ok);
        assert_eq!(result.reason, "no project is open; memory is disabled");
        assert_eq!(
            memory.render_section(None),
            "[project memory]\n(no project memory)"
        );
        assert!(
            !root.exists(),
            "disabled store must not create the memory root"
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn meta_write_read_and_staleness() {
        let (base, root) = memory_root("meta");
        let memory = ProjectMemory::new(root, "Project");

        assert!(memory.is_stale("LIST a"));

        let mut meta = Map::new();
        meta.insert("custom".to_string(), json!("kept"));
        memory.write_meta(&meta).unwrap();
        assert_eq!(memory.read_meta().get("custom"), Some(&json!("kept")));

        memory.update_list_hash("LIST a").unwrap();
        assert!(!memory.is_stale("LIST a"));
        assert!(memory.is_stale("LIST b"));
        assert_eq!(memory.read_meta().get("version"), Some(&json!(1)));
        assert_eq!(
            memory.read_meta().get("list_hash"),
            Some(&json!(list_hash("LIST a")))
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn render_section_disabled_and_enabled_order_staleness() {
        let (base, root) = memory_root("render");
        let disabled = ProjectMemory::new(root.clone(), "");
        assert_eq!(
            disabled.render_section(None),
            "[project memory]\n(no project memory)"
        );

        let memory = ProjectMemory::new(root, "Project");
        assert!(memory.write("resources", "res").ok);
        assert!(memory.write("structure", "struct").ok);
        assert!(memory.write("conventions", "conv").ok);
        assert!(memory.write("lessons", "").ok);
        memory.update_list_hash("LIST old").unwrap();

        let section = memory.render_section(Some("LIST new"));
        let resources = section.find("## resources\nres").unwrap();
        let structure = section
            .find(&format!("## structure {STALE_SUFFIX}\nstruct"))
            .unwrap();
        let conventions = section.find("## conventions\nconv").unwrap();

        assert!(resources < structure);
        assert!(structure < conventions);
        assert!(!section.contains("## lessons"));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn render_truncation_tail_truncates_lessons() {
        let (base, root) = memory_root("truncate");
        let memory = ProjectMemory::new(root, "Project");
        assert!(memory.write("resources", "res").ok);

        let store = memory.store_dir().unwrap();
        fs::create_dir_all(&store).unwrap();
        fs::write(
            store.join("lessons.md"),
            "L".repeat(SECTION_CAP_CHARS + 1000),
        )
        .unwrap();

        let section = memory.render_section(None);
        assert!(section.chars().count() <= SECTION_CAP_CHARS);
        assert!(section.contains(TRUNCATED_MARKER));
        assert!(section.contains("## lessons\n"));
        assert!(section.contains(&"L".repeat(100)));
        assert!(!section.contains(&"L".repeat(SECTION_CAP_CHARS)));
        fs::remove_dir_all(base).ok();
    }
    fn native_fixture(base: &Path, name: &str) -> crate::native_project::NativeProject {
        use crate::native_project::{
            NativeProject, ProjectManifest, ProjectSettings, PROJECT_SCHEMA_VERSION,
        };
        let root = base.join(name);
        fs::create_dir_all(root.join("maps")).unwrap();
        fs::write(root.join("maps/source.scx"), b"map").unwrap();
        NativeProject::create(
            &root,
            ProjectManifest {
                schema_version: PROJECT_SCHEMA_VERSION,
                name: name.to_string(),
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
        .unwrap()
    }

    #[test]
    fn local_store_is_isolated_and_moves_with_project_root() {
        let base = unique_temp_dir("local-isolation");
        let first = native_fixture(&base.join("first"), "Shared");
        let second = native_fixture(&base.join("second"), "Shared");
        assert!(
            ProjectMemory::for_project(&first)
                .write("resources", "one")
                .ok
        );
        assert!(
            ProjectMemory::for_project(&second)
                .write("resources", "two")
                .ok
        );
        assert_eq!(ProjectMemory::for_project(&first).read("resources"), "one");
        assert_eq!(ProjectMemory::for_project(&second).read("resources"), "two");
        let moved_root = base.join("moved");
        fs::rename(first.root(), &moved_root).unwrap();
        let moved = crate::native_project::NativeProject::open(&moved_root).unwrap();
        assert_eq!(ProjectMemory::for_project(&moved).read("resources"), "one");
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn import_reports_bad_items_but_copies_healthy_siblings() {
        let base = unique_temp_dir("partial-import");
        let target = native_fixture(&base, "target");
        let legacy_root = base.join("legacy");
        let source = legacy_root.join(sanitize_project_name("old.e3s"));
        fs::create_dir_all(source.join("wiki")).unwrap();
        fs::write(source.join("resources.md"), "healthy").unwrap();
        fs::write(source.join("structure.md"), [0xff, 0xfe]).unwrap();
        fs::write(source.join(META_FILE), b"{bad").unwrap();
        fs::write(source.join("wiki/ledger.json"), b"{bad").unwrap();
        let mut issues = Vec::new();
        let guard =
            ProjectMemory::import_legacy_harness(&legacy_root, "old.e3s", &target, &mut issues)
                .unwrap();
        assert_eq!(
            ProjectMemory::for_project(&target).read("resources"),
            "healthy"
        );
        assert!(issues
            .iter()
            .any(|issue| issue.path.ends_with("structure.md")));
        assert!(issues.iter().any(|issue| issue.path.ends_with("meta.json")));
        assert!(issues
            .iter()
            .any(|issue| issue.path.ends_with("ledger.json")));
        guard.commit();
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn rollback_preserves_concurrent_changes_and_existing_files() {
        let base = unique_temp_dir("rollback-owned");
        let target = native_fixture(&base, "target");
        let legacy_root = base.join("legacy");
        let source = legacy_root.join("old");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("resources.md"), "imported").unwrap();
        let mut issues = Vec::new();
        let guard = ProjectMemory::import_legacy_harness(&legacy_root, "old", &target, &mut issues)
            .unwrap();
        let memory = ProjectMemory::for_project(&target);
        assert!(memory.write("resources", "user changed").ok);
        assert!(memory.write("lessons", "concurrent").ok);
        guard.rollback().unwrap();
        assert_eq!(memory.read("resources"), "user changed");
        assert_eq!(memory.read("lessons"), "concurrent");
        fs::remove_dir_all(base).ok();
    }
    #[test]
    fn ambiguous_legacy_aliases_are_reported_without_copying() {
        let base = unique_temp_dir("ambiguous-import");
        let target = native_fixture(&base, "old");
        let legacy_root = base.join("legacy");
        fs::create_dir_all(legacy_root.join("old")).unwrap();
        fs::create_dir_all(legacy_root.join("'old'")).unwrap();
        let mut issues = Vec::new();
        let guard =
            ProjectMemory::migrate_native_harness(&legacy_root, &target, &mut issues).unwrap();
        assert!(!issues.is_empty());
        assert!(ProjectMemory::for_project(&target)
            .read("resources")
            .is_empty());
        guard.commit();
        fs::remove_dir_all(base).ok();
    }
    #[test]
    fn quoted_full_path_alias_imports_without_mutating_origin() {
        let base = unique_temp_dir("quoted-import");
        let target = native_fixture(&base, "target");
        let legacy_root = base.join("legacy");
        let source_project = r"E:\proj\maps\one.e3s";
        let source_name = sanitize_project_name(source_project);
        let source = legacy_root.join(format!("'{source_name}'"));
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("resources.md"), "from legacy").unwrap();
        let before = fs::read(source.join("resources.md")).unwrap();
        let mut issues = Vec::new();
        let guard = ProjectMemory::import_legacy_harness(
            &legacy_root,
            source_project,
            &target,
            &mut issues,
        )
        .unwrap();
        assert_eq!(
            ProjectMemory::for_project(&target).read("resources"),
            "from legacy"
        );
        guard.commit();
        assert_eq!(fs::read(source.join("resources.md")).unwrap(), before);
        fs::remove_dir_all(base).ok();
    }
    #[test]
    fn import_collision_preserves_local_file_and_reports_issue() {
        let base = unique_temp_dir("collision-import");
        let target = native_fixture(&base, "target");
        let local = ProjectMemory::for_project(&target);
        assert!(local.write("resources", "local").ok);
        let legacy_root = base.join("legacy");
        let source = legacy_root.join("old");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("resources.md"), "legacy").unwrap();
        let mut issues = Vec::new();
        let guard = ProjectMemory::import_legacy_harness(&legacy_root, "old", &target, &mut issues)
            .unwrap();
        assert_eq!(local.read("resources"), "local");
        assert!(issues
            .iter()
            .any(|issue| issue.path.ends_with("resources.md")));
        guard.commit();
        fs::remove_dir_all(base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn ancestors_trust_root_owned_system_links_but_reject_user_links() {
        let base = std::env::temp_dir().join(format!("eud-memory-links-{}", uuid::Uuid::new_v4()));
        let real = base.join("real");
        fs::create_dir_all(&real).unwrap();
        // macOS temp dirs live under the root-owned `/var` -> `/private/var` link.
        validate_existing_components(&real.join("child")).unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let error = validate_existing_components(&link.join("child")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        fs::remove_dir_all(base).ok();
    }
}
