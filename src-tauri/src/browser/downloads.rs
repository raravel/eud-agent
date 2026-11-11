//! Files the browser downloaded, kept apart from the webviews so their rules
//! (where a file lands, how big it may be, who sees it, what it is, where it
//! may be saved in the project) are testable without a window.
//!
//! Every download of every tab lands in its own folder under
//! `LocalAppData/eud-agent/browser/downloads/<uuid>/`, whether an agent call
//! or the user started it. A session sees only the downloads of its own tabs.
//! Nothing here touches the project: `browser_save` copies one file into it
//! through [`save_target`]'s path policy.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Largest file the browser keeps.
pub const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
/// `browser_download` waits this long by default, and at most `MAX_...`.
pub const DEFAULT_DOWNLOAD_WAIT_MS: u64 = 60_000;
pub const MAX_DOWNLOAD_WAIT_MS: u64 = 120_000;
/// Largest downloaded image shown to the model as an image.
pub const MAX_IMAGE_RESULT_BYTES: usize = 5 * 1024 * 1024;
/// Downloads older than this are removed when the app starts.
pub const DOWNLOAD_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
/// Longest file name kept (extension included).
const MAX_FILE_NAME_CHARS: usize = 100;
const MAX_EXTENSION_CHARS: usize = 16;
/// Longest original (display) name kept.
const MAX_DISPLAY_NAME_CHARS: usize = 200;

pub const DOWNLOAD_NOT_FOUND: &str =
    "다운로드를 찾을 수 없습니다. browser_downloads 로 이 세션이 받은 파일의 downloadId 를 확인하세요.";
pub const DOWNLOAD_TOO_LARGE: &str =
    "파일이 64 MiB를 넘어 받지 않았습니다. 더 작은 파일(짧은 길이, 낮은 품질)을 고르세요.";

/// The size a page reported for a blob it fetched, checked before any byte
/// is read: the page script is not trusted to keep to the limit itself.
pub fn page_blob_size(reported: Option<u64>) -> Result<u64, String> {
    match reported {
        Some(size) if size > MAX_DOWNLOAD_BYTES => Err(DOWNLOAD_TOO_LARGE.to_string()),
        Some(size) if size > 0 => Ok(size),
        _ => Err("페이지가 빈 파일(또는 크기를 알 수 없는 파일)을 돌려주어 받지 않았습니다. 주소를 확인하거나 페이지의 다운로드 버튼 ref 로 받으세요.".to_string()),
    }
}

/// Extensions `browser_save` writes into the project.
pub const SAVE_EXTENSIONS: &[&str] = &[
    "ogg", "wav", "mp3", "flac", "png", "jpg", "jpeg", "gif", "webp", "bmp", "txt", "md", "json",
    "csv",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadState {
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadRecord {
    /// `d1`, `d2`, ... in the order downloads started.
    pub download_id: String,
    pub number: u64,
    pub session_id: String,
    pub tab_id: String,
    /// The tab webview's label, which the download events name.
    pub label: String,
    /// The URL the download came from; download events are matched on it.
    pub url: String,
    /// The name on disk: [`sanitize_file_name`] of the page's name.
    pub file_name: String,
    /// The page's own name for the file, for the chrome.
    pub display_name: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub mime: String,
    pub sha256: String,
    pub state: DownloadState,
    pub error: Option<String>,
    /// An agent call was running on the tab when the download started.
    pub by_agent: bool,
    /// A `browser_download` call already returned it.
    pub reported: bool,
    /// The browser announced it would go over [`MAX_DOWNLOAD_BYTES`].
    pub oversize: bool,
    pub finished_at_ms: Option<u64>,
}

impl DownloadRecord {
    /// What a tool result shows of a download: never its local path.
    pub fn tool_value(&self) -> Value {
        let mut value = json!({
            "downloadId": self.download_id,
            "tabId": self.tab_id,
            "fileName": self.file_name,
            "url": shorten(&self.url, 300),
            "state": self.state,
            "byAgent": self.by_agent,
        });
        if self.display_name != self.file_name {
            value["originalName"] = json!(self.display_name);
        }
        if self.state == DownloadState::Completed {
            value["bytes"] = json!(self.bytes);
            value["mime"] = json!(self.mime);
            value["sha256"] = json!(self.sha256);
        }
        if let Some(error) = &self.error {
            value["error"] = json!(error);
        }
        if let Some(at) = self.finished_at_ms {
            value["finishedAt"] = json!(at);
        }
        value
    }
}

/// One download as the browser chrome lists it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDownloadView {
    pub download_id: String,
    pub tab_id: String,
    pub session_id: String,
    pub session_title: String,
    pub file_name: String,
    pub bytes: u64,
    pub state: DownloadState,
    pub error: Option<String>,
}

struct StoreInner {
    root: PathBuf,
    next: AtomicU64,
    records: Mutex<Vec<DownloadRecord>>,
    /// `(label, url)` of downloads the browser announced as too large
    /// before they started; their start is refused.
    oversize_pending: Mutex<Vec<(String, String)>>,
    changed: tokio::sync::watch::Sender<u64>,
}

/// Every download of this process. Cheap to clone; every clone is the same
/// store.
#[derive(Clone)]
pub struct DownloadStore {
    inner: Arc<StoreInner>,
}

impl std::fmt::Debug for DownloadStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DownloadStore")
            .field("root", &self.inner.root)
            .finish()
    }
}

impl DownloadStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            inner: Arc::new(StoreInner {
                root,
                next: AtomicU64::new(0),
                records: Mutex::new(Vec::new()),
                oversize_pending: Mutex::new(Vec::new()),
                changed: tokio::sync::watch::channel(0).0,
            }),
        }
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    fn bump(&self) {
        self.inner.changed.send_modify(|seq| *seq += 1);
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.changed.subscribe()
    }

    /// The number the latest download got (0 before the first).
    pub fn last_number(&self) -> u64 {
        self.inner.next.load(Ordering::SeqCst)
    }

    /// A download of tab `tab_id` (`label`) starts: give it its own folder
    /// and return its id and the absolute path it must be written to.
    pub fn begin(
        &self,
        session_id: &str,
        tab_id: &str,
        label: &str,
        url: &str,
        suggested_name: &str,
        by_agent: bool,
    ) -> Result<(String, PathBuf), String> {
        let file_name = sanitize_file_name(suggested_name);
        let refused = {
            let mut pending = self.inner.oversize_pending.lock();
            pending
                .iter()
                .position(|(l, u)| l == label && u == url)
                .map(|index| pending.remove(index))
                .is_some()
        };
        let folder = self.inner.root.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&folder)
            .map_err(|error| format!("다운로드 폴더를 만들 수 없습니다: {error}"))?;
        let path = folder.join(&file_name);
        if refused {
            let _ = std::fs::remove_dir(&folder);
        }
        let number = self.inner.next.fetch_add(1, Ordering::SeqCst) + 1;
        let download_id = format!("d{number}");
        let display = suggested_name.trim();
        self.inner.records.lock().push(DownloadRecord {
            download_id: download_id.clone(),
            number,
            session_id: session_id.to_string(),
            tab_id: tab_id.to_string(),
            label: label.to_string(),
            url: url.to_string(),
            file_name: file_name.clone(),
            display_name: if display.is_empty() {
                file_name
            } else {
                shorten(display, MAX_DISPLAY_NAME_CHARS)
            },
            path: path.clone(),
            bytes: 0,
            mime: String::new(),
            sha256: String::new(),
            state: if refused {
                DownloadState::Failed
            } else {
                DownloadState::InProgress
            },
            error: refused.then(|| DOWNLOAD_TOO_LARGE.to_string()),
            by_agent,
            reported: false,
            oversize: refused,
            finished_at_ms: refused.then(now_ms),
        });
        self.bump();
        if refused {
            return Err(DOWNLOAD_TOO_LARGE.to_string());
        }
        Ok((download_id, path))
    }

    /// The oldest in-progress download of `label` from `url`: the one a
    /// "finished" event of that tab and URL is about.
    pub fn pending_for(&self, label: &str, url: &str) -> Option<String> {
        self.inner
            .records
            .lock()
            .iter()
            .find(|record| {
                record.state == DownloadState::InProgress
                    && record.label == label
                    && record.url == url
            })
            .map(|record| record.download_id.clone())
    }

    /// The browser reported the download from `url` on `label` will exceed
    /// [`MAX_DOWNLOAD_BYTES`]: it is being cancelled and will fail for that.
    /// The browser can say so before the download starts; that start is
    /// then refused ([`DownloadStore::begin`] records it as failed).
    pub fn mark_oversize(&self, label: &str, url: &str) {
        let mut records = self.inner.records.lock();
        if let Some(record) = records.iter_mut().find(|record| {
            record.state == DownloadState::InProgress && record.label == label && record.url == url
        }) {
            record.oversize = true;
        } else {
            self.inner
                .oversize_pending
                .lock()
                .push((label.to_string(), url.to_string()));
        }
    }

    /// Settle download `id` once the webview finished it: check its size,
    /// hash it and recognize what it is, or record why it failed. A file
    /// over the size limit is deleted. Reads the file, so call it off the UI
    /// thread.
    pub fn finalize(&self, id: &str, path: Option<PathBuf>, success: bool) {
        let Some(record) = self.find(id) else {
            return;
        };
        if record.state != DownloadState::InProgress {
            return;
        }
        let path = path.unwrap_or_else(|| record.path.clone());
        let outcome = if success {
            settle_file(&path)
        } else if record.oversize {
            Err(DOWNLOAD_TOO_LARGE.to_string())
        } else {
            Err("다운로드가 취소되었거나 실패했습니다. 페이지에서 다시 받거나 잠시 뒤 다시 시도하세요.".to_string())
        };
        if outcome.is_err() {
            remove_download_folder(&self.inner.root, &record.path);
        }
        let finished = now_ms();
        {
            let mut records = self.inner.records.lock();
            if let Some(record) = records.iter_mut().find(|record| record.download_id == id) {
                record.finished_at_ms = Some(finished);
                match outcome {
                    Ok(settled) => {
                        record.state = DownloadState::Completed;
                        record.bytes = settled.bytes;
                        record.mime = settled.mime;
                        record.sha256 = settled.sha256;
                        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                            record.file_name = name.to_string();
                        }
                        record.path = path;
                    }
                    Err(error) => {
                        record.state = DownloadState::Failed;
                        record.error = Some(error);
                    }
                }
            }
        }
        self.bump();
    }

    fn find(&self, id: &str) -> Option<DownloadRecord> {
        self.inner
            .records
            .lock()
            .iter()
            .find(|record| record.download_id == id)
            .cloned()
    }

    /// Download `id` of `session_id`. Another session's download is reported
    /// exactly like one that does not exist.
    pub fn get(&self, session_id: &str, id: &str) -> Result<DownloadRecord, String> {
        let id = id.trim();
        self.inner
            .records
            .lock()
            .iter()
            .find(|record| record.download_id == id && record.session_id == session_id)
            .cloned()
            .ok_or_else(|| DOWNLOAD_NOT_FOUND.to_string())
    }

    /// `session_id`'s downloads, oldest first.
    pub fn of_session(&self, session_id: &str) -> Vec<DownloadRecord> {
        self.inner
            .records
            .lock()
            .iter()
            .filter(|record| record.session_id == session_id)
            .cloned()
            .collect()
    }

    pub fn mark_reported(&self, id: &str) {
        if let Some(record) = self
            .inner
            .records
            .lock()
            .iter_mut()
            .find(|record| record.download_id == id)
        {
            record.reported = true;
        }
    }

    /// The download a `browser_download` call on `tab_id` is waiting for:
    /// the first one started after download number `after`, or, with no
    /// `after`, the newest one no call has returned yet.
    pub fn awaited(
        &self,
        session_id: &str,
        tab_id: &str,
        after: Option<u64>,
    ) -> Option<DownloadRecord> {
        let records = self.inner.records.lock();
        let mut mine = records
            .iter()
            .filter(|record| record.session_id == session_id && record.tab_id == tab_id);
        match after {
            Some(after) => mine.find(|record| record.number > after).cloned(),
            None => mine.rev().find(|record| !record.reported).cloned(),
        }
    }

    /// Wait (until `deadline`) for `pick` to return something.
    pub async fn wait_for<T>(
        &self,
        deadline: Instant,
        mut pick: impl FnMut(&DownloadStore) -> Option<T>,
    ) -> Option<T> {
        let mut changed = self.subscribe();
        loop {
            changed.borrow_and_update();
            if let Some(found) = pick(self) {
                return Some(found);
            }
            let wait = deadline.saturating_duration_since(Instant::now());
            if wait.is_zero() {
                return None;
            }
            // A missed notification costs at most one poll.
            let _ =
                tokio::time::timeout(wait.min(Duration::from_millis(500)), changed.changed()).await;
        }
    }

    /// The session was deleted: forget its downloads and delete their files.
    pub fn forget_session(&self, session_id: &str) {
        let removed = {
            let mut records = self.inner.records.lock();
            let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut *records)
                .into_iter()
                .partition(|record| record.session_id == session_id);
            *records = kept;
            gone
        };
        for record in &removed {
            if record.state != DownloadState::InProgress {
                remove_download_folder(&self.inner.root, &record.path);
            }
        }
        if !removed.is_empty() {
            self.bump();
        }
    }

    /// The profile was reset: forget every download (their folders go with
    /// the downloads directory).
    pub fn forget_all(&self) {
        let had = !std::mem::take(&mut *self.inner.records.lock()).is_empty();
        self.inner.oversize_pending.lock().clear();
        if had {
            self.bump();
        }
    }

    /// The chrome's list, newest first; `title_of` names each session.
    pub fn views(&self, title_of: impl Fn(&str) -> Option<String>) -> Vec<BrowserDownloadView> {
        self.inner
            .records
            .lock()
            .iter()
            .rev()
            .map(|record| BrowserDownloadView {
                download_id: record.download_id.clone(),
                tab_id: record.tab_id.clone(),
                session_id: record.session_id.clone(),
                session_title: title_of(&record.session_id).unwrap_or_else(|| "세션".to_string()),
                file_name: record.display_name.clone(),
                bytes: record.bytes,
                state: record.state,
                error: record.error.clone(),
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn complete_for_tests(&self, id: &str, path: &Path) {
        self.finalize(id, Some(path.to_path_buf()), true);
    }
}

struct SettledFile {
    bytes: u64,
    mime: String,
    sha256: String,
}

fn settle_file(path: &Path) -> Result<SettledFile, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("받은 파일을 찾을 수 없습니다({error}). 다시 받으세요."))?;
    if !metadata.is_file() {
        return Err("받은 항목이 파일이 아닙니다. 다시 받으세요.".to_string());
    }
    if metadata.len() > MAX_DOWNLOAD_BYTES {
        return Err(DOWNLOAD_TOO_LARGE.to_string());
    }
    if metadata.len() == 0 {
        return Err("받은 파일이 비어 있습니다. 페이지에서 다시 받으세요.".to_string());
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("받은 파일을 읽을 수 없습니다({error}). 다시 받으세요."))?;
    if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
        return Err(DOWNLOAD_TOO_LARGE.to_string());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    Ok(SettledFile {
        bytes: bytes.len() as u64,
        mime: detect_kind(&bytes, name).mime().to_string(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    })
}

/// Remove the per-download folder holding `path`, only when it is a direct
/// child of `root` (never anything above it).
fn remove_download_folder(root: &Path, path: &Path) {
    if let Some(folder) = path.parent().filter(|folder| folder.parent() == Some(root)) {
        let _ = std::fs::remove_dir_all(folder);
    }
}

/// Remove download folders (and stray files) under `root` whose newest file
/// is older than `max_age` at `now`; a folder with no file is removed too.
/// Returns how many entries were removed.
pub fn cleanup_stale(root: &Path, max_age: Duration, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let old = |modified: Option<SystemTime>| {
        modified.map_or(true, |modified| {
            now.duration_since(modified).unwrap_or_default() > max_age
        })
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            let newest = std::fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|file| file.metadata().ok()?.modified().ok())
                .max();
            if old(newest) && std::fs::remove_dir_all(&path).is_ok() {
                removed += 1;
            }
        } else if old(metadata.modified().ok()) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// A safe file name for a download: ASCII letters, digits, `-` and `_`
/// (anything else, path separators included, becomes `_`), a lowercase
/// alphanumeric extension, at most 100 characters, never a Windows device
/// name. An empty result is `download`.
pub fn sanitize_file_name(raw: &str) -> String {
    let name = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let name = name.trim_end_matches(['.', ' ']);
    let (stem, extension) = match name.rfind('.') {
        Some(index)
            if index > 0
                && name.len() - index - 1 <= MAX_EXTENSION_CHARS
                && name[index + 1..].chars().all(|c| c.is_ascii_alphanumeric())
                && index + 1 < name.len() =>
        {
            (&name[..index], name[index + 1..].to_ascii_lowercase())
        }
        _ => (name, String::new()),
    };
    let mut clean = String::new();
    for c in stem.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            c
        } else {
            '_'
        };
        if !(c == '_' && clean.ends_with('_')) {
            clean.push(c);
        }
    }
    let mut stem = clean.trim_matches('_').to_string();
    if stem.is_empty() {
        stem = "download".to_string();
    }
    let reserved = {
        let upper = stem.to_ascii_uppercase();
        matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((upper.starts_with("COM") || upper.starts_with("LPT"))
                && upper.len() == 4
                && upper.as_bytes()[3].is_ascii_digit())
    };
    if reserved {
        stem.insert(0, '_');
    }
    let room = MAX_FILE_NAME_CHARS
        - if extension.is_empty() {
            0
        } else {
            extension.len() + 1
        };
    stem.truncate(room);
    if extension.is_empty() {
        stem
    } else {
        format!("{stem}.{extension}")
    }
}

/// What a downloaded file is, by its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadKind {
    Audio {
        mime: &'static str,
        extension: &'static str,
    },
    Image {
        mime: &'static str,
    },
    Other {
        mime: &'static str,
    },
}

impl DownloadKind {
    pub fn mime(self) -> &'static str {
        match self {
            Self::Audio { mime, .. } | Self::Image { mime } | Self::Other { mime } => mime,
        }
    }
}

/// Recognize a download by its magic bytes (audio as the attachment path
/// does, images incl. BMP); text by UTF-8 content and its name's extension.
pub fn detect_kind(bytes: &[u8], file_name: &str) -> DownloadKind {
    if let Some((mime, extension)) = crate::attachment::detect_audio(bytes) {
        return DownloadKind::Audio { mime, extension };
    }
    if let Some((mime, _)) = crate::attachment::detect_image(bytes) {
        return DownloadKind::Image { mime };
    }
    if is_bmp(bytes) {
        return DownloadKind::Image { mime: "image/bmp" };
    }
    let mime = if bytes.starts_with(b"PK\x03\x04") {
        "application/zip"
    } else if bytes.starts_with(b"%PDF-") {
        "application/pdf"
    } else if is_text(bytes) {
        match extension_of(file_name).as_str() {
            "json" => "application/json",
            "csv" => "text/csv",
            "md" => "text/markdown",
            "html" | "htm" => "text/html",
            _ => "text/plain",
        }
    } else {
        "application/octet-stream"
    };
    DownloadKind::Other { mime }
}

fn is_bmp(bytes: &[u8]) -> bool {
    bytes.len() >= 26 && bytes.starts_with(b"BM")
}

fn is_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

fn extension_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Width and height of a recognized image.
pub fn image_dimensions(bytes: &[u8], mime: &str) -> Option<(u32, u32)> {
    if mime == "image/bmp" {
        if !is_bmp(bytes) {
            return None;
        }
        let width = i32::from_le_bytes(bytes[18..22].try_into().ok()?);
        let height = i32::from_le_bytes(bytes[22..26].try_into().ok()?);
        return Some((width.unsigned_abs(), height.unsigned_abs()));
    }
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// The image envelope (`{"image":{mimeType,width,height,data}}` fields) the
/// model sees a downloaded image through, when providers accept its format
/// and it is small enough; `None` otherwise.
pub fn image_envelope(bytes: &[u8], mime: &str) -> Option<Value> {
    use base64::Engine as _;
    if !matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) || bytes.len() > MAX_IMAGE_RESULT_BYTES
    {
        return None;
    }
    let (width, height) = image_dimensions(bytes, mime)?;
    Some(json!({
        "mimeType": mime,
        "width": width,
        "height": height,
        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
    }))
}

/// Where `browser_save` may write `bytes` at `requested` under the
/// (canonical) project `root`. Refused: anything outside the root, `.git`,
/// agent settings and state, sources (`src/`, written through the file
/// tools), DAT documents, maps and references, build outputs, the project
/// manifest; any extension but [`SAVE_EXTENSIONS`]; content that is not
/// what the extension says; and an existing file unless `overwrite`.
pub fn save_target(
    root: &Path,
    requested: &str,
    bytes: &[u8],
    overwrite: bool,
) -> Result<(String, PathBuf), String> {
    let (relative, path) = crate::fs_tools::resolve(root, requested)
        .map_err(|error| format!("저장할 경로 '{requested}' 가 올바르지 않습니다: {error}"))?;
    let first = relative
        .split('/')
        .next()
        .unwrap_or(&relative)
        .to_ascii_lowercase();
    let at_root = !relative.contains('/');
    // The project manifest, current (`project.eap`, any root `*.eap`) or
    // legacy (`project.json`, `*.eudproj`).
    let manifest = at_root
        && (first.ends_with(".eap") || first == "project.json" || first.ends_with(".eudproj"));
    let reason = match first.as_str() {
        ".git" => Some("git 히스토리는 이 프로젝트의 되돌리기 수단 자체입니다."),
        ".claude" | ".codex" | ".mcp.json" => {
            Some("에이전트 권한 설정은 세션이 스스로 바꾸지 않습니다.")
        }
        _ if crate::fs_tools::is_agent_instruction_path(&relative) => Some(
            "에이전트 CLI가 읽는 지침 파일(CLAUDE.md, AGENTS.md, GEMINI.md, SKILL.md, .mcp.json 이나 .gemini/.cursor/.github 아래)은 어느 폴더에서든 내려받은 파일로 바꾸지 않습니다.",
        ),
        ".vscode" | ".idea" | ".github" => Some(
            "편집기·CI 설정 폴더는 내려받은 파일로 바꾸지 않습니다(작업·워크플로를 실행할 수 있습니다).",
        ),
        ".eud-agent" => Some("앱 상태 폴더입니다. 문서는 fs_write 로 쓰세요."),
        "src" => Some("소스는 file_write/fs_write 같은 파일 도구로만 바꿉니다."),
        "dat" => Some("DAT 문서는 dat_patch 로만 바꿉니다."),
        "maps" | "references" => {
            Some("맵 파일은 MapSafe 검증을 거치는 map_* 도구나 Map 세션으로만 바꿉니다.")
        }
        "build" => Some("빌드 출력은 build_run 이 만듭니다."),
        "compat" => Some("호환 파일은 가져오기/내보내기가 만듭니다."),
        _ if manifest => Some("프로젝트 매니페스트입니다."),
        _ => None,
    };
    if let Some(reason) = reason {
        return Err(format!(
            "'{relative}' 에는 저장할 수 없습니다: {reason} 예: assets/sounds/<이름>.ogg"
        ));
    }
    let extension = extension_of(&relative);
    if !SAVE_EXTENSIONS.contains(&extension.as_str()) {
        return Err(format!(
            "'{relative}' 확장자로는 저장할 수 없습니다. 저장할 수 있는 확장자: {}",
            SAVE_EXTENSIONS.join(", ")
        ));
    }
    let fits = match extension.as_str() {
        "ogg" | "wav" | "mp3" | "flac" => crate::attachment::detect_audio(bytes)
            .is_some_and(|(_, detected)| detected == extension),
        "png" => {
            crate::attachment::detect_image(bytes).is_some_and(|(mime, _)| mime == "image/png")
        }
        "jpg" | "jpeg" => {
            crate::attachment::detect_image(bytes).is_some_and(|(mime, _)| mime == "image/jpeg")
        }
        "gif" => {
            crate::attachment::detect_image(bytes).is_some_and(|(mime, _)| mime == "image/gif")
        }
        "webp" => {
            crate::attachment::detect_image(bytes).is_some_and(|(mime, _)| mime == "image/webp")
        }
        "bmp" => is_bmp(bytes),
        _ => is_text(bytes),
    };
    if !fits {
        let kind = detect_kind(bytes, &relative).mime();
        return Err(format!(
            "받은 파일은 {kind} 인데 경로의 확장자는 .{extension} 입니다. 파일 내용에 맞는 확장자로 저장하세요."
        ));
    }
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {
            return Err(format!("'{relative}' 은(는) 폴더입니다. 파일 이름까지 주세요."))
        }
        Ok(_) if !overwrite => {
            return Err(format!(
                "'{relative}' 가 이미 있습니다. 덮어쓰려면 overwrite: true 로 다시 호출하거나 다른 이름을 주세요."
            ))
        }
        _ => {}
    }
    Ok((relative, path))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn shorten(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(chars).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "eud-browser-downloads-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const OGG: &[u8] = b"OggS\0\x02rest-of-a-page";
    const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgb);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![0; (width * height * 3) as usize])
                .unwrap();
        }
        bytes
    }

    #[test]
    fn file_names_are_ascii_safe_bounded_and_keep_their_extension() {
        assert_eq!(sanitize_file_name("sound.ogg"), "sound.ogg");
        assert_eq!(sanitize_file_name("My Sound (1).OGG"), "My_Sound_1.ogg");
        assert_eq!(sanitize_file_name("효과음.ogg"), "download.ogg");
        assert_eq!(sanitize_file_name("효과음 01.wav"), "01.wav");
        assert_eq!(sanitize_file_name("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(sanitize_file_name("../a/b/c.png"), "c.png");
        assert_eq!(sanitize_file_name(""), "download");
        assert_eq!(sanitize_file_name(".bashrc"), "bashrc");
        assert_eq!(sanitize_file_name("archive.tar.gz"), "archive_tar.gz");
        assert_eq!(sanitize_file_name("name.weird ext"), "name_weird_ext");
        assert_eq!(sanitize_file_name("trailing.png. "), "trailing.png");
        assert_eq!(sanitize_file_name("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_file_name("com1"), "_com1");
        assert_eq!(sanitize_file_name("console.txt"), "console.txt");
        let long = sanitize_file_name(&format!("{}.flac", "a".repeat(500)));
        assert_eq!(long.len(), MAX_FILE_NAME_CHARS);
        assert!(long.ends_with(".flac"));
        let no_ext = sanitize_file_name(&"b".repeat(500));
        assert_eq!(no_ext.len(), MAX_FILE_NAME_CHARS);
    }

    #[test]
    fn kinds_follow_magic_bytes_not_names() {
        assert_eq!(
            detect_kind(OGG, "x.bin"),
            DownloadKind::Audio {
                mime: "audio/ogg",
                extension: "ogg"
            }
        );
        assert_eq!(
            detect_kind(&png(2, 2), "x.ogg"),
            DownloadKind::Image { mime: "image/png" }
        );
        assert_eq!(
            detect_kind(b"\xff\xd8\xff\xe0rest", "x"),
            DownloadKind::Image { mime: "image/jpeg" }
        );
        assert_eq!(
            detect_kind(b"GIF89a\x01\x00\x01\x00", "x"),
            DownloadKind::Image { mime: "image/gif" }
        );
        let mut bmp = b"BM".to_vec();
        bmp.resize(26, 0);
        bmp[18] = 4;
        bmp[22] = 0xfd;
        bmp[23..26].copy_from_slice(&[0xff, 0xff, 0xff]);
        assert_eq!(
            detect_kind(&bmp, "x"),
            DownloadKind::Image { mime: "image/bmp" }
        );
        assert_eq!(image_dimensions(&bmp, "image/bmp"), Some((4, 3)));
        assert_eq!(
            detect_kind(b"{\"a\":1}", "data.json").mime(),
            "application/json"
        );
        assert_eq!(detect_kind(b"a,b\n1,2", "t.csv").mime(), "text/csv");
        assert_eq!(detect_kind(b"hello", "t").mime(), "text/plain");
        assert_eq!(detect_kind(b"PK\x03\x04..", "t").mime(), "application/zip");
        assert_eq!(
            detect_kind(b"\x00\x01\x02\xff", "t.txt").mime(),
            "application/octet-stream"
        );
    }

    #[test]
    fn only_small_provider_images_become_envelopes() {
        let bytes = png(3, 2);
        let envelope = image_envelope(&bytes, "image/png").unwrap();
        assert_eq!(envelope["width"], 3);
        assert_eq!(envelope["height"], 2);
        assert_eq!(envelope["mimeType"], "image/png");
        assert!(envelope["data"].as_str().unwrap().len() > 10);
        let mut bmp = b"BM".to_vec();
        bmp.resize(26, 0);
        assert!(
            image_envelope(&bmp, "image/bmp").is_none(),
            "providers take no BMP"
        );
        let mut large = bytes.clone();
        large.resize(MAX_IMAGE_RESULT_BYTES + 1, 0);
        assert!(image_envelope(&large, "image/png").is_none());
        assert!(
            image_envelope(PNG_SIG, "image/png").is_none(),
            "no IHDR, no size"
        );
    }

    #[test]
    fn finished_downloads_are_hashed_recognized_and_capped() {
        let root = temp_dir("finalize");
        let store = DownloadStore::new(root.clone());
        let (id, path) = store
            .begin(
                "s1",
                "t1",
                "browser-tab-1",
                "https://a.example/x.ogg",
                "x.ogg",
                true,
            )
            .unwrap();
        assert_eq!(id, "d1");
        assert!(path.starts_with(&root));
        assert_eq!(path.file_name().unwrap(), "x.ogg");
        assert_eq!(
            store.pending_for("browser-tab-1", "https://a.example/x.ogg"),
            Some(id.clone())
        );
        std::fs::write(&path, OGG).unwrap();
        store.finalize(&id, Some(path.clone()), true);
        let record = store.get("s1", &id).unwrap();
        assert_eq!(record.state, DownloadState::Completed);
        assert_eq!(record.bytes, OGG.len() as u64);
        assert_eq!(record.mime, "audio/ogg");
        assert_eq!(record.sha256, format!("{:x}", Sha256::digest(OGG)));
        assert!(record.finished_at_ms.is_some());
        assert!(store
            .pending_for("browser-tab-1", "https://a.example/x.ogg")
            .is_none());
        let value = record.tool_value();
        assert!(
            value.get("path").is_none(),
            "the local path never reaches the model"
        );
        assert_eq!(value["bytes"], OGG.len() as u64);

        // Over the cap: failed, and the file is gone.
        let (big, big_path) = store
            .begin(
                "s1",
                "t1",
                "browser-tab-1",
                "https://a.example/big",
                "big.bin",
                false,
            )
            .unwrap();
        let file = std::fs::File::create(&big_path).unwrap();
        file.set_len(MAX_DOWNLOAD_BYTES + 1).unwrap();
        drop(file);
        store.finalize(&big, None, true);
        let record = store.get("s1", &big).unwrap();
        assert_eq!(record.state, DownloadState::Failed);
        assert_eq!(record.error.as_deref(), Some(DOWNLOAD_TOO_LARGE));
        assert!(!big_path.exists());
        assert!(!big_path.parent().unwrap().exists());

        // Exactly the cap is kept.
        let (edge, edge_path) = store
            .begin(
                "s1",
                "t1",
                "browser-tab-1",
                "https://a.example/edge",
                "edge.bin",
                false,
            )
            .unwrap();
        let file = std::fs::File::create(&edge_path).unwrap();
        file.set_len(MAX_DOWNLOAD_BYTES).unwrap();
        drop(file);
        store.finalize(&edge, None, true);
        assert_eq!(
            store.get("s1", &edge).unwrap().state,
            DownloadState::Completed
        );

        // A cancelled oversize download says why.
        let (cancelled, _) = store
            .begin(
                "s1",
                "t1",
                "browser-tab-1",
                "https://a.example/huge",
                "huge.bin",
                false,
            )
            .unwrap();
        store.mark_oversize("browser-tab-1", "https://a.example/huge");
        store.finalize(&cancelled, None, false);
        assert_eq!(
            store.get("s1", &cancelled).unwrap().error.as_deref(),
            Some(DOWNLOAD_TOO_LARGE)
        );
        // Announced as too large before it started: the start is refused and
        // the waiting call sees why.
        store.mark_oversize("browser-tab-1", "https://a.example/early");
        let refused = store
            .begin(
                "s1",
                "t1",
                "browser-tab-1",
                "https://a.example/early",
                "early.bin",
                true,
            )
            .unwrap_err();
        assert_eq!(refused, DOWNLOAD_TOO_LARGE);
        let early = store.awaited("s1", "t1", Some(0)).map(|_| ());
        assert!(early.is_some());
        let record = store
            .of_session("s1")
            .into_iter()
            .find(|record| record.url == "https://a.example/early")
            .unwrap();
        assert_eq!(record.state, DownloadState::Failed);
        assert!(!record.path.parent().unwrap().exists());
        assert!(
            store
                .begin(
                    "s1",
                    "t1",
                    "browser-tab-1",
                    "https://a.example/early",
                    "again.bin",
                    true
                )
                .is_ok(),
            "the refusal is used once"
        );
        let (failed, _) = store
            .begin(
                "s1",
                "t1",
                "browser-tab-1",
                "https://a.example/f",
                "f.bin",
                false,
            )
            .unwrap();
        store.finalize(&failed, None, false);
        assert!(store
            .get("s1", &failed)
            .unwrap()
            .error
            .unwrap()
            .contains("취소"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_session_sees_only_its_own_downloads() {
        let root = temp_dir("sessions");
        let store = DownloadStore::new(root.clone());
        let (a, a_path) = store
            .begin(
                "session-a",
                "t1",
                "browser-tab-1",
                "https://x/a.png",
                "a.png",
                false,
            )
            .unwrap();
        let (b, _) = store
            .begin(
                "session-b",
                "t2",
                "browser-tab-2",
                "https://x/b.png",
                "b.png",
                true,
            )
            .unwrap();
        assert_eq!(store.get("session-a", &a).unwrap().download_id, "d1");
        assert_eq!(store.get("session-a", " d1 ").unwrap().download_id, "d1");
        assert_eq!(store.get("session-a", &b).unwrap_err(), DOWNLOAD_NOT_FOUND);
        assert_eq!(store.get("session-b", &a).unwrap_err(), DOWNLOAD_NOT_FOUND);
        assert_eq!(
            store
                .of_session("session-b")
                .iter()
                .map(|record| record.download_id.as_str())
                .collect::<Vec<_>>(),
            ["d2"]
        );
        assert!(store.awaited("session-a", "t2", None).is_none());
        assert_eq!(
            store
                .awaited("session-b", "t2", Some(0))
                .unwrap()
                .download_id,
            "d2"
        );

        std::fs::write(&a_path, png(1, 1)).unwrap();
        store.complete_for_tests(&a, &a_path);
        store.forget_session("session-a");
        assert!(store.of_session("session-a").is_empty());
        assert!(!a_path.exists(), "a deleted session's files go with it");
        assert_eq!(store.of_session("session-b").len(), 1);
        let views = store.views(|session| (session == "session-b").then(|| "B".to_string()));
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].session_title, "B");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn awaited_downloads_are_the_next_one_or_the_newest_unreported() {
        let root = temp_dir("awaited");
        let store = DownloadStore::new(root.clone());
        assert_eq!(store.last_number(), 0);
        store.begin("s", "t1", "l1", "u1", "a", false).unwrap();
        let before = store.last_number();
        assert!(store.awaited("s", "t1", Some(before)).is_none());
        store.begin("s", "t2", "l2", "u2", "b", false).unwrap();
        assert!(
            store.awaited("s", "t1", Some(before)).is_none(),
            "another tab's"
        );
        store.begin("s", "t1", "l1", "u3", "c", false).unwrap();
        assert_eq!(
            store.awaited("s", "t1", Some(before)).unwrap().download_id,
            "d3"
        );
        assert_eq!(store.awaited("s", "t1", None).unwrap().download_id, "d3");
        store.mark_reported("d3");
        assert_eq!(store.awaited("s", "t1", None).unwrap().download_id, "d1");
        store.mark_reported("d1");
        assert!(store.awaited("s", "t1", None).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn waiting_ends_when_the_download_settles_or_the_time_is_up() {
        let root = temp_dir("wait");
        let store = DownloadStore::new(root.clone());
        let none = store
            .wait_for(Instant::now() + Duration::from_millis(50), |store| {
                store.awaited("s", "t1", Some(0))
            })
            .await;
        assert!(none.is_none());
        let (id, path) = store.begin("s", "t1", "l1", "u", "x.ogg", true).unwrap();
        std::fs::write(&path, OGG).unwrap();
        let settle = store.clone();
        let settled_id = id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            settle.finalize(&settled_id, None, true);
        });
        let done = store
            .wait_for(Instant::now() + Duration::from_secs(5), |store| {
                store
                    .awaited("s", "t1", Some(0))
                    .filter(|record| record.state != DownloadState::InProgress)
            })
            .await
            .unwrap();
        assert_eq!(done.download_id, id);
        assert_eq!(done.state, DownloadState::Completed);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stale_downloads_are_removed_after_a_day() {
        let root = temp_dir("cleanup");
        let now = SystemTime::now();
        let old_dir = root.join("old");
        let fresh_dir = root.join("fresh");
        let empty_dir = root.join("empty");
        for dir in [&old_dir, &fresh_dir, &empty_dir] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let old_file = old_dir.join("a.ogg");
        std::fs::write(&old_file, b"x").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&old_file)
            .unwrap()
            .set_modified(now - Duration::from_secs(25 * 3600))
            .unwrap();
        std::fs::write(fresh_dir.join("b.ogg"), b"y").unwrap();
        let stray = root.join("stray.tmp");
        std::fs::write(&stray, b"z").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&stray)
            .unwrap()
            .set_modified(now - Duration::from_secs(48 * 3600))
            .unwrap();

        assert_eq!(cleanup_stale(&root, DOWNLOAD_RETENTION, now), 3);
        assert!(!old_dir.exists());
        assert!(!empty_dir.exists());
        assert!(!stray.exists());
        assert!(fresh_dir.join("b.ogg").exists());
        assert_eq!(
            cleanup_stale(&root.join("missing"), DOWNLOAD_RETENTION, now),
            0
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_page_reported_blob_size_is_checked_before_reading() {
        assert_eq!(page_blob_size(Some(1)), Ok(1));
        assert_eq!(
            page_blob_size(Some(MAX_DOWNLOAD_BYTES)),
            Ok(MAX_DOWNLOAD_BYTES)
        );
        assert_eq!(
            page_blob_size(Some(MAX_DOWNLOAD_BYTES + 1)),
            Err(DOWNLOAD_TOO_LARGE.to_string())
        );
        assert!(page_blob_size(Some(0)).is_err());
        assert!(page_blob_size(None).is_err());
    }

    #[test]
    fn saves_stay_in_the_project_away_from_its_authority_files() {
        let root = std::fs::canonicalize(temp_dir("save")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let image = png(2, 2);

        let (relative, path) = save_target(&root, "assets/sounds/hit.ogg", OGG, false).unwrap();
        assert_eq!(relative, "assets/sounds/hit.ogg");
        assert!(path.starts_with(&root));
        assert_eq!(
            save_target(&root, "assets\\a.png", &image, false)
                .unwrap()
                .0,
            "assets/a.png"
        );
        assert!(save_target(&root, "notes.md", b"# hi", false).is_ok());
        assert!(save_target(&root, "docs/agents-notes.md", b"# hi", false).is_ok());
        assert!(save_target(&root, "data/table.csv", b"a,b", false).is_ok());
        assert!(save_target(&root, "img/photo.JPG", b"\xff\xd8\xff\xe0", false).is_ok());

        for refused in [
            "../outside.ogg",
            "assets/../../outside.ogg",
            "/etc/x.ogg",
            ".git/hooks/x.ogg",
            ".GIT/x.ogg",
            "src/sound.ogg",
            "SRC/main.ogg",
            "dat/standard.json",
            "maps/source.ogg",
            "references/x.ogg",
            "build/x.ogg",
            ".eud-agent/workspace/x.ogg",
            ".claude/x.json",
            "project.json",
            // Windows spellings of the same folders, and names it rewrites.
            ".claude./settings.json",
            ".claude /settings.json",
            "dat./standard.json",
            "src ./x.json",
            "Src/x.json",
            "assets/x:y.png",
            "COM1.png",
            "assets/nul.json",
            // Instructions and settings agent CLIs and editors load from the
            // project, in any case.
            "CLAUDE.md",
            "claude.MD",
            "AGENTS.md",
            "GEMINI.md",
            ".mcp.json",
            ".Mcp.Json",
            ".claude/commands/x.md",
            ".codex/config.json",
            ".vscode/settings.json",
            ".VSCode/tasks.json",
            ".idea/workspace.json",
            ".github/workflows/ci.json",
            ".GitHub/x.md",
            // Instruction files at any depth.
            "assets/CLAUDE.md",
            "docs/AGENTS.md",
            "a/b/gemini.md",
            "skills/x/SKILL.md",
            "sub/.mcp.json",
            ".gemini/styleguide.md",
            ".cursor/rules.md",
        ] {
            let lower = refused.to_ascii_lowercase();
            let content: &[u8] = if lower.ends_with(".json") {
                b"{}"
            } else if lower.ends_with(".md") {
                b"# valid text"
            } else {
                OGG
            };
            assert!(
                save_target(&root, refused, content, false).is_err(),
                "{refused} is refused"
            );
        }
        // The manifest, current and legacy, is named as such.
        for manifest in ["project.eap", "Arena.EAP", "project.json", "old.eudproj"] {
            let error = save_target(&root, manifest, b"{}", false).unwrap_err();
            assert!(error.contains("매니페스트"), "{manifest}: {error}");
        }
        assert!(save_target(&root, "data/project.json", b"{}", false).is_ok());
        for extension in ["eap", "scx", "scm", "eps", "py", "exe", "zip", "html"] {
            let error =
                save_target(&root, &format!("assets/x.{extension}"), b"x", false).unwrap_err();
            assert!(error.contains("확장자"), "{extension}: {error}");
        }
        // Content must be what the extension says.
        assert!(save_target(&root, "assets/x.ogg", &image, false)
            .unwrap_err()
            .contains("image/png"));
        assert!(save_target(&root, "assets/x.png", OGG, false).is_err());
        assert!(save_target(&root, "assets/x.wav", OGG, false).is_err());
        assert!(save_target(&root, "assets/x.txt", b"\x00\x01", false).is_err());

        // Overwrite only when asked; never a folder.
        std::fs::write(root.join("assets/hit.ogg"), OGG).unwrap();
        let error = save_target(&root, "assets/hit.ogg", OGG, false).unwrap_err();
        assert!(error.contains("overwrite"), "{error}");
        assert!(save_target(&root, "assets/hit.ogg", OGG, true).is_ok());
        std::fs::create_dir_all(root.join("assets/dir.ogg")).unwrap();
        assert!(save_target(&root, "assets/dir.ogg", OGG, true)
            .unwrap_err()
            .contains("폴더"));
        let _ = std::fs::remove_dir_all(root);
    }
}
