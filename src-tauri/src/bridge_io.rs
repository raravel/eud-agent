//! File-IPC client for the EUD Editor 3 Lua bridge.
//!
//! The app writes `srv-<id8>.cmd` into the editor's `Data\agent\inbox` and polls
//! `outbox` for the matching `.result` file. Files are raw UTF-8 bytes without a BOM,
//! and command writes are atomic so the Lua bridge never reads a partial `.cmd`.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use uuid::Uuid;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(180);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(200);
const EPSNAPSHOT_TIMEOUT: Duration = Duration::from_secs(180);
const MAP_SOURCE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);
/// Heartbeat freshness window used by app-facing editor liveness checks.
///
/// The Lua bridge writes `heartbeat.txt` on roughly every 1s UI tick, so a 3s window
/// tolerates short scheduling delays while still surfacing a disconnected editor quickly.
pub const HEARTBEAT_STALE_AFTER: Duration = Duration::from_secs(3);
const EDITOR_NOT_CONNECTED_MESSAGE: &str = "editor not connected";
const SETTABLE_FAMILIES: [&str; 2] = ["CUI", "RAWTEXT"];
const DAT_NAMES: [&str; 10] = [
    "units", "weapons", "flingy", "sprites", "images", "upgrades", "techdata", "orders",
    "portdata", "sfxdata",
];
const EPSNAPSHOT_MANIFEST_MAX_BYTES: u64 = 8 * 1024 * 1024;
const BRIDGE_SESSION_MARKER_MAX_BYTES: u64 = 4 * 1024;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// File-IPC client bound to the editor's `Data\agent` directory.
#[derive(Debug, Clone)]
pub struct BridgeIo {
    data_dir: PathBuf,
    inbox: PathBuf,
    outbox: PathBuf,
    status_file: PathBuf,
    heartbeat_file: PathBuf,
}

/// Polling and busy-editor timeout settings for a bridge request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendOpts {
    pub timeout: Duration,
    pub busy_timeout: Duration,
    pub poll_interval: Duration,
}

impl SendOpts {
    /// Timeout policy for a full-project EPSNAPSHOT scan.
    pub fn for_epsnapshot() -> Self {
        Self {
            timeout: EPSNAPSHOT_TIMEOUT,
            ..Self::default()
        }
    }

    /// Short timeout for explicit saved-map confirmation. Passive Map Agent
    /// source monitoring reads `status.txt` and never uses bridge commands.
    pub fn for_map_source_confirmation() -> Self {
        Self {
            timeout: MAP_SOURCE_CONFIRM_TIMEOUT,
            busy_timeout: MAP_SOURCE_CONFIRM_TIMEOUT,
            ..Self::default()
        }
    }
}

impl Default for SendOpts {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

/// Parsed entry from the bridge `LIST` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub ftype: String,
    pub settable: bool,
}

/// Editor status read directly from `status.txt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSnapshot {
    /// True while EUD Editor is compiling.
    pub compiling: bool,
    /// Current project line from the editor status file.
    pub project: String,
    /// Saved OpenMapName from the same idle editor tick. `None` means the loaded
    /// bridge predates this status contract.
    pub open_map_name: Option<String>,
}
/// One `.eps` project entry from a coherent editor snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpsSnapshotFile {
    pub path: String,
    pub ftype: String,
    /// `None` means the bridge could enumerate the file but could not read it.
    pub content: Option<String>,
}

/// Coherent `.eps` project snapshot produced by one editor UI-thread Tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpsSnapshot {
    pub project: String,
    /// Full project identity used only for stable mirror ownership.
    pub identity: String,
    pub files: Vec<EpsSnapshotFile>,
}

/// Errors returned by the file-IPC bridge client.
#[derive(Debug)]
pub enum BridgeError {
    /// The editor bridge heartbeat is absent or stale.
    EditorNotConnected,
    /// The bridge returned an `ERROR:`-prefixed reply.
    Error(String),
    /// The bridge did not answer before the selected timeout window.
    Busy(String),
    /// Filesystem error while writing, polling, or cleaning IPC files.
    Io(io::Error),
    /// Snapshot manifest or ordinal content failed validation.
    InvalidSnapshot(String),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EditorNotConnected => f.write_str(EDITOR_NOT_CONNECTED_MESSAGE),
            Self::Error(message) | Self::Busy(message) | Self::InvalidSnapshot(message) => {
                f.write_str(message)
            }
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl Error for BridgeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::EditorNotConnected
            | Self::Error(_)
            | Self::Busy(_)
            | Self::InvalidSnapshot(_) => None,
        }
    }
}

impl From<io::Error> for BridgeError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl BridgeIo {
    /// Create a bridge client rooted at the editor's `Data\agent` directory.
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        let data_dir = data_dir.as_ref().to_path_buf();
        Self {
            inbox: data_dir.join("inbox"),
            outbox: data_dir.join("outbox"),
            status_file: data_dir.join("status.txt"),
            heartbeat_file: data_dir.join("heartbeat.txt"),
            data_dir,
        }
    }

    /// The editor `Data\agent` directory this client is bound to.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Write a command, poll for its result, and return the bridge reply text.
    ///
    /// The `.cmd` is left in place on timeout so the Lua bridge can still process it once
    /// the editor leaves a compiling state. The `.result` is deleted by this reader after
    /// a successful consume.
    pub fn send(
        &self,
        command_text: &str,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<String, BridgeError> {
        let id = id8();
        let cmd_path = self.inbox.join(format!("srv-{id}.cmd"));
        let result_path = self.outbox.join(format!("srv-{id}.result"));

        self.write_cmd(&cmd_path, command_text)?;

        let start = Instant::now();
        let mut busy_notified = false;
        let mut poll_state = ConsumePollState::default();

        loop {
            match self.consume_result(&result_path, poll_state)? {
                ConsumeResult::Ready(reply) => {
                    if reply.starts_with("ERROR:") {
                        return Err(BridgeError::Error(reply.trim().to_string()));
                    }
                    return Ok(reply);
                }
                ConsumeResult::Pending { state } => {
                    poll_state = state;
                }
            }

            let compiling = self.is_compiling();
            if compiling && !busy_notified {
                busy_notified = true;
                if let Some(callback) = on_busy {
                    callback();
                }
            }

            let window = if compiling || busy_notified {
                opts.busy_timeout
            } else {
                opts.timeout
            };
            if start.elapsed() >= window {
                return Err(BridgeError::Busy(format!(
                    "bridge did not answer srv-{id} within {:.1}s (compiling={compiling})",
                    window.as_secs_f64()
                )));
            }

            thread::sleep(opts.poll_interval);
        }
    }

    /// Liveness check; the bridge replies with `PONG ...`.
    pub fn ping(&self, opts: &SendOpts, on_busy: Option<&dyn Fn()>) -> Result<String, BridgeError> {
        self.send("PING", opts, on_busy)
    }

    /// Editor state as raw `compiling` / `project` / `version` lines.
    pub fn status(
        &self,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<String, BridgeError> {
        self.send("STATUS", opts, on_busy)
    }

    /// Configured EUD Editor start file as its exact project-relative path.
    ///
    /// An empty successful reply and the expected no-project state both mean that
    /// no MainFile is configured. All other bridge failures remain visible.
    pub fn get_main(
        &self,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<Option<String>, BridgeError> {
        match self.send("GETMAIN", opts, on_busy) {
            Ok(reply) => {
                let path = reply.trim();
                Ok((!path.is_empty()).then(|| path.to_string()))
            }
            Err(BridgeError::Error(message)) if message == "ERROR: no project" => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Read editor status directly from `status.txt` after validating heartbeat freshness.
    pub fn read_status_snapshot(
        &self,
        stale_after: Duration,
    ) -> Result<StatusSnapshot, BridgeError> {
        self.read_status_snapshot_at(SystemTime::now(), stale_after)
    }

    /// Read editor status directly from `status.txt` using an injected clock for tests.
    pub fn read_status_snapshot_at(
        &self,
        now: SystemTime,
        stale_after: Duration,
    ) -> Result<StatusSnapshot, BridgeError> {
        self.ensure_heartbeat_fresh_at(now, stale_after)?;

        let text = fs::read_to_string(&self.status_file)?;
        let mut compiling = false;
        let mut project = String::new();
        let mut open_map_name = None;
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key.trim().eq_ignore_ascii_case("compiling") {
                compiling = value.trim().eq_ignore_ascii_case("true");
            } else if key.trim().eq_ignore_ascii_case("project") {
                project = value.trim().to_string();
            } else if key.trim().eq_ignore_ascii_case("openMapName") {
                open_map_name = Some(value.trim().to_string());
            }
        }

        Ok(StatusSnapshot {
            compiling,
            project,
            open_map_name,
        })
    }

    /// Project file tree parsed from `path\t<EFileType>` bridge lines.
    pub fn list(
        &self,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<Vec<FileEntry>, BridgeError> {
        let reply = self.send("LIST", opts, on_busy)?;
        let mut files = Vec::new();
        for line in reply.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            let (path, ftype) = match line.split_once('\t') {
                Some((path, ftype)) => (path, ftype),
                None => (line, ""),
            };
            files.push(FileEntry {
                path: path.to_string(),
                ftype: ftype.to_string(),
                settable: settable_for(ftype),
            });
        }
        Ok(files)
    }

    /// List project files only when the editor heartbeat is fresh.
    pub fn list_connected(
        &self,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
        stale_after: Duration,
    ) -> Result<Vec<FileEntry>, BridgeError> {
        self.list_connected_at(opts, on_busy, SystemTime::now(), stale_after)
    }

    /// List project files only when the editor heartbeat is fresh, using an injected clock.
    pub fn list_connected_at(
        &self,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
        now: SystemTime,
        stale_after: Duration,
    ) -> Result<Vec<FileEntry>, BridgeError> {
        self.ensure_heartbeat_fresh_at(now, stale_after)?;
        self.list(opts, on_busy)
    }

    /// Read a project file by path.
    pub fn get(
        &self,
        path: &str,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<String, BridgeError> {
        self.send(&format!("GET {path}"), opts, on_busy)
    }
    /// Snapshot every `.eps` file in one idle editor Tick and consume the
    /// request-owned ordinal directory after validating its manifest.
    pub fn snapshot_eps(
        &self,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<EpsSnapshot, BridgeError> {
        let token = Uuid::new_v4().hyphenated().to_string();
        let snapshot_dir = self.outbox.join(format!("epsnapshot-{token}"));
        if let Err(error) = self.send(&format!("EPSNAPSHOT {token}"), opts, on_busy) {
            let _ = fs::remove_dir_all(&snapshot_dir);
            return Err(error);
        }

        let decoded = decode_eps_snapshot(&self.outbox, &snapshot_dir, &token);
        let cleanup = fs::remove_dir_all(&snapshot_dir);
        match (decoded, cleanup) {
            (Ok(snapshot), Ok(())) => Ok(snapshot),
            (Ok(_), Err(error)) => Err(BridgeError::Io(error)),
            (Err(error), _) => Err(error),
        }
    }

    /// Replace a CUI/RawText file's in-memory text.
    pub fn set(
        &self,
        path: &str,
        code: &str,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<String, BridgeError> {
        self.send(&format!("SET {path}\n{code}"), opts, on_busy)
    }

    /// Create a new root-folder eps file.
    pub fn neweps(
        &self,
        name: &str,
        code: &str,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<String, BridgeError> {
        self.send(&format!("NEWEPS {name}\n{code}"), opts, on_busy)
    }

    /// Read a standard dat field.
    pub fn getdat(
        &self,
        dat: &str,
        param: &str,
        obj_id: impl TryInto<i64>,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<String, BridgeError> {
        require_dat_name(dat)?;
        let obj_id = require_nonnegative_int(obj_id, "objId")?;
        self.send(&format!("GETDAT {dat}|{param}|{obj_id}"), opts, on_busy)
    }

    /// Write a standard dat field. The value is validated as numeric before sending.
    pub fn setdat(
        &self,
        dat: &str,
        param: &str,
        obj_id: impl TryInto<i64>,
        value: impl ToString,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<String, BridgeError> {
        require_dat_name(dat)?;
        let obj_id = require_nonnegative_int(obj_id, "objId")?;
        let value = require_numeric_value(value, "value")?;
        self.send(
            &format!("SETDAT {dat}|{param}|{obj_id}|{value}"),
            opts,
            on_busy,
        )
    }

    /// Start an editor build.
    pub fn build(
        &self,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<String, BridgeError> {
        self.send("BUILD", opts, on_busy)
    }

    /// Run arbitrary Lua bridge code.
    pub fn lua(
        &self,
        code: &str,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<String, BridgeError> {
        self.send(&format!("LUA\n{code}"), opts, on_busy)
    }

    /// Remove stale server-owned IPC files and snapshot directories at startup.
    ///
    /// Only `srv-*.cmd`, `srv-*.result`, and immediate `epsnapshot-*`
    /// directories are removed. Legacy `agent_*` files are never touched, and
    /// missing inbox/outbox dirs are tolerated.
    pub fn cleanup_stale(&self) {
        remove_matching(&self.inbox, "srv-", ".cmd");
        remove_matching(&self.outbox, "srv-", ".result");
        remove_snapshot_dirs(&self.outbox);
    }

    fn write_cmd(&self, cmd_path: &Path, command_text: &str) -> Result<(), BridgeError> {
        fs::create_dir_all(&self.inbox)?;
        let file_name = cmd_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid command path"))?;
        let tmp_path = cmd_path.with_file_name(format!("{file_name}.tmp"));
        fs::write(&tmp_path, command_text.as_bytes())?;
        fs::rename(&tmp_path, cmd_path)?;
        Ok(())
    }

    fn ensure_heartbeat_fresh_at(
        &self,
        now: SystemTime,
        stale_after: Duration,
    ) -> Result<(), BridgeError> {
        let modified = fs::metadata(&self.heartbeat_file)
            .and_then(|metadata| metadata.modified())
            .map_err(|_| BridgeError::EditorNotConnected)?;
        let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
        if age > stale_after {
            Err(BridgeError::EditorNotConnected)
        } else {
            Ok(())
        }
    }

    fn consume_result(
        &self,
        result_path: &Path,
        state: ConsumePollState,
    ) -> Result<ConsumeResult, BridgeError> {
        if !result_path.is_file() {
            return Ok(ConsumeResult::Pending {
                state: ConsumePollState::default(),
            });
        }

        let bytes = match fs::read(result_path) {
            Ok(bytes) => bytes,
            Err(error) if is_transient_read_error(error.kind()) => {
                return Ok(ConsumeResult::Pending {
                    state: ConsumePollState::default(),
                });
            }
            Err(error) => return Err(BridgeError::Io(error)),
        };

        if bytes.is_empty() {
            if !state.empty_seen {
                return Ok(ConsumeResult::Pending {
                    state: ConsumePollState {
                        empty_seen: true,
                        last_non_empty_len: None,
                    },
                });
            }
        } else if state.last_non_empty_len != Some(bytes.len()) {
            return Ok(ConsumeResult::Pending {
                state: ConsumePollState {
                    empty_seen: false,
                    last_non_empty_len: Some(bytes.len()),
                },
            });
        }

        let text = String::from_utf8(bytes)
            .map_err(|error| BridgeError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
        match fs::remove_file(result_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(BridgeError::Io(error)),
        }
        Ok(ConsumeResult::Ready(text))
    }

    fn is_compiling(&self) -> bool {
        let Ok(text) = fs::read_to_string(&self.status_file) else {
            return false;
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key.trim().eq_ignore_ascii_case("compiling") {
                return value.trim().eq_ignore_ascii_case("true");
            }
        }
        false
    }
}
fn decode_eps_snapshot(
    outbox: &Path,
    snapshot_dir: &Path,
    expected_token: &str,
) -> Result<EpsSnapshot, BridgeError> {
    let parsed_token = Uuid::parse_str(expected_token)
        .map_err(|_| invalid_snapshot("request token is not a UUID"))?;
    if parsed_token.hyphenated().to_string() != expected_token {
        return Err(invalid_snapshot(
            "request token is not normalized lowercase",
        ));
    }

    let outbox_root = fs::canonicalize(outbox)?;
    let snapshot_root = fs::canonicalize(snapshot_dir)?;
    let expected_directory_name = format!("epsnapshot-{expected_token}");
    if snapshot_root.parent() != Some(outbox_root.as_path())
        || snapshot_root.file_name().and_then(|name| name.to_str())
            != Some(expected_directory_name.as_str())
    {
        return Err(invalid_snapshot(
            "snapshot directory escapes the bridge outbox",
        ));
    }

    let manifest_path = fs::canonicalize(snapshot_root.join("manifest.tsv"))?;
    if manifest_path.parent() != Some(snapshot_root.as_path()) {
        return Err(invalid_snapshot(
            "snapshot manifest escapes its request directory",
        ));
    }
    let metadata = fs::metadata(&manifest_path)?;
    if metadata.len() > EPSNAPSHOT_MANIFEST_MAX_BYTES {
        return Err(invalid_snapshot("snapshot manifest exceeds its size limit"));
    }
    let manifest_bytes = fs::read(&manifest_path)?;
    if manifest_bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(invalid_snapshot(
            "snapshot manifest must be UTF-8 without BOM",
        ));
    }
    let manifest = String::from_utf8(manifest_bytes)
        .map_err(|_| invalid_snapshot("snapshot manifest is not valid UTF-8"))?;
    let lines: Vec<&str> = manifest.lines().collect();
    if lines.len() < 4 || lines[0].trim_end_matches('\r') != "EUD-EPSNAPSHOT\t1" {
        return Err(invalid_snapshot("snapshot manifest header is invalid"));
    }

    let token_fields: Vec<&str> = lines[1].trim_end_matches('\r').split('\t').collect();
    if token_fields.as_slice() != ["token", expected_token] {
        return Err(invalid_snapshot(
            "snapshot manifest token does not match the request",
        ));
    }
    let project_fields: Vec<&str> = lines[2].trim_end_matches('\r').split('\t').collect();
    if project_fields.len() != 2 || project_fields[0] != "project" {
        return Err(invalid_snapshot(
            "snapshot project display name is malformed",
        ));
    }
    let mut project = decode_base64_utf8(project_fields[1], "project display name")?;
    let identity_fields: Vec<&str> = lines[3].trim_end_matches('\r').split('\t').collect();
    if identity_fields.len() != 2 || identity_fields[0] != "identity" {
        return Err(invalid_snapshot("snapshot project identity is malformed"));
    }
    let mut identity = decode_base64_utf8(identity_fields[1], "project identity")?;
    let used_legacy_untitled_identity = identity.trim().is_empty();
    if used_legacy_untitled_identity {
        identity = legacy_untitled_identity(&outbox_root)?;
    }
    if project.trim().is_empty() {
        if used_legacy_untitled_identity {
            project = "Untitled".to_string();
        } else {
            project = identity
                .split('\n')
                .find(|value| !value.trim().is_empty())
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
        }
    }
    if project.is_empty() {
        return Err(invalid_snapshot("snapshot project display name is empty"));
    }

    let mut paths = HashMap::<String, String>::new();
    let mut files = Vec::new();
    for (index, line) in lines.iter().skip(4).enumerate() {
        let fields: Vec<&str> = line.trim_end_matches('\r').split('\t').collect();
        if fields.len() != 6 || fields[0] != "file" {
            return Err(invalid_snapshot("snapshot file row is malformed"));
        }
        let ordinal = fields[1]
            .parse::<usize>()
            .map_err(|_| invalid_snapshot("snapshot ordinal is not numeric"))?;
        if ordinal != index + 1 || ordinal > 999_999 {
            return Err(invalid_snapshot(
                "snapshot ordinals must be unique and contiguous from one",
            ));
        }
        if fields[2].is_empty() {
            return Err(invalid_snapshot("snapshot file type is empty"));
        }
        let project_path = decode_base64_utf8(fields[3], "project path")?;
        let project_path = crate::eps_preflight::normalize_editor_path(&project_path)
            .map_err(|error| invalid_snapshot(&error))?;
        let key = project_path.to_lowercase();
        if let Some(previous) = paths.insert(key, project_path.clone()) {
            return Err(invalid_snapshot(&format!(
                "snapshot paths collide case-insensitively: {previous} and {project_path}"
            )));
        }
        let declared_length = fields[4]
            .parse::<usize>()
            .map_err(|_| invalid_snapshot("snapshot byte length is not numeric"))?;
        let content = match fields[5] {
            "ok" => {
                let ordinal_path = snapshot_root.join(format!("{ordinal:06}.eps"));
                let canonical = fs::canonicalize(&ordinal_path)?;
                if canonical.parent() != Some(snapshot_root.as_path()) {
                    return Err(invalid_snapshot(
                        "snapshot ordinal escapes its request directory",
                    ));
                }
                let bytes = fs::read(canonical)?;
                if bytes.len() != declared_length {
                    return Err(invalid_snapshot(
                        "snapshot ordinal byte length does not match",
                    ));
                }
                if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
                    return Err(invalid_snapshot(
                        "snapshot content must be UTF-8 without BOM",
                    ));
                }
                Some(
                    String::from_utf8(bytes)
                        .map_err(|_| invalid_snapshot("snapshot content is not valid UTF-8"))?,
                )
            }
            "unreadable" if declared_length == 0 => None,
            "unreadable" => {
                return Err(invalid_snapshot(
                    "unreadable snapshot rows must declare zero bytes",
                ))
            }
            _ => return Err(invalid_snapshot("snapshot read status is invalid")),
        };
        files.push(EpsSnapshotFile {
            path: project_path,
            ftype: fields[2].to_string(),
            content,
        });
    }

    Ok(EpsSnapshot {
        project,
        identity,
        files,
    })
}

fn legacy_untitled_identity(outbox_root: &Path) -> Result<String, BridgeError> {
    let data_dir = outbox_root
        .parent()
        .ok_or_else(|| invalid_snapshot("bridge outbox has no data directory"))?;
    let marker_path = fs::canonicalize(data_dir.join("bridge_loaded.txt")).map_err(|_| {
        invalid_snapshot(
            "snapshot project identity is empty and bridge session marker is unavailable",
        )
    })?;
    if marker_path.parent() != Some(data_dir) {
        return Err(invalid_snapshot(
            "bridge session marker escapes the bridge data directory",
        ));
    }
    let metadata = fs::metadata(&marker_path)?;
    if metadata.len() == 0 || metadata.len() > BRIDGE_SESSION_MARKER_MAX_BYTES {
        return Err(invalid_snapshot(
            "bridge session marker has an invalid size",
        ));
    }
    let marker = String::from_utf8(fs::read(marker_path)?)
        .map_err(|_| invalid_snapshot("bridge session marker is not valid UTF-8"))?;
    let marker = marker.trim();
    if marker.is_empty() {
        return Err(invalid_snapshot("bridge session marker is empty"));
    }

    Ok(format!(
        "legacy-untitled\n{}\n{marker}",
        data_dir.to_string_lossy()
    ))
}

fn decode_base64_utf8(value: &str, label: &str) -> Result<String, BridgeError> {
    let bytes = BASE64_STANDARD
        .decode(value)
        .map_err(|_| invalid_snapshot(&format!("snapshot {label} is not valid base64")))?;
    String::from_utf8(bytes)
        .map_err(|_| invalid_snapshot(&format!("snapshot {label} is not valid UTF-8")))
}

fn invalid_snapshot(message: &str) -> BridgeError {
    BridgeError::InvalidSnapshot(format!("invalid EPSNAPSHOT: {message}"))
}

enum ConsumeResult {
    Ready(String),
    Pending { state: ConsumePollState },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ConsumePollState {
    empty_seen: bool,
    last_non_empty_len: Option<usize>,
}

fn id8() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    if NEXT_ID.load(Ordering::Relaxed) == 0 {
        let seed = nanos.max(1);
        let _ = NEXT_ID.compare_exchange(0, seed, Ordering::Relaxed, Ordering::Relaxed);
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{:08x}", id as u32)
}

fn settable_for(ftype: &str) -> bool {
    let upper = ftype.to_ascii_uppercase();
    SETTABLE_FAMILIES
        .iter()
        .any(|family| upper.contains(family))
}

fn require_dat_name(dat: &str) -> Result<(), BridgeError> {
    if DAT_NAMES.contains(&dat) {
        Ok(())
    } else {
        Err(BridgeError::Error(format!(
            "ERROR: invalid dat name {dat:?} (one of {})",
            DAT_NAMES.join(", ")
        )))
    }
}

fn require_nonnegative_int(value: impl TryInto<i64>, label: &str) -> Result<i64, BridgeError> {
    let value = value
        .try_into()
        .map_err(|_| BridgeError::Error(format!("ERROR: {label} must be an integer in range")))?;
    if value < 0 {
        Err(BridgeError::Error(format!(
            "ERROR: {label} must be non-negative, got {value}"
        )))
    } else {
        Ok(value)
    }
}

fn require_numeric_value(value: impl ToString, label: &str) -> Result<String, BridgeError> {
    let value = value.to_string();
    if parse_numeric_i64(&value).is_some() {
        Ok(value)
    } else {
        Err(BridgeError::Error(format!(
            "ERROR: {label} must be numeric, got {value:?}"
        )))
    }
}

fn parse_numeric_i64(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    let unsigned = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"));
    if let Some(hex) = unsigned {
        i64::from_str_radix(hex, 16).ok()
    } else if let Some(hex) = trimmed
        .strip_prefix("-0x")
        .or_else(|| trimmed.strip_prefix("-0X"))
    {
        i64::from_str_radix(hex, 16).ok().map(|n| -n)
    } else {
        trimmed.parse::<i64>().ok()
    }
}

fn remove_matching(dir: &Path, prefix: &str, suffix: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with(prefix) && name.ends_with(suffix) {
            let _ = fs::remove_file(path);
        }
    }
}

fn remove_snapshot_dirs(outbox: &Path) {
    let Ok(entries) = fs::read_dir(outbox) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let is_directory = entry
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false);
        if is_directory && name.starts_with("epsnapshot-") {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn is_transient_read_error(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::NotFound
            | io::ErrorKind::PermissionDenied
            | io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
    )
}

#[cfg(test)]
mod tests {
    use super::{
        decode_eps_snapshot, BridgeError, BridgeIo, ConsumePollState, ConsumeResult, SendOpts,
        BASE64_STANDARD,
    };
    use base64::Engine;
    use parking_lot::Mutex;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    type SeenLog = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    /// Unique temp base dir for a test, avoiding a `tempfile` dev-dependency
    /// (Cargo.toml is out of scope for this task).
    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-bridge-io-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fast_opts() -> SendOpts {
        SendOpts {
            timeout: Duration::from_millis(500),
            busy_timeout: Duration::from_millis(500),
            poll_interval: Duration::from_millis(10),
        }
    }

    fn short_busy_opts() -> SendOpts {
        SendOpts {
            timeout: Duration::from_millis(20),
            busy_timeout: Duration::from_millis(60),
            poll_interval: Duration::from_millis(10),
        }
    }

    #[test]
    fn epsnapshot_opts_allow_full_project_scan() {
        let opts = SendOpts::for_epsnapshot();

        assert_eq!(opts.timeout, Duration::from_secs(180));
        assert_eq!(opts.busy_timeout, Duration::from_secs(180));
        assert_eq!(opts.poll_interval, Duration::from_millis(200));
    }

    #[test]
    fn map_source_confirmation_never_inherits_build_timeout() {
        let opts = SendOpts::for_map_source_confirmation();

        assert_eq!(opts.timeout, Duration::from_secs(3));
        assert_eq!(opts.busy_timeout, Duration::from_secs(3));
        assert_eq!(opts.poll_interval, Duration::from_millis(200));
    }

    fn srv_entries(dir: &Path, suffix: &str) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.starts_with("srv-") && name.ends_with(suffix))
                    .unwrap_or(false)
            })
            .collect()
    }

    fn canned_reply(command: &str) -> String {
        match command {
            "PING" => "PONG 123".to_string(),
            "STATUS" => "compiling=false\nproject=Demo\nversion=3".to_string(),
            "LIST" => [
                "triggers/main.eps\tRawText",
                "ui/dialog.cui\tCUI",
                "scenario.chk\tCHK",
            ]
            .join("\n"),
            "GET scripts/main.eps" => "function main()\n    // file body\nend".to_string(),
            "SET scripts/main.eps\nline1\nline2" => "OK".to_string(),
            other => format!("ERROR: unexpected command {other}"),
        }
    }

    struct FakeBridge {
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl FakeBridge {
        fn spawn(data_dir: &Path, expected_count: usize, seen: SeenLog) -> Self {
            Self::spawn_with(data_dir, expected_count, seen, canned_reply)
        }

        fn spawn_with<F>(
            data_dir: &Path,
            expected_count: usize,
            seen: SeenLog,
            responder: F,
        ) -> Self
        where
            F: Fn(&str) -> String + Send + 'static,
        {
            let data_dir = data_dir.to_path_buf();
            let stop = Arc::new(AtomicBool::new(false));
            let stop_thread = Arc::clone(&stop);
            let handle = thread::spawn(move || {
                let inbox = data_dir.join("inbox");
                let outbox = data_dir.join("outbox");
                fs::create_dir_all(&inbox).unwrap();
                fs::create_dir_all(&outbox).unwrap();

                let deadline = Instant::now() + Duration::from_secs(5);
                let mut handled = 0usize;
                while !stop_thread.load(Ordering::SeqCst)
                    && handled < expected_count
                    && Instant::now() < deadline
                {
                    let Ok(entries) = fs::read_dir(&inbox) else {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    };

                    for entry in entries.filter_map(Result::ok) {
                        let cmd_path = entry.path();
                        let Some(file_name) = cmd_path.file_name().and_then(|name| name.to_str())
                        else {
                            continue;
                        };
                        if !file_name.starts_with("srv-") || !file_name.ends_with(".cmd") {
                            continue;
                        }

                        let bytes = fs::read(&cmd_path).unwrap();
                        let command = String::from_utf8(bytes.clone()).unwrap();
                        seen.lock().push((command.clone(), bytes));

                        fs::remove_file(&cmd_path).unwrap();
                        let stem = file_name.trim_end_matches(".cmd");
                        let result_path = outbox.join(format!("{stem}.result"));
                        fs::write(result_path, responder(&command).as_bytes()).unwrap();
                        handled += 1;
                    }

                    if handled < expected_count {
                        thread::sleep(Duration::from_millis(5));
                    }
                }
            });

            Self {
                stop,
                handle: Some(handle),
            }
        }
    }

    impl Drop for FakeBridge {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                handle.join().unwrap();
            }
        }
    }

    // Target contract for EUD-147: the Lua bridge writes heartbeat.txt roughly every UI
    // tick (~1s), so app-side liveness should treat a heartbeat older than 3s as stale.
    const TARGET_HEARTBEAT_STALE_AFTER: Duration = Duration::from_secs(3);
    const EDITOR_NOT_CONNECTED: &str = "editor not connected";

    fn write_live_status_files(
        data_dir: &Path,
        compiling: bool,
        project: &str,
        open_map_name: &str,
    ) -> SystemTime {
        fs::create_dir_all(data_dir).unwrap();
        fs::write(
            data_dir.join("status.txt"),
            format!("compiling={compiling}\nproject={project}\nopenMapName={open_map_name}\n"),
        )
        .unwrap();
        fs::write(data_dir.join("heartbeat.txt"), b"alive\n").unwrap();
        SystemTime::now()
    }

    fn assert_editor_not_connected(error: BridgeError) {
        assert_eq!(error.to_string(), EDITOR_NOT_CONNECTED);
    }

    #[test]
    fn status_snapshot_reads_status_txt_when_heartbeat_is_fresh() {
        let data_dir = unique_temp_dir("status-fresh");
        let now = write_live_status_files(&data_dir, true, "DemoProject", r"'C:\maps\demo.scx'");
        let bridge = BridgeIo::new(&data_dir);

        let status = bridge
            .read_status_snapshot_at(now, TARGET_HEARTBEAT_STALE_AFTER)
            .unwrap();

        assert!(status.compiling);
        assert_eq!(status.project, "DemoProject");
        assert_eq!(status.open_map_name.as_deref(), Some(r"'C:\maps\demo.scx'"));

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn status_snapshot_rejects_stale_or_absent_heartbeat() {
        let data_dir = unique_temp_dir("status-stale");
        let heartbeat_time =
            write_live_status_files(&data_dir, false, "DemoProject", r"'C:\maps\demo.scx'");
        let bridge = BridgeIo::new(&data_dir);

        let stale_now = heartbeat_time + TARGET_HEARTBEAT_STALE_AFTER + Duration::from_millis(1);
        assert_editor_not_connected(
            bridge
                .read_status_snapshot_at(stale_now, TARGET_HEARTBEAT_STALE_AFTER)
                .unwrap_err(),
        );

        fs::remove_file(data_dir.join("heartbeat.txt")).unwrap();
        assert_editor_not_connected(
            bridge
                .read_status_snapshot_at(SystemTime::now(), TARGET_HEARTBEAT_STALE_AFTER)
                .unwrap_err(),
        );

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn connected_list_round_trip_derives_settable_from_file_type() {
        let data_dir = unique_temp_dir("list-connected");
        let now = write_live_status_files(&data_dir, false, "DemoProject", r"'C:\maps\demo.scx'");
        let seen: SeenLog = Arc::new(Mutex::new(Vec::new()));
        let _fake = FakeBridge::spawn(&data_dir, 1, Arc::clone(&seen));
        let bridge = BridgeIo::new(&data_dir);
        let opts = fast_opts();

        let files = bridge
            .list_connected_at(&opts, None, now, TARGET_HEARTBEAT_STALE_AFTER)
            .unwrap();

        assert_eq!(files.len(), 3);
        assert_eq!(files[0].path, "triggers/main.eps");
        assert_eq!(files[0].ftype, "RawText");
        assert!(files[0].settable);
        assert_eq!(files[1].path, "ui/dialog.cui");
        assert_eq!(files[1].ftype, "CUI");
        assert!(files[1].settable);
        assert_eq!(files[2].path, "scenario.chk");
        assert_eq!(files[2].ftype, "CHK");
        assert!(!files[2].settable);

        let commands: Vec<String> = seen
            .lock()
            .iter()
            .map(|(command, _)| command.clone())
            .collect();
        assert_eq!(commands, vec!["LIST".to_string()]);
        assert!(
            srv_entries(&data_dir.join("outbox"), ".result").is_empty(),
            "BridgeIo should delete consumed LIST .result files"
        );

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn get_main_preserves_root_and_nested_project_paths() {
        for (tag, reply, expected) in [
            ("main-root", "survivor_mvp\r\n", "survivor_mvp"),
            (
                "main-nested",
                "systems/combat/survivor_mvp\r\n",
                "systems/combat/survivor_mvp",
            ),
        ] {
            let data_dir = unique_temp_dir(tag);
            let seen: SeenLog = Arc::new(Mutex::new(Vec::new()));
            {
                let response = reply.to_string();
                let _fake =
                    FakeBridge::spawn_with(&data_dir, 1, Arc::clone(&seen), move |command| {
                        assert_eq!(command, "GETMAIN");
                        response.clone()
                    });
                let bridge = BridgeIo::new(&data_dir);

                assert_eq!(
                    bridge.get_main(&fast_opts(), None).unwrap(),
                    Some(expected.to_string())
                );
            }
            assert_eq!(seen.lock()[0].0, "GETMAIN");
            fs::remove_dir_all(&data_dir).ok();
        }
    }

    #[test]
    fn get_main_maps_empty_success_and_no_project_to_none() {
        for (tag, reply) in [("main-empty", ""), ("main-no-project", "ERROR: no project")] {
            let data_dir = unique_temp_dir(tag);
            let seen: SeenLog = Arc::new(Mutex::new(Vec::new()));
            {
                let response = reply.to_string();
                let _fake = FakeBridge::spawn_with(&data_dir, 1, Arc::clone(&seen), move |_| {
                    response.clone()
                });
                let bridge = BridgeIo::new(&data_dir);

                assert_eq!(bridge.get_main(&fast_opts(), None).unwrap(), None);
            }
            fs::remove_dir_all(&data_dir).ok();
        }
    }

    #[test]
    fn get_main_surfaces_unexpected_bridge_errors() {
        let data_dir = unique_temp_dir("main-error");
        let seen: SeenLog = Arc::new(Mutex::new(Vec::new()));
        {
            let _fake =
                FakeBridge::spawn_with(&data_dir, 1, seen, |_| "ERROR: getmain failed".to_string());
            let bridge = BridgeIo::new(&data_dir);

            match bridge.get_main(&fast_opts(), None).unwrap_err() {
                BridgeError::Error(message) => assert_eq!(message, "ERROR: getmain failed"),
                other => panic!("expected BridgeError::Error, got {other:?}"),
            }
        }
        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn connected_list_returns_editor_not_connected_without_heartbeat_or_data_dir() {
        let data_dir = unique_temp_dir("list-disconnected");
        let bridge = BridgeIo::new(data_dir.join("missing-agent-dir"));
        let opts = fast_opts();

        assert_editor_not_connected(
            bridge
                .list_connected_at(&opts, None, SystemTime::now(), TARGET_HEARTBEAT_STALE_AFTER)
                .unwrap_err(),
        );

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn consume_result_waits_for_non_empty_byte_length_to_stabilize() {
        let data_dir = unique_temp_dir("stable-result");
        let outbox = data_dir.join("outbox");
        fs::create_dir_all(&outbox).unwrap();
        let result_path = outbox.join("srv-deadbeef.result");
        let bridge = BridgeIo::new(&data_dir);
        let prefix = "line 1\npartial";
        let full = "line 1\npartial\nline 2\nline 3";

        fs::write(&result_path, prefix.as_bytes()).unwrap();
        let state = match bridge
            .consume_result(&result_path, ConsumePollState::default())
            .unwrap()
        {
            ConsumeResult::Pending { state } => state,
            ConsumeResult::Ready(reply) => panic!("truncated reply was consumed: {reply:?}"),
        };
        assert_eq!(state.last_non_empty_len, Some(prefix.len()));
        assert!(
            result_path.exists(),
            "a first non-empty sighting must remain for the next poll"
        );

        fs::write(&result_path, full.as_bytes()).unwrap();
        let state = match bridge.consume_result(&result_path, state).unwrap() {
            ConsumeResult::Pending { state } => state,
            ConsumeResult::Ready(reply) => panic!("changed-length reply was consumed: {reply:?}"),
        };
        assert_eq!(state.last_non_empty_len, Some(full.len()));
        assert!(
            result_path.exists(),
            "a changed non-empty length must remain for the next poll"
        );

        let reply = match bridge.consume_result(&result_path, state).unwrap() {
            ConsumeResult::Ready(reply) => reply,
            ConsumeResult::Pending { state } => panic!("stable reply stayed pending: {state:?}"),
        };
        assert_eq!(reply, full);
        assert!(!result_path.exists());

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn ping_status_get_round_trip_against_fake_bridge() {
        let data_dir = unique_temp_dir("roundtrip");
        let seen: SeenLog = Arc::new(Mutex::new(Vec::new()));
        let _fake = FakeBridge::spawn(&data_dir, 4, Arc::clone(&seen));
        let bridge = BridgeIo::new(&data_dir);
        let opts = fast_opts();

        assert_eq!(bridge.send("PING", &opts, None).unwrap(), "PONG 123");
        assert_eq!(bridge.ping(&opts, None).unwrap(), "PONG 123");
        assert_eq!(
            bridge.status(&opts, None).unwrap(),
            "compiling=false\nproject=Demo\nversion=3"
        );
        assert_eq!(
            bridge.get("scripts/main.eps", &opts, None).unwrap(),
            "function main()\n    // file body\nend"
        );

        let commands: Vec<String> = seen
            .lock()
            .iter()
            .map(|(command, _)| command.clone())
            .collect();
        assert_eq!(
            commands,
            vec![
                "PING".to_string(),
                "PING".to_string(),
                "STATUS".to_string(),
                "GET scripts/main.eps".to_string(),
            ]
        );

        assert!(
            srv_entries(&data_dir.join("inbox"), ".cmd").is_empty(),
            "the fake bridge should delete consumed .cmd files"
        );
        assert!(
            srv_entries(&data_dir.join("outbox"), ".result").is_empty(),
            "BridgeIo should delete consumed .result files"
        );

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn busy_timeout_notifies_once_and_leaves_command_file() {
        let data_dir = unique_temp_dir("busy");
        fs::create_dir_all(data_dir.join("inbox")).unwrap();
        fs::create_dir_all(data_dir.join("outbox")).unwrap();
        fs::write(
            data_dir.join("status.txt"),
            "compiling=true\nproject=Demo\n",
        )
        .unwrap();

        let bridge = BridgeIo::new(&data_dir);
        let opts = short_busy_opts();
        let busy_count = AtomicUsize::new(0);
        let on_busy = || {
            busy_count.fetch_add(1, Ordering::SeqCst);
        };

        let err = bridge.send("PING", &opts, Some(&on_busy)).unwrap_err();
        match err {
            BridgeError::Busy(message) => {
                assert!(
                    message.contains("bridge did not answer") || message.contains("busy"),
                    "busy errors should describe the timed-out bridge wait"
                );
            }
            other => panic!("expected BridgeError::Busy, got {other:?}"),
        }

        assert_eq!(
            busy_count.load(Ordering::SeqCst),
            1,
            "on_busy must fire exactly once while status.txt reports compiling=true"
        );
        assert_eq!(
            srv_entries(&data_dir.join("inbox"), ".cmd").len(),
            1,
            "timed-out commands must be left in place for the bridge to apply later"
        );

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn cleanup_stale_removes_only_server_namespace() {
        let data_dir = unique_temp_dir("cleanup");
        let inbox = data_dir.join("inbox");
        let outbox = data_dir.join("outbox");
        fs::create_dir_all(&inbox).unwrap();
        fs::create_dir_all(&outbox).unwrap();

        fs::write(inbox.join("srv-deadbeef.cmd"), "PING").unwrap();
        fs::write(inbox.join("agent_legacy.cmd"), "legacy").unwrap();
        fs::write(outbox.join("srv-deadbeef.result"), "PONG").unwrap();
        fs::write(outbox.join("agent_legacy.result"), "legacy").unwrap();
        fs::create_dir(outbox.join("epsnapshot-stale")).unwrap();
        fs::write(outbox.join("epsnapshot-foreign-file"), "keep").unwrap();

        let bridge = BridgeIo::new(&data_dir);
        bridge.cleanup_stale();

        assert!(!inbox.join("srv-deadbeef.cmd").exists());
        assert!(!outbox.join("srv-deadbeef.result").exists());
        assert!(!outbox.join("epsnapshot-stale").exists());
        assert!(outbox.join("epsnapshot-foreign-file").exists());
        assert!(
            inbox.join("agent_legacy.cmd").exists(),
            "cleanup_stale must never touch legacy agent_* inbox files"
        );
        assert!(
            outbox.join("agent_legacy.result").exists(),
            "cleanup_stale must never touch legacy agent_* outbox files"
        );

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn command_file_is_utf8_without_bom_and_byte_exact() {
        let data_dir = unique_temp_dir("nobom");
        let seen: SeenLog = Arc::new(Mutex::new(Vec::new()));
        let _fake = FakeBridge::spawn(&data_dir, 1, Arc::clone(&seen));
        let bridge = BridgeIo::new(&data_dir);
        let opts = fast_opts();
        let command = "SET scripts/main.eps\nline1\nline2";

        assert_eq!(bridge.send(command, &opts, None).unwrap(), "OK");

        let seen = seen.lock();
        assert_eq!(seen.len(), 1);
        let bytes = &seen[0].1;
        assert!(
            !bytes.starts_with(&[0xEF, 0xBB, 0xBF]),
            ".cmd files must be UTF-8 without a BOM"
        );
        assert_eq!(
            bytes,
            command.as_bytes(),
            ".cmd body must be delivered byte-exact with no newline translation"
        );

        fs::remove_dir_all(&data_dir).ok();
    }

    fn write_snapshot_fixture(
        outbox: &Path,
        token: &str,
        project: &str,
        rows: &[(&str, &str, Option<&str>)],
    ) -> PathBuf {
        let snapshot_dir = outbox.join(format!("epsnapshot-{token}"));
        fs::create_dir_all(&snapshot_dir).unwrap();
        let mut manifest = vec![
            "EUD-EPSNAPSHOT\t1".to_string(),
            format!("token\t{token}"),
            format!("project\t{}", BASE64_STANDARD.encode(project.as_bytes())),
            format!(
                "identity\t{}",
                BASE64_STANDARD.encode(format!("{project}\nmap.scx").as_bytes())
            ),
        ];
        for (index, (project_path, ftype, content)) in rows.iter().enumerate() {
            let ordinal = index + 1;
            let (length, status) = match content {
                Some(content) => {
                    fs::write(
                        snapshot_dir.join(format!("{ordinal:06}.eps")),
                        content.as_bytes(),
                    )
                    .unwrap();
                    (content.len(), "ok")
                }
                None => (0, "unreadable"),
            };
            manifest.push(format!(
                "file\t{ordinal}\t{ftype}\t{}\t{length}\t{status}",
                BASE64_STANDARD.encode(project_path.as_bytes())
            ));
        }
        fs::write(
            snapshot_dir.join("manifest.tsv"),
            manifest.join("\n").as_bytes(),
        )
        .unwrap();
        snapshot_dir
    }

    #[test]
    fn eps_snapshot_manifest_preserves_nested_unicode_empty_and_unreadable_files() {
        let data_dir = unique_temp_dir("snapshot-valid");
        let outbox = data_dir.join("outbox");
        fs::create_dir_all(&outbox).unwrap();
        let token = "00000000-0000-4000-8000-000000000001";
        let snapshot_dir = write_snapshot_fixture(
            &outbox,
            token,
            "프로젝트/Example",
            &[
                ("lib/한글.eps", "CUIEps", Some("object 상태 { var 체력; };")),
                ("empty.eps", "RawText", Some("")),
                ("closed.eps", "CUIEps", None),
            ],
        );

        let snapshot = decode_eps_snapshot(&outbox, &snapshot_dir, token).unwrap();
        assert_eq!(snapshot.project, "프로젝트/Example");
        assert_eq!(snapshot.files[0].path, "lib/한글.eps");
        assert_eq!(
            snapshot.files[0].content.as_deref(),
            Some("object 상태 { var 체력; };")
        );
        assert_eq!(snapshot.files[1].content.as_deref(), Some(""));
        assert_eq!(snapshot.files[2].content, None);
        assert!(!fs::read(snapshot_dir.join("manifest.tsv"))
            .unwrap()
            .starts_with(&[0xef, 0xbb, 0xbf]));

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn eps_snapshot_manifest_accepts_extensionless_settable_paths() {
        let data_dir = unique_temp_dir("snapshot-extensionless-settable");
        let outbox = data_dir.join("outbox");
        fs::create_dir_all(&outbox).unwrap();
        let token = "00000000-0000-4000-8000-000000000021";
        let snapshot_dir = write_snapshot_fixture(
            &outbox,
            token,
            "Project",
            &[
                (
                    "survivor_mvp",
                    "CUIEps",
                    Some("function onPluginStart() {}"),
                ),
                ("notes", "RawText", Some("plain text")),
            ],
        );

        let snapshot = decode_eps_snapshot(&outbox, &snapshot_dir, token).unwrap();
        assert_eq!(snapshot.files[0].path, "survivor_mvp");
        assert_eq!(
            snapshot.files[0].content.as_deref(),
            Some("function onPluginStart() {}")
        );
        assert_eq!(snapshot.files[1].path, "notes");
        assert_eq!(snapshot.files[1].content.as_deref(), Some("plain text"));

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn eps_snapshot_manifest_uses_map_name_when_project_filename_is_empty() {
        let data_dir = unique_temp_dir("snapshot-empty-project-filename");
        let outbox = data_dir.join("outbox");
        fs::create_dir_all(&outbox).unwrap();
        let token = "00000000-0000-4000-8000-000000000002";
        let snapshot_dir = write_snapshot_fixture(
            &outbox,
            token,
            "",
            &[("main.eps", "CUIEps", Some("function onPluginStart() {}"))],
        );

        let snapshot = decode_eps_snapshot(&outbox, &snapshot_dir, token).unwrap();
        assert_eq!(snapshot.project, "map.scx");
        assert_eq!(snapshot.identity, "\nmap.scx");

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn eps_snapshot_manifest_uses_bridge_session_when_all_project_names_are_empty() {
        let data_dir = unique_temp_dir("snapshot-untitled-legacy");
        let outbox = data_dir.join("outbox");
        fs::create_dir_all(&outbox).unwrap();
        fs::write(
            data_dir.join("bridge_loaded.txt"),
            "agent bridge v7 loaded at session-1",
        )
        .unwrap();
        let token = "00000000-0000-4000-8000-000000000003";
        let snapshot_dir = write_snapshot_fixture(
            &outbox,
            token,
            "",
            &[("main.eps", "CUIEps", Some("function onPluginStart() {}"))],
        );
        let manifest_path = snapshot_dir.join("manifest.tsv");
        let mut manifest: Vec<String> = fs::read_to_string(&manifest_path)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        manifest[3] = format!("identity\t{}", BASE64_STANDARD.encode(""));
        fs::write(&manifest_path, manifest.join("\n")).unwrap();

        let snapshot = decode_eps_snapshot(&outbox, &snapshot_dir, token).unwrap();
        assert_eq!(snapshot.project, "Untitled");
        assert!(snapshot.identity.starts_with("legacy-untitled\n"));
        assert!(snapshot
            .identity
            .ends_with("\nagent bridge v7 loaded at session-1"));

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn eps_snapshot_manifest_rejects_bad_base64_lengths_ordinals_tokens_and_containment() {
        let data_dir = unique_temp_dir("snapshot-invalid");
        let outbox = data_dir.join("outbox");
        fs::create_dir_all(&outbox).unwrap();

        let token = "00000000-0000-4000-8000-000000000010";
        let snapshot_dir = write_snapshot_fixture(
            &outbox,
            token,
            "Project",
            &[("main.eps", "CUIEps", Some("a"))],
        );
        fs::write(snapshot_dir.join("000001.eps"), b"too long").unwrap();
        assert!(decode_eps_snapshot(&outbox, &snapshot_dir, token).is_err());

        let token = "00000000-0000-4000-8000-000000000011";
        let snapshot_dir = write_snapshot_fixture(
            &outbox,
            token,
            "Project",
            &[("main.eps", "CUIEps", Some("a"))],
        );
        let manifest_path = snapshot_dir.join("manifest.tsv");
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap()
            .replace("file\t1\t", "file\t2\t");
        fs::write(&manifest_path, manifest).unwrap();
        assert!(decode_eps_snapshot(&outbox, &snapshot_dir, token).is_err());

        let token = "00000000-0000-4000-8000-000000000012";
        let snapshot_dir = write_snapshot_fixture(
            &outbox,
            token,
            "Project",
            &[("main.eps", "CUIEps", Some("a"))],
        );
        let manifest_path = snapshot_dir.join("manifest.tsv");
        let manifest = fs::read_to_string(&manifest_path).unwrap().replace(
            &BASE64_STANDARD.encode("main.eps".as_bytes()),
            "not-base64!",
        );
        fs::write(&manifest_path, manifest).unwrap();
        assert!(decode_eps_snapshot(&outbox, &snapshot_dir, token).is_err());

        let token = "00000000-0000-4000-8000-000000000013";
        let snapshot_dir = write_snapshot_fixture(
            &outbox,
            token,
            "Project",
            &[("main.eps", "CUIEps", Some("a"))],
        );
        let manifest_path = snapshot_dir.join("manifest.tsv");
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap()
            .replace(token, "00000000-0000-4000-8000-000000000099");
        fs::write(&manifest_path, manifest).unwrap();
        assert!(decode_eps_snapshot(&outbox, &snapshot_dir, token).is_err());

        let outside = data_dir.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let token = "00000000-0000-4000-8000-000000000014";
        let outside_dir = write_snapshot_fixture(
            &outside,
            token,
            "Project",
            &[("main.eps", "CUIEps", Some("a"))],
        );
        assert!(decode_eps_snapshot(&outbox, &outside_dir, token).is_err());

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn eps_snapshot_manifest_rejects_case_insensitive_path_collisions() {
        let data_dir = unique_temp_dir("snapshot-collision");
        let outbox = data_dir.join("outbox");
        fs::create_dir_all(&outbox).unwrap();
        let token = "00000000-0000-4000-8000-000000000020";
        let snapshot_dir = write_snapshot_fixture(
            &outbox,
            token,
            "Project",
            &[
                ("Lib/Main.eps", "CUIEps", Some("a")),
                ("lib/main.eps", "CUIEps", Some("b")),
            ],
        );
        assert!(decode_eps_snapshot(&outbox, &snapshot_dir, token).is_err());
        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn snapshot_eps_uses_one_request_token_and_removes_consumed_directory() {
        let data_dir = unique_temp_dir("snapshot-roundtrip");
        let inbox = data_dir.join("inbox");
        let outbox = data_dir.join("outbox");
        fs::create_dir_all(&inbox).unwrap();
        fs::create_dir_all(&outbox).unwrap();
        let responder_data = data_dir.clone();
        let responder = thread::spawn(move || {
            let inbox = responder_data.join("inbox");
            let outbox = responder_data.join("outbox");
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                for entry in fs::read_dir(&inbox).unwrap().filter_map(Result::ok) {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    if !file_name.starts_with("srv-") || !file_name.ends_with(".cmd") {
                        continue;
                    }
                    let command = fs::read_to_string(entry.path()).unwrap();
                    let Some(token) = command.strip_prefix("EPSNAPSHOT ") else {
                        continue;
                    };
                    let token = token.trim().to_string();
                    write_snapshot_fixture(
                        &outbox,
                        &token,
                        "Project",
                        &[
                            ("main.eps", "CUIEps", Some("import lib.units;")),
                            ("lib/units.eps", "CUIEps", Some("object Unit {};")),
                        ],
                    );
                    let name = file_name.trim_end_matches(".cmd").to_string();
                    fs::remove_file(entry.path()).unwrap();
                    fs::write(
                        outbox.join(format!("{name}.result")),
                        b"OK: epsnapshot 2 files",
                    )
                    .unwrap();
                    return token;
                }
                assert!(Instant::now() < deadline, "snapshot request did not arrive");
                thread::sleep(Duration::from_millis(5));
            }
        });

        let bridge = BridgeIo::new(&data_dir);
        let snapshot = bridge.snapshot_eps(&fast_opts(), None).unwrap();
        let token = responder.join().unwrap();
        assert_eq!(snapshot.project, "Project");
        assert_eq!(snapshot.files.len(), 2);
        assert!(!outbox.join(format!("epsnapshot-{token}")).exists());
        assert!(srv_entries(&outbox, ".result").is_empty());

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn lua_getmain_uses_mainfile_identity_without_changing_list_shape() {
        let source = include_str!("../../bridge/ZZZ_10_agent_bridge.lua");
        assert_eq!(source.matches("elseif cmd == \"GETMAIN\" then").count(), 1);

        let helper = source
            .split("local function mainFilePath")
            .nth(1)
            .unwrap()
            .split("-- ------------------------------------------------------------------")
            .next()
            .unwrap();
        assert!(helper.contains("local main = pj.TEData.MainFile"));
        assert!(helper.contains("if main == nil then return \"\" end"));
        assert!(helper.contains(
            "walk(pj.TEData.PFIles, \"\", function(p, f) if f == main then found = p end end)"
        ));

        let getmain = source
            .split("elseif cmd == \"GETMAIN\" then")
            .nth(1)
            .unwrap()
            .split("elseif cmd == \"GETDAT\" then")
            .next()
            .unwrap();
        assert!(getmain.contains("return mainFilePath()"));

        let list = source
            .split("elseif cmd == \"LIST\" then")
            .nth(1)
            .unwrap()
            .split("elseif cmd == \"EPSNAPSHOT\" then")
            .next()
            .unwrap();
        assert!(list
            .contains("lines[#lines + 1] = p .. \"\\t\" .. ((okT and ftype) and ftype or \"?\")"));
    }

    #[test]
    fn lua_eps_snapshot_falls_back_when_project_filename_is_empty() {
        let source = include_str!("../../bridge/ZZZ_10_agent_bridge.lua");
        let metadata = source
            .split("local function snapshotProjectMetadata")
            .nth(1)
            .unwrap()
            .split("local function split")
            .next()
            .unwrap();
        assert!(metadata.contains("if filename == \"\" and openMapName == \"\" then"));
        assert!(metadata.contains("\"untitled\\n\" .. bridgeSessionId"));
        assert!(metadata.contains("return openMapName, filename .. \"\\n\" .. openMapName"));

        let snapshot = source
            .split("local function writeEpsSnapshot")
            .nth(1)
            .unwrap()
            .split("local function handleCommand")
            .next()
            .unwrap();
        assert!(snapshot
            .contains("local projectDisplay, projectIdentity = snapshotProjectMetadata(pj)"));
        assert!(snapshot.contains("\"project\\t\" .. base64Utf8(projectDisplay)"));
        assert!(snapshot.contains("\"identity\\t\" .. base64Utf8(projectIdentity)"));
        assert_eq!(source.matches("snapshotProjectMetadata(pj)").count(), 4);
    }

    #[test]
    fn lua_eps_snapshot_contract_preserves_tick_compiling_and_dump_invariants() {
        let source = include_str!("../../bridge/ZZZ_10_agent_bridge.lua");
        assert!(source.contains("elseif cmd == \"EPSNAPSHOT\" then"));
        assert!(source.contains("elseif cmd == \"DUMP\" then"));
        assert!(source.contains("UTF8Encoding(false)"));
        let snapshot = source
            .split("local function writeEpsSnapshot")
            .nth(1)
            .unwrap()
            .split("local function handleCommand")
            .next()
            .unwrap();
        assert!(snapshot.contains("local ftype = ftypeName(f)"));
        assert!(snapshot.contains("if isSettableTypeName(ftype) or isEpsPath then"));
        assert!(
            snapshot.find("ordinalName").unwrap() < snapshot.find("manifest.tsv").unwrap(),
            "ordinal content must be written before the last-written manifest"
        );

        let tick = source.split("timer.Tick:Add").nth(1).unwrap();
        let heartbeat = tick.find("heartbeat.txt").unwrap();
        let compiling = tick.find("if pg ~= nil and pg.IsCompilng then").unwrap();
        let busy_status = tick[compiling..].find("status.txt").unwrap() + compiling;
        let early_return = tick[busy_status..].find("return").unwrap() + busy_status;
        let project_access = tick.find("local pj = GlobalObj.pjData").unwrap();
        assert!(heartbeat < compiling);
        assert!(compiling < busy_status);
        assert!(busy_status < early_return);
        assert!(
            tick[busy_status..early_return].contains("\"\\r\\nopenMapName=\" .. lastOpenMapLine")
        );
        assert!(
            early_return < project_access,
            "compiling Tick must return before any project-object access"
        );
        assert!(tick[project_access..]
            .contains("local projectDisplay, _, openMapName = snapshotProjectMetadata(pj)"));
        assert!(tick[project_access..].contains("lastOpenMapLine = \"'\" .. openMapName .. \"'\""));
    }
}
