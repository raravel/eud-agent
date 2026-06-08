//! File-IPC client for the EUD Editor 3 Lua bridge.
//!
//! The app writes `srv-<id8>.cmd` into the editor's `Data\agent\inbox` and polls
//! `outbox` for the matching `.result` file. Files are raw UTF-8 bytes without a BOM,
//! and command writes are atomic so the Lua bridge never reads a partial `.cmd`.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(180);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(200);
const SETTABLE_FAMILIES: [&str; 2] = ["CUI", "RAWTEXT"];
const DAT_NAMES: [&str; 10] = [
    "units", "weapons", "flingy", "sprites", "images", "upgrades", "techdata", "orders",
    "portdata", "sfxdata",
];

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// File-IPC client bound to the editor's `Data\agent` directory.
#[derive(Debug, Clone)]
pub struct BridgeIo {
    data_dir: PathBuf,
    inbox: PathBuf,
    outbox: PathBuf,
    status_file: PathBuf,
}

/// Polling and busy-editor timeout settings for a bridge request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendOpts {
    pub timeout: Duration,
    pub busy_timeout: Duration,
    pub poll_interval: Duration,
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

/// Errors returned by the file-IPC bridge client.
#[derive(Debug)]
pub enum BridgeError {
    /// The bridge returned an `ERROR:`-prefixed reply.
    Error(String),
    /// The bridge did not answer before the selected timeout window.
    Busy(String),
    /// Filesystem error while writing, polling, or cleaning IPC files.
    Io(io::Error),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error(message) | Self::Busy(message) => f.write_str(message),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl Error for BridgeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Error(_) | Self::Busy(_) => None,
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

    /// Read a project file by path.
    pub fn get(
        &self,
        path: &str,
        opts: &SendOpts,
        on_busy: Option<&dyn Fn()>,
    ) -> Result<String, BridgeError> {
        self.send(&format!("GET {path}"), opts, on_busy)
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

    /// Remove stale server-owned IPC files from startup.
    ///
    /// Only `srv-*.cmd` and `srv-*.result` are removed. Legacy `agent_*` files are never
    /// touched, and missing inbox/outbox dirs are tolerated.
    pub fn cleanup_stale(&self) {
        remove_matching(&self.inbox, "srv-", ".cmd");
        remove_matching(&self.outbox, "srv-", ".result");
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
    use super::{BridgeError, BridgeIo, ConsumePollState, ConsumeResult, SendOpts};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
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
                        seen.lock().unwrap().push((command.clone(), bytes));

                        fs::remove_file(&cmd_path).unwrap();
                        let stem = file_name.trim_end_matches(".cmd");
                        let result_path = outbox.join(format!("{stem}.result"));
                        fs::write(result_path, canned_reply(&command).as_bytes()).unwrap();
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
            .unwrap()
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

        let bridge = BridgeIo::new(&data_dir);
        bridge.cleanup_stale();

        assert!(!inbox.join("srv-deadbeef.cmd").exists());
        assert!(!outbox.join("srv-deadbeef.result").exists());
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

        let seen = seen.lock().unwrap();
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
}
