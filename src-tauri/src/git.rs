//! Project-root git: the app's rollback authority.
//!
//! Free CRUD at the project root removes the journal's reverse-order rollback,
//! so the way back to a known state is a commit. The app initializes the
//! repository when the project has none, commits at every turn boundary, and
//! separates the user's own external edits (SCMDraft, an editor, another tool)
//! into their own commit so a turn commit holds only what that turn did.
//!
//! A repository that was already here belongs to the user. The app never
//! commits into it before the user says it may; until then the project works
//! exactly as before and rolling back is the user's own business.
//!
//! Every command runs non-interactively. A credential prompt, a commit hook
//! waiting on input, or a signing key with no agent would hang a windowless
//! GUI app, so terminal prompts are off and hooks and signing are skipped for
//! the app's own commits. The user's `git` in a terminal is unaffected.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Where the per-project git decision is kept. The generated `.gitignore`
/// excludes `.eud-agent/state/`, so the decision never becomes a commit.
const SETTINGS_RELATIVE: &str = ".eud-agent/state/git.json";

/// What the app's own `.gitignore` excludes: generated build output, the
/// epScript Python cache, the app's project-local runtime state, and the E3S
/// compatibility copy. Canonical authoring state and `maps/` stay committed —
/// a map the user changed in SCMDraft has to be recoverable too.
const IGNORED: &[&str] = &["build/", "**/__epspy__/", ".eud-agent/state/", "compat/"];

/// The identity the app commits under when the repository has none configured.
/// A repository that already names an author keeps it.
const IDENTITY_NAME: &str = "eud-agent";
const IDENTITY_EMAIL: &str = "eud-agent@localhost";

/// The longest request summary a commit subject carries.
const SUBJECT_LIMIT: usize = 72;

/// Where a project's repository came from, which decides whose history it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoOrigin {
    /// The app ran `git init` here, so the whole history is the app's.
    App,
    /// The repository was already here when the project was first opened.
    Preexisting,
}

/// Whether the app may commit into this repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Consent {
    /// The user has not answered yet. The app does not commit.
    Pending,
    Granted,
    Declined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    schema_version: u32,
    origin: RepoOrigin,
    consent: Consent,
}

impl Settings {
    fn new(origin: RepoOrigin, consent: Consent) -> Self {
        Self {
            schema_version: 1,
            origin,
            consent,
        }
    }
}

/// What the app knows about one project's repository after preparing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoState {
    /// `git` is on PATH.
    pub available: bool,
    /// The project root is inside a git work tree.
    pub tracked: bool,
    /// The work tree's top level is above the project root — the project is a
    /// folder inside a larger repository, so commits stay limited to the root.
    pub nested: bool,
    pub origin: Option<RepoOrigin>,
    pub consent: Consent,
    /// What the user has to be told once, in Korean, or nothing.
    pub warning: Option<String>,
}

impl RepoState {
    /// True when a turn boundary may commit.
    pub fn auto_commit_allowed(&self) -> bool {
        self.available && self.tracked && self.consent == Consent::Granted
    }

    fn unavailable(warning: String) -> Self {
        Self {
            available: false,
            tracked: false,
            nested: false,
            origin: None,
            consent: Consent::Pending,
            warning: Some(warning),
        }
    }
}

/// One commit the app made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRecord {
    pub sha: String,
    pub subject: String,
    /// How many paths the commit changed.
    pub files: usize,
}

/// One entry of the project's history, for the panel's diff view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSummary {
    pub sha: String,
    pub subject: String,
    /// Committer date, seconds since the epoch.
    pub timestamp: i64,
}

/// The `git` executable, resolved once. `None` means git is not installed.
fn git_binary() -> Option<&'static PathBuf> {
    static BINARY: OnceLock<Option<PathBuf>> = OnceLock::new();
    BINARY.get_or_init(|| which::which("git").ok()).as_ref()
}

/// True when this machine has git at all.
pub fn available() -> bool {
    git_binary().is_some()
}

/// git refuses to touch a repository it decides belongs to someone else. On a
/// volume that records no ownership at all — exFAT, a network share — that is
/// every repository on it, so an ordinary project on an ordinary drive stops
/// the app with "detected dubious ownership" before it can commit anything.
///
/// The exception is passed per command and never written to the user's config,
/// and it covers only the project root and the folders above it: exactly the
/// repositories `-C root` could discover, and nothing else on the machine.
/// git matches these against its own spelling of the path — forward-slashed,
/// with no `\\?\` prefix — and silently ignores any other form.
fn safe_directory_args(root: &Path) -> Vec<String> {
    root.ancestors()
        .map(git_path)
        .filter(|path| !path.is_empty())
        .map(|path| format!("safe.directory={path}"))
        .collect()
}

/// A path as git spells it.
fn git_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix("\\\\?\\")
        .unwrap_or(&text)
        .replace('\\', "/")
}

/// Run one git command rooted at `root` and return its output, whatever the
/// exit status. Only a failure to start the process is an error here; a
/// non-zero status is the caller's to interpret.
fn run(root: &Path, args: &[&str]) -> Result<Output, String> {
    let binary = git_binary().ok_or_else(|| "git이 설치되어 있지 않습니다.".to_string())?;
    let mut command = Command::new(binary);
    command.arg("-C").arg(root);
    for exception in safe_directory_args(root) {
        command.arg("-c").arg(exception);
    }
    command
        // Windows project paths are long (checkpoint 41's Map path work); a
        // repository the app creates must not fail on one.
        .args(["-c", "core.longpaths=true"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // The app is a windowless GUI process; without CREATE_NO_WINDOW every
        // git call flashes a console.
        command.creation_flags(0x0800_0000);
    }
    command
        .output()
        .map_err(|error| format!("git 실행에 실패했습니다: {error}"))
}

/// Run one git command and require success, returning its stdout.
fn run_ok(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = run(root, args)?;
    if !output.status.success() {
        return Err(format!(
            "git {} 실패: {}",
            args.first().copied().unwrap_or(""),
            message_of(&output)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The most useful line of a failed command's output.
fn message_of(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if stderr.trim().is_empty() {
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        stderr.into_owned()
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "알 수 없는 오류".to_string()
    } else {
        trimmed.lines().take(3).collect::<Vec<_>>().join(" / ")
    }
}

/// Prepare `root` for automatic commits and report what the user has to know.
///
/// A project with no repository gets one, with the app's `.gitignore` and an
/// initial commit of the current state. A repository that was already here
/// waits for the user's consent and is left untouched until then.
pub fn prepare(root: &Path) -> RepoState {
    if !available() {
        return RepoState::unavailable(
            "git이 설치되어 있지 않아 턴마다 자동 저장하지 못합니다. git을 설치하면 되돌리기가 켜집니다.".to_string(),
        );
    }
    let toplevel = work_tree_toplevel(root);
    let settings = load_settings(root);

    match (settings, toplevel) {
        // The app already decided for this project; keep that decision.
        (Some(settings), Some(toplevel)) => RepoState {
            available: true,
            tracked: true,
            nested: !same_dir(&toplevel, root),
            origin: Some(settings.origin),
            consent: settings.consent,
            warning: None,
        },
        // Settings survived but the repository is gone (the user deleted
        // `.git`). Start over rather than trusting a decision about a history
        // that no longer exists.
        (Some(_), None) => initialize(root),
        (None, Some(toplevel)) => {
            let nested = !same_dir(&toplevel, root);
            let settings = Settings::new(RepoOrigin::Preexisting, Consent::Pending);
            let warning = save_settings(root, &settings).err();
            RepoState {
                available: true,
                tracked: true,
                nested,
                origin: Some(RepoOrigin::Preexisting),
                consent: Consent::Pending,
                warning: warning.or_else(|| {
                    Some(if nested {
                        "이 프로젝트는 이미 상위 폴더의 git 저장소 안에 있습니다. 턴마다 자동 커밋할지 확인이 필요합니다.".to_string()
                    } else {
                        "이 프로젝트는 이미 git으로 관리되고 있습니다. 턴마다 자동 커밋할지 확인이 필요합니다.".to_string()
                    })
                }),
            }
        }
        (None, None) => initialize(root),
    }
}

/// `git init` plus the app's `.gitignore` and an initial commit.
fn initialize(root: &Path) -> RepoState {
    if let Err(error) = run_ok(root, &["init"]) {
        return RepoState::unavailable(format!(
            "git 저장소를 만들지 못해 되돌리기를 켜지 못했습니다: {error}"
        ));
    }
    if let Err(error) = write_gitignore(root) {
        return RepoState::unavailable(format!(".gitignore를 쓰지 못했습니다: {error}"));
    }
    let settings = Settings::new(RepoOrigin::App, Consent::Granted);
    let mut warning = save_settings(root, &settings).err();
    if let Err(error) = commit_all(root, "프로젝트 초기 상태") {
        warning.get_or_insert(format!("초기 상태를 커밋하지 못했습니다: {error}"));
    }
    RepoState {
        available: true,
        tracked: true,
        nested: false,
        origin: Some(RepoOrigin::App),
        consent: Consent::Granted,
        warning,
    }
}

/// Record the user's answer to the "may I commit here?" question.
pub fn set_consent(root: &Path, granted: bool) -> Result<RepoState, String> {
    let mut settings = load_settings(root)
        .ok_or_else(|| "이 프로젝트의 git 상태를 아직 확인하지 않았습니다.".to_string())?;
    settings.consent = if granted {
        Consent::Granted
    } else {
        Consent::Declined
    };
    save_settings(root, &settings)?;
    Ok(prepare(root))
}

/// The work tree top level containing `root`, or `None` when `root` is not in
/// a repository.
fn work_tree_toplevel(root: &Path) -> Option<PathBuf> {
    let output = run(root, &["rev-parse", "--show-toplevel"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// Compare two directories by their canonical form so a short path, a symlink,
/// or a different separator does not read as a different directory.
fn same_dir(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Write the app's `.gitignore`, keeping whatever the file already says.
fn write_gitignore(root: &Path) -> Result<(), String> {
    let path = root.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let present: Vec<&str> = existing.lines().map(str::trim).collect();
    let missing: Vec<&str> = IGNORED
        .iter()
        .copied()
        .filter(|line| !present.contains(line))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if text.is_empty() {
        text.push_str(
            "# eud-agent가 생성한 목록입니다. 아래 경로는 다시 만들 수 있는 산출물입니다.\n",
        );
    }
    for line in missing {
        text.push_str(line);
        text.push('\n');
    }
    std::fs::write(&path, text.as_bytes()).map_err(|error| format!("{}: {error}", path.display()))
}

fn settings_path(root: &Path) -> PathBuf {
    root.join(SETTINGS_RELATIVE)
}

fn load_settings(root: &Path) -> Option<Settings> {
    let text = std::fs::read_to_string(settings_path(root)).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_settings(root: &Path, settings: &Settings) -> Result<(), String> {
    let path = settings_path(root);
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("git 설정을 직렬화하지 못했습니다: {error}"))?;
    write_atomic(&path, &bytes).map_err(|error| format!("{}: {error}", path.display()))
}

/// Temp file plus rename, so a crash never leaves half a decision behind.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    if let Err(error) = std::fs::write(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

/// Whether this project's repository already accepts the app's automatic
/// commits. A turn boundary asks this, so it prepares nothing and starts no
/// process: a project that was never prepared, or whose user declined, simply
/// does not commit.
pub fn auto_commit_ready(root: &Path) -> bool {
    available() && load_settings(root).is_some_and(|settings| settings.consent == Consent::Granted)
}

/// The project-relative paths that differ from the last commit, including
/// files git does not track yet and excluding what `.gitignore` covers.
pub fn dirty_paths(root: &Path) -> Result<Vec<String>, String> {
    let stdout = run_ok(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            ".",
        ],
    )?;
    let mut paths = Vec::new();
    let mut records = stdout.split('\0').filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        // `XY <path>`; a rename or copy puts its source in the next record.
        let Some((status, path)) = record.split_once(' ') else {
            continue;
        };
        if status.starts_with('R') || status.starts_with('C') {
            records.next();
        }
        paths.push(path.trim_start().to_string());
    }
    Ok(paths)
}

/// Commit everything that changed under `root`, or report that nothing did.
///
/// The pathspec keeps a project nested in a larger repository from committing
/// its siblings, and keeps another tool's staged work out of the app's commit.
fn commit_all(root: &Path, message: &str) -> Result<Option<CommitRecord>, String> {
    let files = dirty_paths(root)?;
    if files.is_empty() {
        return Ok(None);
    }
    run_ok(root, &["add", "--all", "--", "."])?;

    let mut args: Vec<String> = Vec::new();
    if !has_identity(root) {
        args.push("-c".into());
        args.push(format!("user.name={IDENTITY_NAME}"));
        args.push("-c".into());
        args.push(format!("user.email={IDENTITY_EMAIL}"));
    }
    // A signing key with no agent, or a hook that runs a test suite, would
    // block the turn. The app's own commits skip both; the user's do not.
    args.push("-c".into());
    args.push("commit.gpgsign=false".into());
    args.extend(
        ["commit", "--no-verify", "--message", message, "--", "."]
            .iter()
            .map(|arg| (*arg).to_string()),
    );
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run(root, &borrowed)?;
    if !output.status.success() {
        return Err(format!("커밋하지 못했습니다: {}", message_of(&output)));
    }
    let sha = run_ok(root, &["rev-parse", "HEAD"])?.trim().to_string();
    Ok(Some(CommitRecord {
        sha,
        subject: subject_of(message),
        files: files.len(),
    }))
}

/// True when this repository has at least one commit. A repository the user
/// just created has an unborn HEAD, which `log` and `revert` cannot read.
fn has_commits(root: &Path) -> bool {
    run(root, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// True when this repository already knows who commits here.
fn has_identity(root: &Path) -> bool {
    run(root, &["config", "user.email"])
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

/// Commit whatever changed outside the app before a turn starts, so the turn's
/// own commit holds only the turn's work.
///
/// This replaces the three-way merge the workspace used to run: the user's
/// external edit is not merged into the agent's view, it is recorded as its
/// own commit and the turn starts from it.
pub fn commit_external_edits(root: &Path) -> Result<Option<CommitRecord>, String> {
    commit_all(root, "외부 편집 — 에이전트 턴 밖에서 바뀐 파일")
}

/// Commit the work of one turn.
pub fn commit_turn(root: &Path, message: &str) -> Result<Option<CommitRecord>, String> {
    commit_all(root, message)
}

/// The commit message for one turn: what was asked, then which session and
/// request produced it, so a history entry can be traced back to a
/// conversation.
pub fn turn_message(summary: &str, session_id: &str, request_id: &str) -> String {
    let subject = subject_of(summary);
    format!("{subject}\n\nsession: {session_id}\nrequest: {request_id}\n")
}

/// The first line of `text`, bounded and never empty.
fn subject_of(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("에이전트 턴");
    if first.chars().count() <= SUBJECT_LIMIT {
        return first.to_string();
    }
    let kept: String = first.chars().take(SUBJECT_LIMIT - 1).collect();
    format!("{kept}…")
}

/// Undo one commit by recording its inverse, the way the user would.
pub fn revert(root: &Path, sha: &str) -> Result<CommitRecord, String> {
    if !dirty_paths(root)?.is_empty() {
        return Err(
            "저장하지 않은 변경이 남아 있어 되돌릴 수 없습니다. 먼저 현재 상태를 커밋하거나 되돌리세요."
                .to_string(),
        );
    }
    let mut args: Vec<String> = Vec::new();
    if !has_identity(root) {
        args.push("-c".into());
        args.push(format!("user.name={IDENTITY_NAME}"));
        args.push("-c".into());
        args.push(format!("user.email={IDENTITY_EMAIL}"));
    }
    args.push("-c".into());
    args.push("commit.gpgsign=false".into());
    args.extend(
        ["revert", "--no-edit", sha]
            .iter()
            .map(|arg| (*arg).to_string()),
    );
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run(root, &borrowed)?;
    if !output.status.success() {
        // A conflicted revert leaves the work tree mid-operation; put it back
        // so the project is usable whatever the user does next.
        let _ = run(root, &["revert", "--abort"]);
        return Err(format!(
            "되돌리지 못했습니다: {}. 이후 변경과 충돌하므로 직접 수정해야 합니다.",
            message_of(&output)
        ));
    }
    let head = run_ok(root, &["rev-parse", "HEAD"])?.trim().to_string();
    let subject = run_ok(root, &["log", "-1", "--format=%s"])?
        .trim()
        .to_string();
    let files = run_ok(root, &["show", "--name-only", "--format=", head.as_str()])?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    Ok(CommitRecord {
        sha: head,
        subject,
        files,
    })
}

/// The newest commits touching this project, newest first.
pub fn log(root: &Path, limit: usize) -> Result<Vec<CommitSummary>, String> {
    if !has_commits(root) {
        return Ok(Vec::new());
    }
    let limit = limit.max(1).to_string();
    let stdout = run_ok(
        root,
        &[
            "log",
            "--max-count",
            limit.as_str(),
            "--format=%H%x1f%ct%x1f%s",
            "--",
            ".",
        ],
    )?;
    Ok(stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\u{1f}');
            let sha = parts.next()?.trim().to_string();
            let timestamp = parts.next()?.trim().parse().ok()?;
            let subject = parts.next().unwrap_or_default().trim().to_string();
            Some(CommitSummary {
                sha,
                subject,
                timestamp,
            })
        })
        .collect())
}

/// How far a single file's patch is rendered before the view stops carrying it.
/// A source edit is a few KiB; a regenerated file can be megabytes, and the
/// panel does not need to show one to say it changed.
const MAX_FILE_PATCH_BYTES: usize = 64 * 1024;
/// And how much of one commit is carried in total.
const MAX_COMMIT_PATCH_BYTES: usize = 256 * 1024;

/// What happened to one file in a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFile {
    pub path: String,
    pub insertions: usize,
    pub deletions: usize,
    /// Git could not diff this file as text (a map, an image, a wheel).
    pub binary: bool,
    /// The unified diff, when it is carried.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// Why the patch is absent, in Korean, when it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted: Option<String>,
}

/// One commit as the panel shows it: what was asked, and what changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetail {
    pub sha: String,
    pub subject: String,
    /// The rest of the message — the session and request the turn belonged to.
    pub body: String,
    pub timestamp: i64,
    pub files: Vec<CommitFile>,
}

/// Everything one commit changed, with each file's patch bounded.
pub fn commit_detail(root: &Path, sha: &str) -> Result<CommitDetail, String> {
    let header = run_ok(
        root,
        &["show", "--no-patch", "--format=%H%x1f%ct%x1f%s%x1f%b", sha],
    )?;
    let mut parts = header.splitn(4, '\u{1f}');
    let resolved = parts.next().unwrap_or_default().trim().to_string();
    let timestamp = parts
        .next()
        .unwrap_or_default()
        .trim()
        .parse()
        .unwrap_or_default();
    let subject = parts.next().unwrap_or_default().trim().to_string();
    let body = parts.next().unwrap_or_default().trim().to_string();
    if resolved.is_empty() {
        return Err(format!("커밋 {sha}을(를) 찾을 수 없습니다."));
    }

    let stats = run_ok(
        root,
        &[
            "show",
            "--numstat",
            "--format=",
            "--find-renames",
            &resolved,
        ],
    )?;
    let mut budget = MAX_COMMIT_PATCH_BYTES;
    let mut files = Vec::new();
    for line in stats.lines().filter(|line| !line.trim().is_empty()) {
        let mut columns = line.split('\t');
        let added = columns.next().unwrap_or("-");
        let removed = columns.next().unwrap_or("-");
        let Some(path) = columns.next() else {
            continue;
        };
        // A rename prints `old => new`; the new name is what the user looks for.
        let path = path.rsplit(" => ").next().unwrap_or(path).trim_matches('"');
        let binary = added == "-" || removed == "-";
        let (patch, omitted) = if binary {
            (
                None,
                Some("바이너리 파일이라 내용 비교를 표시하지 않습니다.".to_string()),
            )
        } else {
            file_patch(root, &resolved, path, &mut budget)
        };
        files.push(CommitFile {
            path: path.to_string(),
            insertions: added.parse().unwrap_or_default(),
            deletions: removed.parse().unwrap_or_default(),
            binary,
            patch,
            omitted,
        });
    }

    Ok(CommitDetail {
        sha: resolved,
        subject,
        body,
        timestamp,
        files,
    })
}

/// One file's unified diff, or the reason it is not carried.
fn file_patch(
    root: &Path,
    sha: &str,
    path: &str,
    budget: &mut usize,
) -> (Option<String>, Option<String>) {
    if *budget == 0 {
        return (
            None,
            Some("이 커밋의 표시 한도를 넘어 생략했습니다.".to_string()),
        );
    }
    let patch = match run_ok(
        root,
        &["show", "--format=", "--find-renames", sha, "--", path],
    ) {
        Ok(patch) => patch,
        Err(error) => return (None, Some(error)),
    };
    if patch.len() > MAX_FILE_PATCH_BYTES {
        return (
            None,
            Some(format!(
                "변경이 너무 커서 생략했습니다 ({} KiB). 필요하면 git으로 직접 보세요.",
                patch.len() / 1024
            )),
        );
    }
    *budget = budget.saturating_sub(patch.len());
    (Some(patch), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_without_ownership_still_gets_its_exception() {
        // The first live run failed here: git refuses "dubious ownership" on a
        // volume that records none, which is every repository on it.
        let exceptions = super::safe_directory_args(Path::new(r"E:\proj\eud\rpg"));
        // git matches its own spelling of the path; a backslash form is
        // silently ignored, which is how this looked like git being broken.
        assert!(exceptions.contains(&"safe.directory=E:/proj/eud/rpg".to_string()));
        // A project inside a larger repository is covered by the ancestor that
        // actually holds the work tree.
        assert!(exceptions.contains(&"safe.directory=E:/proj/eud".to_string()));
        assert!(
            exceptions.iter().all(|entry| !entry.contains('\\')),
            "{exceptions:?}"
        );
    }

    #[test]
    fn the_extended_length_prefix_never_reaches_git() {
        assert_eq!(
            super::git_path(Path::new(r"\\?\E:\proj\eud\rpg")),
            "E:/proj/eud/rpg"
        );
    }

    /// A fresh directory for one test.
    fn temp_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("eud-agent-git-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// git is required to run the app, but a machine without it must not fail
    /// the suite — it reports the same way the product does.
    fn git_present(test: &str) -> bool {
        if available() {
            return true;
        }
        eprintln!("skipping {test}: git is not installed on this machine");
        false
    }

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_project_without_a_repository_gets_one_and_an_initial_commit() {
        if !git_present("a_project_without_a_repository_gets_one_and_an_initial_commit") {
            return;
        }
        let root = temp_root("init");
        write(&root, "src/main.eps", "const a = 1;");

        let state = prepare(&root);

        assert!(state.available);
        assert!(state.tracked);
        assert!(!state.nested);
        assert_eq!(state.origin, Some(RepoOrigin::App));
        assert_eq!(state.consent, Consent::Granted);
        assert!(state.auto_commit_allowed());
        assert!(root.join(".gitignore").is_file());
        assert_eq!(log(&root, 10).unwrap().len(), 1);
        assert!(dirty_paths(&root).unwrap().is_empty());
    }

    #[test]
    fn generated_output_and_app_state_stay_out_of_the_history() {
        if !git_present("generated_output_and_app_state_stay_out_of_the_history") {
            return;
        }
        let root = temp_root("ignore");
        write(&root, "src/main.eps", "const a = 1;");
        prepare(&root);

        write(&root, "build/out.scx", "binary");
        write(&root, "src/__epspy__/main.py", "generated");
        write(&root, "compat/editor-project.e3s", "graph");

        assert!(dirty_paths(&root).unwrap().is_empty());
        assert!(commit_turn(&root, "아무 것도 바뀌지 않음")
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_turn_commit_records_the_turn_and_carries_its_session_and_request() {
        if !git_present("a_turn_commit_records_the_turn_and_carries_its_session_and_request") {
            return;
        }
        let root = temp_root("turn");
        write(&root, "src/main.eps", "const a = 1;");
        prepare(&root);

        write(&root, "src/main.eps", "const a = 2;");
        let message = turn_message("체력을 200으로 올려줘", "session-7", "request-3");
        let record = commit_turn(&root, &message).unwrap().expect("a commit");

        assert_eq!(record.subject, "체력을 200으로 올려줘");
        assert_eq!(record.files, 1);
        let body = run_ok(&root, &["log", "-1", "--format=%B"]).unwrap();
        assert!(body.contains("session: session-7"), "{body}");
        assert!(body.contains("request: request-3"), "{body}");
        assert!(dirty_paths(&root).unwrap().is_empty());
    }

    #[test]
    fn an_external_edit_becomes_its_own_commit_before_the_turn() {
        if !git_present("an_external_edit_becomes_its_own_commit_before_the_turn") {
            return;
        }
        let root = temp_root("external");
        write(&root, "src/main.eps", "const a = 1;");
        write(&root, "maps/source.scx", "map v1");
        prepare(&root);

        // The user edits the map in SCMDraft while the app is open.
        write(&root, "maps/source.scx", "map v2");
        let external = commit_external_edits(&root).unwrap().expect("a commit");
        assert_eq!(external.files, 1);

        // Only then does the turn run and commit its own work.
        write(&root, "src/main.eps", "const a = 2;");
        let turn = commit_turn(&root, turn_message("소스 수정", "s", "r").as_str())
            .unwrap()
            .expect("a commit");

        assert_ne!(external.sha, turn.sha);
        let history = log(&root, 10).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].subject, "소스 수정");
        assert!(history[1].subject.starts_with("외부 편집"));
        let files = run_ok(
            &root,
            &["show", "--name-only", "--format=", turn.sha.as_str()],
        )
        .unwrap();
        assert!(files.contains("src/main.eps"), "{files}");
        assert!(!files.contains("maps/source.scx"), "{files}");
    }

    #[test]
    fn nothing_changed_means_no_commit() {
        if !git_present("nothing_changed_means_no_commit") {
            return;
        }
        let root = temp_root("clean");
        write(&root, "src/main.eps", "const a = 1;");
        prepare(&root);

        assert!(commit_external_edits(&root).unwrap().is_none());
        assert!(commit_turn(&root, "읽기만 한 턴").unwrap().is_none());
        assert_eq!(log(&root, 10).unwrap().len(), 1);
    }

    #[test]
    fn a_preexisting_repository_is_not_committed_into_before_consent() {
        if !git_present("a_preexisting_repository_is_not_committed_into_before_consent") {
            return;
        }
        let root = temp_root("existing");
        write(&root, "src/main.eps", "const a = 1;");
        run_ok(&root, &["init"]).unwrap();

        let state = prepare(&root);
        assert_eq!(state.origin, Some(RepoOrigin::Preexisting));
        assert_eq!(state.consent, Consent::Pending);
        assert!(!state.auto_commit_allowed());
        assert!(state.warning.is_some());
        // Nothing was committed and no `.gitignore` was imposed.
        assert!(log(&root, 10).unwrap().is_empty());
        assert!(!root.join(".gitignore").exists());

        let declined = set_consent(&root, false).unwrap();
        assert_eq!(declined.consent, Consent::Declined);
        assert!(!declined.auto_commit_allowed());

        let granted = set_consent(&root, true).unwrap();
        assert_eq!(granted.consent, Consent::Granted);
        assert!(granted.auto_commit_allowed());
        // The decision survives a later open.
        assert!(prepare(&root).auto_commit_allowed());
    }

    #[test]
    fn a_project_inside_a_larger_repository_is_reported_as_nested() {
        if !git_present("a_project_inside_a_larger_repository_is_reported_as_nested") {
            return;
        }
        let parent = temp_root("parent");
        run_ok(&parent, &["init"]).unwrap();
        let root = parent.join("projects/one");
        std::fs::create_dir_all(&root).unwrap();
        write(&root, "src/main.eps", "const a = 1;");

        let state = prepare(&root);

        assert!(state.tracked);
        assert!(state.nested);
        assert_eq!(state.origin, Some(RepoOrigin::Preexisting));
        assert_eq!(state.consent, Consent::Pending);
    }

    #[test]
    fn a_nested_project_commits_only_its_own_files() {
        if !git_present("a_nested_project_commits_only_its_own_files") {
            return;
        }
        let parent = temp_root("nested-commit");
        run_ok(&parent, &["init"]).unwrap();
        write(&parent, "sibling.txt", "not the project");
        let root = parent.join("projects/one");
        std::fs::create_dir_all(&root).unwrap();
        write(&root, "src/main.eps", "const a = 1;");
        prepare(&root);
        set_consent(&root, true).unwrap();

        let record = commit_turn(&root, "프로젝트만 커밋")
            .unwrap()
            .expect("a commit");

        let files = run_ok(
            &root,
            &["show", "--name-only", "--format=", record.sha.as_str()],
        )
        .unwrap();
        assert!(files.contains("src/main.eps"), "{files}");
        assert!(!files.contains("sibling.txt"), "{files}");
    }

    #[test]
    fn a_commit_carries_its_message_and_one_patch_per_file() {
        if !git_present("a_commit_carries_its_message_and_one_patch_per_file") {
            return;
        }
        let root = temp_root("detail");
        write(&root, "src/main.eps", "const a = 1;\n");
        prepare(&root);

        write(&root, "src/main.eps", "const a = 2;\n");
        write(&root, "src/added.eps", "const b = 1;\n");
        let record = commit_turn(&root, turn_message("체력 수정", "s-1", "r-1").as_str())
            .unwrap()
            .expect("a commit");

        let detail = commit_detail(&root, &record.sha).unwrap();

        assert_eq!(detail.sha, record.sha);
        assert_eq!(detail.subject, "체력 수정");
        assert!(detail.body.contains("session: s-1"), "{}", detail.body);
        assert!(detail.timestamp > 0);
        let mut paths = detail
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        paths.sort_unstable();
        assert_eq!(paths, ["src/added.eps", "src/main.eps"]);
        let edited = detail
            .files
            .iter()
            .find(|file| file.path == "src/main.eps")
            .unwrap();
        assert_eq!((edited.insertions, edited.deletions), (1, 1));
        assert!(!edited.binary);
        let patch = edited
            .patch
            .as_deref()
            .expect("a text file carries a patch");
        assert!(patch.contains("-const a = 1;"), "{patch}");
        assert!(patch.contains("+const a = 2;"), "{patch}");
        assert!(edited.omitted.is_none());
    }

    /// A map is the thing the user most wants to see changed and the thing a
    /// text diff says least about, so it is reported as changed and not shown.
    #[test]
    fn a_binary_file_is_reported_as_changed_without_a_patch() {
        if !git_present("a_binary_file_is_reported_as_changed_without_a_patch") {
            return;
        }
        let root = temp_root("binary");
        std::fs::create_dir_all(root.join("maps")).unwrap();
        std::fs::write(root.join("maps/source.scx"), [0_u8, 1, 2, 0, 255]).unwrap();
        prepare(&root);

        std::fs::write(root.join("maps/source.scx"), [0_u8, 9, 9, 0, 1]).unwrap();
        let record = commit_turn(&root, "맵 변경").unwrap().expect("a commit");

        let detail = commit_detail(&root, &record.sha).unwrap();
        let map = detail
            .files
            .iter()
            .find(|file| file.path == "maps/source.scx")
            .unwrap();

        assert!(map.binary);
        assert!(map.patch.is_none());
        assert!(map.omitted.as_deref().unwrap().contains("바이너리"));
    }

    #[test]
    fn a_file_whose_diff_is_too_large_says_so_instead_of_carrying_it() {
        if !git_present("a_file_whose_diff_is_too_large_says_so_instead_of_carrying_it") {
            return;
        }
        let root = temp_root("huge");
        write(&root, "src/main.eps", "const a = 1;\n");
        prepare(&root);

        let huge = (0..20_000)
            .map(|index| format!("const v{index} = {index};\n"))
            .collect::<String>();
        write(&root, "src/main.eps", &huge);
        let record = commit_turn(&root, "대량 생성").unwrap().expect("a commit");

        let detail = commit_detail(&root, &record.sha).unwrap();
        let file = &detail.files[0];

        assert!(file.patch.is_none());
        assert!(file.omitted.as_deref().unwrap().contains("너무 커서"));
        // The counts still tell the user how much moved.
        assert!(file.insertions >= 20_000, "{}", file.insertions);
    }

    #[test]
    fn an_unknown_commit_is_refused_by_name() {
        if !git_present("an_unknown_commit_is_refused_by_name") {
            return;
        }
        let root = temp_root("unknown");
        write(&root, "src/main.eps", "const a = 1;\n");
        prepare(&root);

        let error = commit_detail(&root, "0000000000000000000000000000000000000000").unwrap_err();

        assert!(!error.is_empty());
    }

    #[test]
    fn revert_restores_the_state_before_a_turn() {
        if !git_present("revert_restores_the_state_before_a_turn") {
            return;
        }
        let root = temp_root("revert");
        write(&root, "src/main.eps", "const a = 1;");
        prepare(&root);

        write(&root, "src/main.eps", "const a = 2;");
        write(&root, "src/added.eps", "const b = 1;");
        let turn = commit_turn(&root, turn_message("두 파일 수정", "s", "r").as_str())
            .unwrap()
            .expect("a commit");

        revert(&root, &turn.sha).unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join("src/main.eps")).unwrap(),
            "const a = 1;"
        );
        assert!(!root.join("src/added.eps").exists());
        assert!(dirty_paths(&root).unwrap().is_empty());
    }

    #[test]
    fn revert_refuses_while_the_work_tree_has_uncommitted_changes() {
        if !git_present("revert_refuses_while_the_work_tree_has_uncommitted_changes") {
            return;
        }
        let root = temp_root("revert-dirty");
        write(&root, "src/main.eps", "const a = 1;");
        prepare(&root);
        write(&root, "src/main.eps", "const a = 2;");
        let turn = commit_turn(&root, "수정").unwrap().expect("a commit");
        write(&root, "src/main.eps", "const a = 3;");

        let error = revert(&root, &turn.sha).unwrap_err();

        assert!(error.contains("저장하지 않은 변경"), "{error}");
        assert_eq!(
            std::fs::read_to_string(root.join("src/main.eps")).unwrap(),
            "const a = 3;"
        );
    }

    #[test]
    fn a_long_request_is_shortened_into_one_subject_line() {
        let summary = "가".repeat(200);
        let message = turn_message(&summary, "s", "r");
        let subject = message.lines().next().unwrap();

        assert_eq!(subject.chars().count(), SUBJECT_LIMIT);
        assert!(subject.ends_with('…'));
        assert!(message.contains("session: s"));
    }

    #[test]
    fn an_empty_request_still_produces_a_subject() {
        let message = turn_message("  \n\n", "s", "r");
        assert_eq!(message.lines().next().unwrap(), "에이전트 턴");
    }

    #[test]
    fn the_generated_gitignore_keeps_what_the_project_already_ignored() {
        let root = temp_root("gitignore");
        std::fs::write(root.join(".gitignore"), "node_modules/\nbuild/\n").unwrap();

        write_gitignore(&root).unwrap();

        let text = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(text.starts_with("node_modules/\nbuild/\n"), "{text}");
        assert_eq!(text.matches("build/").count(), 1, "{text}");
        assert!(text.contains(".eud-agent/state/"), "{text}");
        assert!(text.contains("compat/"), "{text}");
    }
}
