//! MapVault maker API (`/api/v1` of the MapVault site): the address and key
//! settings, and the pieces `mapvault_request` / `mapvault_sdk_install` use.
//!
//! The API key lives only in the OS credential store
//! (`eud-agent/integrations/mapvault/api-key`); it is never written to config,
//! logs, IPC responses or tool results, and it is sent only to the configured
//! origin. The address is chosen by the user in Settings → MapVault; no agent
//! tool can read the key or change the address.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::config::DataDirs;

pub const DEFAULT_ADDRESS: &str = "https://sc.mi3.dev";
pub const API_BASE: &str = "/api/v1";
/// The SDK file `import mapvault;` loads; its name must not change.
pub const SDK_SOURCE_PATH: &str = "src/mapvault.eps";
/// Largest map file the upload route accepts (`MAX_MAP_UPLOAD` in mapfile.ts).
pub const MAX_MAP_UPLOAD: u64 = 300 * 1024 * 1024;
/// The upload rate an upload's time budget is planned for, bytes per second.
pub const UPLOAD_PLAN_RATE: u64 = 1024 * 1024;
/// Connection, scmscx's own checks and the answer, on top of the bytes.
const UPLOAD_OVERHEAD: Duration = Duration::from_secs(20);

/// The time an upload of `len` bytes is given to send: the overhead plus
/// the bytes at [`UPLOAD_PLAN_RATE`].
pub fn upload_send_time(len: u64) -> Duration {
    UPLOAD_OVERHEAD + Duration::from_secs(len.div_ceil(UPLOAD_PLAN_RATE))
}

/// The hashing rate the time budget reserves before an upload's card, bytes
/// per second (reading the file once; well below a disk's).
pub const HASH_PLAN_RATE: u64 = 50 * 1024 * 1024;

/// The largest file (bytes) that can be hashed and then sent within a call
/// of `bound` with a card shown for at least `card`: the bytes go through
/// hashing at [`HASH_PLAN_RATE`] and sending at [`UPLOAD_PLAN_RATE`].
pub fn practical_upload_limit(bound: Duration, card: Duration) -> u64 {
    let seconds = bound
        .saturating_sub(UPLOAD_OVERHEAD)
        .saturating_sub(card)
        .as_secs();
    let per_second = UPLOAD_PLAN_RATE * HASH_PLAN_RATE / (UPLOAD_PLAN_RATE + HASH_PLAN_RATE);
    seconds.saturating_mul(per_second).min(MAX_MAP_UPLOAD)
}

const SECRET_INTEGRATION: &str = "mapvault";
const SECRET_NAME: &str = "api-key";
const MAX_ADDRESS_LEN: usize = 512;
const MAX_PATH_LEN: usize = 2048;

pub const ERR_ADDRESS_INVALID: &str = "mapvault_address_invalid";
pub const ERR_KEY_FORMAT: &str = "mapvault_key_format";
pub const ERR_KEY_STORE: &str = "mapvault_key_store";
/// The site answered 401: the key is not valid there.
pub const ERR_KEY_INVALID: &str = "mapvault_key_invalid";
/// No answer from the address (network, DNS, timeout).
pub const ERR_UNREACHABLE: &str = "mapvault_unreachable";
/// An answer that is not the maker API's (a redirect, another status, not its JSON).
pub const ERR_UNAVAILABLE: &str = "mapvault_unavailable";
/// How long Settings waits for the key check.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);

/// Where a tool reads the key from: the OS store in the app, memory in tests.
pub type KeySource = Arc<dyn Fn() -> Result<Option<Zeroizing<String>>, String> + Send + Sync>;

pub fn production_key_source() -> KeySource {
    Arc::new(read_api_key)
}

pub fn read_api_key() -> Result<Option<Zeroizing<String>>, String> {
    crate::provider_secrets::read_integration_secret(SECRET_INTEGRATION, SECRET_NAME)
}

/// The origin (`scheme://host[:port]`) of a site address the user typed, or
/// `None` when it is not one: https only (http only for a loopback host, for
/// a developer's own server), no credentials, path, query or fragment.
pub fn normalize_address(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > MAX_ADDRESS_LEN
        || raw
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return None;
    }
    let url = reqwest::Url::parse(raw).ok()?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return None;
    }
    let loopback = is_loopback(&url);
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => return None,
    }
    Some(url.origin().ascii_serialization())
}

/// Whether `url`'s host is this computer (`localhost` or a loopback IP).
fn is_loopback(url: &reqwest::Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    })
}

/// Whether `key` has the shape of a MapVault key (`mv_` + 32 base32
/// characters, `apikey.ts`).
pub fn is_key_shaped(key: &str) -> bool {
    key.strip_prefix("mv_").is_some_and(|rest| {
        rest.len() == 32
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
    })
}

/// A path under `/api/v1`, checked before anything is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiPath {
    /// `/maps/my-map/versions/12` (no trailing slash; `/` is the reference).
    pub path: String,
    pub segments: Vec<String>,
    pub query: Option<String>,
}

/// Check a model-supplied path relative to `/api/v1`. It must start with `/`
/// and stay under the API: no scheme or host, no `.`/`..` or empty segment,
/// no backslash, percent sign or control character in the path, no
/// fragment. Path segments are ASCII URL-safe characters; the query may hold
/// letters of any script (a nickname search) and percent escapes.
pub fn parse_api_path(raw: &str) -> Result<ApiPath, String> {
    let refuse = |why: &str| {
        Err(format!(
            "invalid MapVault path: {why}. Give a path relative to /api/v1 that starts with '/', e.g. /maps or /maps/<slug>/versions/<id>."
        ))
    };
    if raw.is_empty() || raw.len() > MAX_PATH_LEN {
        return refuse("empty or too long");
    }
    if !raw.starts_with('/') {
        return refuse("it must start with '/' (absolute URLs are not accepted)");
    }
    if raw.starts_with("//") {
        return refuse("it must not start with '//'");
    }
    let (path, query) = match raw.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (raw, None),
    };
    let mut segments: Vec<String> = Vec::new();
    let mut parts = path[1..].split('/').peekable();
    while let Some(segment) = parts.next() {
        if segment.is_empty() {
            // Only a trailing slash (or the bare `/`) leaves an empty part.
            if parts.peek().is_none() {
                break;
            }
            return refuse("empty path segment");
        }
        if segment == "." || segment == ".." {
            return refuse("'.' and '..' segments are not allowed");
        }
        if !segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
        {
            return refuse(
                "path segments may hold only ASCII letters, digits and - _ . ~ (put search text in the query)",
            );
        }
        segments.push(segment.to_string());
    }
    if let Some(query) = query {
        if query.is_empty() {
            return refuse("empty query");
        }
        if !query.chars().all(|c| {
            (c.is_ascii_graphic() && !matches!(c, '#' | '\\'))
                || (!c.is_ascii() && c.is_alphanumeric())
        }) {
            return refuse("the query may not hold spaces, control characters, '#' or '\\'");
        }
    }
    let path = if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    };
    Ok(ApiPath {
        path,
        segments,
        query: query.map(str::to_string),
    })
}

/// The URL of `path` under the API at `origin`, refused unless it is still on
/// that origin and under `/api/v1`.
pub fn api_url(origin: &str, path: &ApiPath) -> Result<reqwest::Url, String> {
    let mut text = format!("{origin}{API_BASE}{}", path.path);
    if let Some(query) = &path.query {
        text.push('?');
        text.push_str(query);
    }
    let url = reqwest::Url::parse(&text).map_err(|_| "invalid MapVault path".to_string())?;
    let under_api = url.path() == API_BASE || url.path().starts_with(&format!("{API_BASE}/"));
    if url.origin().ascii_serialization() != origin || !under_api || url.fragment().is_some() {
        return Err("invalid MapVault path: it leaves the configured site's /api/v1".to_string());
    }
    Ok(url)
}

/// What a request does that needs the user's 허용 first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    MapEdit,
    VersionDelete,
    Release,
    Block,
    StatusChange,
    FileLink,
    FileUpload,
    FileUnlink,
    PlayerEdit,
    PlayerReset,
    PlayerBlock,
    PlayerUnblock,
    DataImport { overwrite: bool },
    WaitingDelete,
    Unknown,
}

impl Action {
    /// Whether the card says the step cannot be undone.
    pub fn irreversible(self) -> bool {
        matches!(
            self,
            Self::MapEdit
                | Self::Release
                | Self::FileUpload
                | Self::PlayerReset
                | Self::VersionDelete
                | Self::DataImport { overwrite: true }
        )
    }

    /// The locale key suffix of this action's card text.
    pub fn code(self) -> &'static str {
        match self {
            Self::MapEdit => "map_edit",
            Self::VersionDelete => "version_delete",
            Self::Release => "release",
            Self::Block => "block",
            Self::StatusChange => "status_change",
            Self::FileLink => "file_link",
            Self::FileUpload => "file_upload",
            Self::FileUnlink => "file_unlink",
            Self::PlayerEdit => "player_edit",
            Self::PlayerReset => "player_reset",
            Self::PlayerBlock => "player_block",
            Self::PlayerUnblock => "player_unblock",
            Self::DataImport { overwrite: false } => "data_import",
            Self::DataImport { overwrite: true } => "data_import_overwrite",
            Self::WaitingDelete => "waiting_delete",
            Self::Unknown => "unknown",
        }
    }
}

/// How `mapvault_request` treats one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Sent at once.
    Free,
    /// Sent only after the user allows it on a card.
    Confirm(Action),
    /// Never sent by `mapvault_request` (the SDK download holds the version's
    /// secret key; `mapvault_sdk_install` handles it).
    Sdk,
}

#[derive(Clone, Copy)]
enum Kind {
    Free,
    Confirm(Action),
    /// `POST …/status`: release, block, or another status.
    Status,
    /// `POST …/players/:uid/block`: block or unblock.
    PlayerBlock,
    /// `POST …/import`: a dry run changes nothing.
    Import,
    Sdk,
}

/// Every route of the MapVault maker API (`routes` in `api.ts`).
const ROUTES: &[(&str, &str, Kind)] = &[
    ("GET", "/me", Kind::Free),
    ("GET", "/maps", Kind::Free),
    ("POST", "/maps", Kind::Free),
    ("GET", "/maps/:slug", Kind::Free),
    ("PATCH", "/maps/:slug", Kind::Confirm(Action::MapEdit)),
    ("POST", "/images", Kind::Free),
    ("POST", "/maps/:slug/versions", Kind::Free),
    ("GET", "/maps/:slug/versions/:id", Kind::Free),
    // Schema and key alphabet change only while a draft (the server refuses
    // them afterwards); camera, stream and screen language only change the
    // next SDK and build.
    ("PATCH", "/maps/:slug/versions/:id", Kind::Free),
    (
        "DELETE",
        "/maps/:slug/versions/:id",
        Kind::Confirm(Action::VersionDelete),
    ),
    ("POST", "/maps/:slug/versions/:id/status", Kind::Status),
    ("GET", "/maps/:slug/versions/:id/sdk/:file", Kind::Sdk),
    (
        "PUT",
        "/maps/:slug/versions/:id/file",
        Kind::Confirm(Action::FileLink),
    ),
    (
        "POST",
        "/maps/:slug/versions/:id/file",
        Kind::Confirm(Action::FileUpload),
    ),
    (
        "DELETE",
        "/maps/:slug/versions/:id/file",
        Kind::Confirm(Action::FileUnlink),
    ),
    ("GET", "/maps/:slug/players", Kind::Free),
    ("GET", "/maps/:slug/players/:uid", Kind::Free),
    (
        "PATCH",
        "/maps/:slug/players/:uid",
        Kind::Confirm(Action::PlayerEdit),
    ),
    (
        "POST",
        "/maps/:slug/players/:uid/reset",
        Kind::Confirm(Action::PlayerReset),
    ),
    ("POST", "/maps/:slug/players/:uid/block", Kind::PlayerBlock),
    ("POST", "/maps/:slug/import", Kind::Import),
    (
        "DELETE",
        "/maps/:slug/waiting/:wid",
        Kind::Confirm(Action::WaitingDelete),
    ),
];

/// The route pattern `segments` match for `method`, with its parameters.
fn matching_route<'a>(
    method: &str,
    segments: &'a [String],
) -> Option<(&'static str, Kind, Vec<(&'static str, &'a str)>)> {
    ROUTES.iter().find_map(|(route_method, pattern, kind)| {
        if *route_method != method {
            return None;
        }
        let parts: Vec<&'static str> = pattern[1..].split('/').collect();
        if parts.len() != segments.len() {
            return None;
        }
        let mut params = Vec::new();
        for (part, segment) in parts.iter().zip(segments) {
            match part.strip_prefix(':') {
                Some(name) => params.push((name, segment.as_str())),
                None if *part == segment => {}
                None => return None,
            }
        }
        Some((*pattern, *kind, params))
    })
}

/// Classify one request by method, route and (for the routes whose effect
/// depends on it) body. Every GET runs free except the SDK download; a
/// non-GET request to a route this table does not know needs a card.
pub fn classify(method: &str, path: &ApiPath, body: Option<&Value>) -> Gate {
    // `/maps/:slug/versions/:id/sdk…` in any case and with anything after
    // it; a map or version merely named `sdk` is not an SDK download.
    let segment = |index: usize| path.segments.get(index).map(String::as_str);
    if segment(0) == Some("maps")
        && segment(2) == Some("versions")
        && segment(4).is_some_and(|part| part.eq_ignore_ascii_case("sdk"))
    {
        return Gate::Sdk;
    }
    let field = |name: &str| body.and_then(|body| body.get(name));
    match matching_route(method, &path.segments).map(|(_, kind, _)| kind) {
        Some(Kind::Free) => Gate::Free,
        Some(Kind::Confirm(action)) => Gate::Confirm(action),
        Some(Kind::Sdk) => Gate::Sdk,
        Some(Kind::Status) => Gate::Confirm(match field("status").and_then(Value::as_str) {
            Some("active") => Action::Release,
            Some("blocked") => Action::Block,
            _ => Action::StatusChange,
        }),
        Some(Kind::PlayerBlock) => Gate::Confirm(match field("blocked") {
            Some(Value::Bool(false)) => Action::PlayerUnblock,
            _ => Action::PlayerBlock,
        }),
        Some(Kind::Import) if field("dryRun") == Some(&Value::Bool(true)) => Gate::Free,
        Some(Kind::Import) => Gate::Confirm(Action::DataImport {
            overwrite: field("overwrite") == Some(&Value::Bool(true)),
        }),
        None if method == "GET" => Gate::Free,
        None => Gate::Confirm(Action::Unknown),
    }
}

/// The `:slug`, `:id`, `:uid` (and `:wid`) of a request, for the card.
pub fn route_params(method: &str, path: &ApiPath) -> Vec<(&'static str, String)> {
    matching_route(method, &path.segments)
        .map(|(_, _, params)| {
            params
                .into_iter()
                .map(|(name, value)| (name, value.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `method` + `path` is the map-file upload route.
pub fn is_upload_route(method: &str, path: &ApiPath) -> bool {
    matching_route(method, &path.segments).is_some_and(|(pattern, _, _)| {
        method == "POST" && pattern == "/maps/:slug/versions/:id/file"
    })
}

/// A request body.
pub enum Payload {
    None,
    /// UTF-8 JSON text.
    Json(Vec<u8>),
    /// A file sent as it is, with its exact length.
    File {
        path: PathBuf,
        len: u64,
    },
}

/// One HTTP answer, its body read up to the caller's bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    /// The body was longer than the bound and was cut there.
    pub truncated: bool,
}

/// Why a request produced no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    Timeout,
    Unreachable,
    File,
}

/// Send one request to `url` with the key, never following a redirect (a
/// 3xx comes back as the answer), reading at most `max_body` bytes.
pub async fn send(
    url: reqwest::Url,
    key: &str,
    method: &str,
    payload: Payload,
    timeout: Duration,
    max_body: usize,
) -> Result<Reply, SendError> {
    let mut client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout);
    if is_loopback(&url) {
        // A developer's own server: never through a system proxy.
        client = client.no_proxy();
    }
    let client = client.build().map_err(|_| SendError::Unreachable)?;
    let method =
        reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| SendError::Unreachable)?;
    let authorization = Zeroizing::new(format!("Bearer {key}"));
    let mut header = reqwest::header::HeaderValue::from_str(&authorization)
        .map_err(|_| SendError::Unreachable)?;
    header.set_sensitive(true);
    let mut request = client
        .request(method, url)
        .header(reqwest::header::AUTHORIZATION, header)
        // The site's messages follow the app's language (`preferredLanguage`).
        .header(
            reqwest::header::ACCEPT_LANGUAGE,
            crate::i18n::current().code(),
        )
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/plain;q=0.9, */*;q=0.1",
        );
    request = match payload {
        Payload::None => request,
        Payload::Json(bytes) => request
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/json; charset=utf-8",
            )
            .body(bytes),
        Payload::File { path, len } => {
            let file = tokio::fs::File::open(&path)
                .await
                .map_err(|_| SendError::File)?;
            // The upload route refuses chunked bodies: state the length.
            request
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .header(reqwest::header::CONTENT_LENGTH, len)
                .body(reqwest::Body::from(file))
        }
    };
    let mut response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            SendError::Timeout
        } else {
            SendError::Unreachable
        }
    })?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let mut body = Vec::new();
    let mut truncated = false;
    loop {
        let chunk = response.chunk().await.map_err(|error| {
            if error.is_timeout() {
                SendError::Timeout
            } else {
                SendError::Unreachable
            }
        })?;
        let Some(chunk) = chunk else { break };
        let room = max_body.saturating_sub(body.len());
        if chunk.len() > room {
            body.extend_from_slice(&chunk[..room]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Reply {
        status,
        content_type,
        body,
        truncated,
    })
}

/// The lines of a generated MapVault SDK that carry key material
/// (`generateSdk` in the site's `sdk/eps.ts`): the SPECK round keys
/// `var rke<i>`/`var rkm<i>`, the key-derived self-test tables `T_SAVE`,
/// `T_CH`, `T_KEY`, and the self-test probes compared in `self_test()`
/// (`macX`/`macY`, and `a`/`b`/`c` of the CTR probe). Matched by content,
/// so a copy or a renamed file is covered, inside grep or diff lines too,
/// and a value cut off at the end of an output is masked as far as it goes.
fn sdk_secret_patterns() -> &'static [regex::Regex; 4] {
    static PATTERNS: std::sync::OnceLock<[regex::Regex; 4]> = std::sync::OnceLock::new();
    PATTERNS.get_or_init(|| {
        // A declaration starts the text, follows a non-word character, or
        // follows an escaped line break or tab (`\n`, `\r`, `\t`) of
        // JSON-escaped text; its quote may be escaped (`\"`).
        const START: &str = r"(^|[^A-Za-z0-9_]|\\[nrt])";
        // A probe line as `self_test()` emits it, or cut off at the very end
        // of an output before its condition closes (no `)`, and no later
        // line, escaped or not).
        const CUT: &str = r"[^\n\\)]*\z";
        [
            regex::Regex::new(&format!(r"{START}(var rk[em]\d{{1,3}} = )0x[0-9A-Fa-f]*")).unwrap(),
            regex::Regex::new(&format!(
                r#"{START}(const T_(?:SAVE|CH|KEY) = EUDArray\(py_eval\(\\*")\[[0-9,]*\]?"#
            ))
            .unwrap(),
            regex::Regex::new(&format!(
                r"if \(macX != 0x[0-9A-Fa-f]+ \|\| macY != 0x[0-9A-Fa-f]+\) \{{ ok = 0; \}}|if \(macX != 0x{CUT}"
            ))
            .unwrap(),
            regex::Regex::new(&format!(
                r"if \(a != 0x[0-9A-Fa-f]+ \|\| b != 0x[0-9A-Fa-f]+ \|\| c != 0x[0-9A-Fa-f]+\) \{{ ok = 0; \}}|if \(a != 0x{CUT}"
            ))
            .unwrap(),
        ]
    })
}

fn hex_values() -> &'static regex::Regex {
    static HEX: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    HEX.get_or_init(|| regex::Regex::new(r"0x[0-9A-Fa-f]*").unwrap())
}

/// `text` with the key material of any MapVault SDK in it masked, or `None`
/// when it holds none.
pub fn mask_sdk_secrets(text: &str) -> Option<String> {
    if ![
        "rke", "rkm", "T_SAVE", "T_CH", "T_KEY", "macX", "macY", "a != 0x",
    ]
    .iter()
    .any(|marker| text.contains(marker))
    {
        return None;
    }
    let [round_keys, tables, mac_probe, ctr_probe] = sdk_secret_patterns();
    let mut masked = round_keys
        .replace_all(text, "${1}${2}0x[masked]")
        .into_owned();
    masked = tables.replace_all(&masked, "${1}${2}[masked]").into_owned();
    for probe in [mac_probe, ctr_probe] {
        masked = probe
            .replace_all(&masked, |found: &regex::Captures| {
                hex_values()
                    .replace_all(&found[0], "0x[masked]")
                    .into_owned()
            })
            .into_owned();
    }
    (masked != text).then_some(masked)
}

/// [`mask_sdk_secrets`] over every string of a JSON value.
pub fn mask_sdk_secrets_value(value: &mut Value) {
    match value {
        Value::String(text) => {
            if let Some(masked) = mask_sdk_secrets(text) {
                *text = masked;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_sdk_secrets_value),
        Value::Object(map) => map.values_mut().for_each(mask_sdk_secrets_value),
        _ => {}
    }
}

/// `text` with every occurrence of `key` replaced, so no answer or error can
/// carry it back.
pub fn redact(text: &str, key: &str) -> String {
    if key.is_empty() {
        text.to_string()
    } else {
        text.replace(key, "[redacted]")
    }
}

/// [`redact`] over every string of a JSON value.
pub fn redact_value(value: &mut Value, key: &str) {
    match value {
        Value::String(text) if !key.is_empty() && text.contains(key) => *text = redact(text, key),
        Value::Array(items) => items.iter_mut().for_each(|item| redact_value(item, key)),
        Value::Object(map) => map.values_mut().for_each(|item| redact_value(item, key)),
        _ => {}
    }
}

/// The plugin text of a downloaded `msqc.edd`, accepted only when every line
/// is one the SDK generator writes (`msqcSnippet` in eps.ts): the plugin
/// headers, `QCDebug`, the ESC line and the restore-key channels. Nothing
/// else (and so no key material) can pass through.
pub fn plugin_text(edd: &str) -> Option<String> {
    let mut turbo = false;
    let mut msqc = false;
    for line in edd.lines().map(|line| line.trim_end_matches('\r')) {
        match line {
            "" => {}
            "[eudTurbo]" => turbo = true,
            "[MSQC]" => msqc = true,
            "QCDebug: false" | "NotTyping ; KeyDown(ESC) : mvCtrl, 0x1" => {}
            _ => {
                let channel = line
                    .strip_prefix("mvS")
                    .and_then(|rest| rest.split_once(" ; val, mvS"))
                    .and_then(|(k, rest)| {
                        let (k2, r) = rest.split_once(" : mvR")?;
                        (k.parse::<u32>().is_ok() && k == k2 && r == k).then_some(())
                    });
                channel?;
            }
        }
    }
    (turbo && msqc).then(|| edd.replace("\r\n", "\n").trim().to_string())
}

/// Whether `text` is the generated SDK of version `version_id`.
pub fn is_sdk_source(text: &str, version_id: u64) -> bool {
    text.starts_with("// MapVault SDK")
        && text
            .lines()
            .any(|line| line.trim() == format!("const VERSION_ID = {version_id};"))
}

/// The usage lines and the per-field accessor lines of an SDK's leading
/// comment block (`// Usage (main.eps):` and `// Player data …:`). Only
/// comment lines are read, and the block ends at the first line of code, so
/// no constant (and no key material) is ever taken.
pub fn sdk_header(text: &str) -> (Vec<String>, Vec<String>) {
    let (mut usage, mut fields) = (Vec::new(), Vec::new());
    let mut section = 0;
    for line in text.lines() {
        let Some(comment) = line.strip_prefix("//") else {
            break;
        };
        if comment.starts_with(" Usage") {
            section = 1;
            continue;
        }
        if comment.starts_with(" Player data") {
            section = 2;
            continue;
        }
        let entry = comment.trim();
        if entry.is_empty() {
            continue;
        }
        match section {
            1 => usage.push(entry.to_string()),
            2 => fields.push(entry.to_string()),
            _ => {}
        }
    }
    (usage, fields)
}

/// Credential operations behind the settings commands (tests use memory).
pub trait MapVaultKeyStore: Send + Sync {
    fn has_key(&self) -> Result<bool, String>;
    fn read(&self) -> Result<Option<Zeroizing<String>>, String>;
    fn save(&self, key: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

struct OsKeyStore;

impl MapVaultKeyStore for OsKeyStore {
    fn has_key(&self) -> Result<bool, String> {
        read_api_key().map(|key| key.is_some_and(|key| !key.is_empty()))
    }
    fn read(&self) -> Result<Option<Zeroizing<String>>, String> {
        read_api_key()
    }
    fn save(&self, key: &str) -> Result<(), String> {
        crate::provider_secrets::save_integration_secret(SECRET_INTEGRATION, SECRET_NAME, key)
    }
    fn delete(&self) -> Result<(), String> {
        crate::provider_secrets::delete_integration_secret(SECRET_INTEGRATION, SECRET_NAME)
    }
}

/// Secret-free MapVault settings view for the panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapVaultSettingsView {
    pub address: String,
    pub has_key: bool,
    /// Whose key the site says it is, after a save that checked one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<KeyOwner>,
}

/// What `GET /api/v1/me` says about a key (no secret in it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyOwner {
    pub username: String,
    pub nickname: String,
    pub key_name: String,
    /// A read-only key can only GET: no SDK, no change.
    pub read_only: bool,
}

/// Ask the site at `origin` whose `key` it is (`GET /api/v1/me`, the
/// cheapest authenticated route), following no redirect. 401 is
/// [`ERR_KEY_INVALID`], no answer [`ERR_UNREACHABLE`], anything that is not
/// the API's answer [`ERR_UNAVAILABLE`]; no error holds the key.
pub async fn verify_key(origin: &str, key: &str) -> Result<KeyOwner, String> {
    let path = parse_api_path("/me")?;
    let url = api_url(origin, &path).map_err(|_| ERR_ADDRESS_INVALID.to_string())?;
    let reply = send(url, key, "GET", Payload::None, VERIFY_TIMEOUT, 16 * 1024)
        .await
        .map_err(|_| ERR_UNREACHABLE.to_string())?;
    if reply.status == 401 {
        return Err(ERR_KEY_INVALID.to_string());
    }
    let unavailable = || ERR_UNAVAILABLE.to_string();
    let me = (reply.status == 200 && !reply.truncated)
        .then(|| serde_json::from_slice::<Value>(&reply.body).ok())
        .flatten()
        .ok_or_else(unavailable)?;
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_string);
    Ok(KeyOwner {
        username: text(me.get("username")).ok_or_else(unavailable)?,
        nickname: text(me.get("nickname")).unwrap_or_default(),
        key_name: text(me.pointer("/key/name")).unwrap_or_default(),
        read_only: me
            .pointer("/key/readOnly")
            .and_then(Value::as_bool)
            .ok_or_else(unavailable)?,
    })
}

/// A Settings save checked locally, waiting for the site's key check.
pub struct SavePlan {
    address: String,
    /// The new key to store, when one was given.
    new_key: Option<Zeroizing<String>>,
    /// The key the site must accept: the new one, or the stored one when
    /// only the address changes; `None` when nothing needs checking.
    check: Option<Zeroizing<String>>,
}

impl SavePlan {
    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn check(&self) -> Option<&str> {
        self.check.as_deref().map(String::as_str)
    }
}

pub fn settings_payload(
    dirs: &DataDirs,
    store: &dyn MapVaultKeyStore,
) -> Result<MapVaultSettingsView, String> {
    let config = dirs.load_config().map_err(|error| error.to_string())?;
    Ok(MapVaultSettingsView {
        address: config.mapvault.address,
        has_key: store.has_key().map_err(|_| ERR_KEY_STORE.to_string())?,
        owner: None,
    })
}

/// Check a Settings save before any network call: the address, the new
/// key's shape (blank means keep the stored one), and which key the site
/// must accept. Blocking I/O.
pub fn plan_save(
    dirs: &DataDirs,
    store: &dyn MapVaultKeyStore,
    address: &str,
    api_key: Option<Zeroizing<String>>,
) -> Result<SavePlan, String> {
    let address = normalize_address(address).ok_or_else(|| ERR_ADDRESS_INVALID.to_string())?;
    let new_key = api_key
        .map(|key| Zeroizing::new(key.trim().to_string()))
        .filter(|key| !key.is_empty());
    if new_key.as_ref().is_some_and(|key| !is_key_shaped(key)) {
        return Err(ERR_KEY_FORMAT.to_string());
    }
    let check = match &new_key {
        Some(key) => Some(key.clone()),
        None => {
            let config = dirs.load_config().map_err(|error| error.to_string())?;
            if config.mapvault.address == address {
                None
            } else {
                store
                    .read()
                    .map_err(|_| ERR_KEY_STORE.to_string())?
                    .filter(|key| !key.is_empty())
            }
        }
    };
    Ok(SavePlan {
        address,
        new_key,
        check,
    })
}

/// Store what a plan's check allowed: the new key and the address.
/// Blocking I/O.
pub fn persist_save(
    dirs: &DataDirs,
    store: &dyn MapVaultKeyStore,
    plan: SavePlan,
    owner: Option<KeyOwner>,
) -> Result<MapVaultSettingsView, String> {
    if let Some(key) = &plan.new_key {
        store.save(key).map_err(|_| ERR_KEY_STORE.to_string())?;
    }
    let mut config = dirs.load_config().map_err(|error| error.to_string())?;
    if config.mapvault.address != plan.address {
        config.mapvault.address = plan.address;
        dirs.save_config(&config)
            .map_err(|error| error.to_string())?;
    }
    Ok(MapVaultSettingsView {
        owner,
        ..settings_payload(dirs, store)?
    })
}

/// Save the address and, when given, a new key: the site must accept the
/// key (the new one, or the stored one at a new address) before anything
/// is stored.
pub async fn save_payload(
    dirs: &DataDirs,
    store: &dyn MapVaultKeyStore,
    address: &str,
    api_key: Option<Zeroizing<String>>,
) -> Result<MapVaultSettingsView, String> {
    let plan = plan_save(dirs, store, address, api_key)?;
    let owner = match plan.check() {
        Some(key) => Some(verify_key(plan.address(), key).await?),
        None => None,
    };
    persist_save(dirs, store, plan, owner)
}

pub fn clear_key_payload(
    dirs: &DataDirs,
    store: &dyn MapVaultKeyStore,
) -> Result<MapVaultSettingsView, String> {
    store.delete().map_err(|_| ERR_KEY_STORE.to_string())?;
    settings_payload(dirs, store)
}

/// Read the MapVault settings (`hasKey` only; the key never leaves the backend).
#[tauri::command]
pub async fn mapvault_settings(
    state: tauri::State<'_, crate::ipc::AppManaged>,
) -> Result<MapVaultSettingsView, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || settings_payload(&dirs, &OsKeyStore))
        .await
        .map_err(|error| error.to_string())?
}

/// Save the site address and optionally a new key, once the site accepted
/// the key. Credential-store and config I/O stay off the async workers.
#[tauri::command]
pub async fn mapvault_settings_save(
    state: tauri::State<'_, crate::ipc::AppManaged>,
    address: String,
    api_key: Option<String>,
) -> Result<MapVaultSettingsView, String> {
    let dirs = state.dirs().clone();
    let api_key = api_key.map(Zeroizing::new);
    let plan_dirs = dirs.clone();
    let plan = tauri::async_runtime::spawn_blocking(move || {
        plan_save(&plan_dirs, &OsKeyStore, &address, api_key)
    })
    .await
    .map_err(|error| error.to_string())??;
    let owner = match plan.check() {
        Some(key) => Some(verify_key(plan.address(), key).await?),
        None => None,
    };
    tauri::async_runtime::spawn_blocking(move || persist_save(&dirs, &OsKeyStore, plan, owner))
        .await
        .map_err(|error| error.to_string())?
}

/// Delete the stored MapVault key.
#[tauri::command]
pub async fn mapvault_key_clear(
    state: tauri::State<'_, crate::ipc::AppManaged>,
) -> Result<MapVaultSettingsView, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || clear_key_payload(&dirs, &OsKeyStore))
        .await
        .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn addresses_are_https_origins_or_loopback_http() {
        for (raw, origin) in [
            ("https://sc.mi3.dev", "https://sc.mi3.dev"),
            ("  https://SC.mi3.dev/  ", "https://sc.mi3.dev"),
            ("https://sc.mi3.dev:443", "https://sc.mi3.dev"),
            ("https://example.com:8443/", "https://example.com:8443"),
            ("http://localhost:5173", "http://localhost:5173"),
            ("http://127.0.0.1:5173/", "http://127.0.0.1:5173"),
            ("http://127.8.0.1", "http://127.8.0.1"),
            ("http://[::1]:8080", "http://[::1]:8080"),
        ] {
            assert_eq!(normalize_address(raw).as_deref(), Some(origin), "{raw}");
        }
        for hostile in [
            "",
            "sc.mi3.dev",
            "http://sc.mi3.dev",
            "http://192.168.0.10",
            "http://localhost.evil.com",
            "http://0.0.0.0",
            "ftp://sc.mi3.dev",
            "file:///C:/x",
            "javascript:alert(1)",
            "https://user:pass@sc.mi3.dev",
            "https://user@sc.mi3.dev",
            "https://sc.mi3.dev@evil.com",
            "https://sc.mi3.dev/api/v1",
            "https://sc.mi3.dev/x",
            "https://sc.mi3.dev?x=1",
            "https://sc.mi3.dev/?",
            "https://sc.mi3.dev#frag",
            "https://evil.com\\@sc.mi3.dev",
            "https://sc.mi3\n.dev",
            "https://sc .mi3.dev",
            "https://sc.mi3.dev\u{0}",
            "https://",
        ] {
            assert_eq!(normalize_address(hostile), None, "{hostile:?}");
        }
        assert_eq!(
            normalize_address(&format!("https://{}.dev", "a".repeat(600))),
            None
        );
    }

    #[test]
    fn keys_have_the_site_shape() {
        assert!(is_key_shaped(&format!("mv_{}", "a2b3c4d5".repeat(4))));
        for bad in [
            "",
            "mv_",
            "mv_ABCDEFGHABCDEFGHABCDEFGHABCDEFGH",
            "mv_abcdefghabcdefghabcdefghabcdefg",
            "mv_abcdefghabcdefghabcdefghabcdefgh1",
            "xx_abcdefghabcdefghabcdefghabcdefgh",
            "mv_abcdefghabcdefghabcdefghabcdefg8",
        ] {
            assert!(!is_key_shaped(bad), "{bad}");
        }
    }

    #[test]
    fn paths_stay_under_the_api() {
        let ok = parse_api_path("/maps/my-map/versions/12/").unwrap();
        assert_eq!(ok.path, "/maps/my-map/versions/12");
        assert_eq!(ok.segments, ["maps", "my-map", "versions", "12"]);
        assert_eq!(ok.query, None);
        let root = parse_api_path("/").unwrap();
        assert_eq!((root.path.as_str(), root.segments.len()), ("/", 0));
        let search = parse_api_path("/maps/m/players?q=홍길동&page=2").unwrap();
        assert_eq!(search.query.as_deref(), Some("q=홍길동&page=2"));
        assert_eq!(
            api_url("https://sc.mi3.dev", &search).unwrap().as_str(),
            "https://sc.mi3.dev/api/v1/maps/m/players?q=%ED%99%8D%EA%B8%B8%EB%8F%99&page=2"
        );
        assert_eq!(
            api_url("http://127.0.0.1:9", &root).unwrap().as_str(),
            "http://127.0.0.1:9/api/v1/"
        );

        for hostile in [
            "",
            "maps",
            "https://evil.com/api/v1/maps",
            "//evil.com/maps",
            "/../me",
            "/maps/../../x",
            "/maps/./x",
            "/maps//x",
            "/maps\\x",
            "/maps/%2e%2e/x",
            "/maps/a b",
            "/maps/a\u{0}",
            "/maps/a\nb",
            "/maps/a#frag",
            "/maps?q=a#b",
            "/maps?q=a b",
            "/maps?q=\u{202E}x",
            "/maps?q=\u{200B}",
            "/maps?",
            "/maps/@evil.com",
            "/maps/:x",
            "/맵",
        ] {
            assert!(parse_api_path(hostile).is_err(), "{hostile:?}");
        }
        assert!(parse_api_path(&format!("/{}", "a".repeat(3000))).is_err());
        // Whatever passes, the URL is still the configured origin's /api/v1.
        for raw in ["/maps", "/me?x=@evil.com", "/maps?next=https://evil.com/"] {
            let url = api_url("https://sc.mi3.dev", &parse_api_path(raw).unwrap()).unwrap();
            assert_eq!(url.origin().ascii_serialization(), "https://sc.mi3.dev");
            assert!(url.path().starts_with("/api/v1"), "{raw}");
        }
    }

    fn gate(method: &str, path: &str, body: Option<Value>) -> Gate {
        classify(method, &parse_api_path(path).unwrap(), body.as_ref())
    }

    #[test]
    fn every_api_route_is_classified() {
        use Action::*;
        let free = Gate::Free;
        let card = Gate::Confirm;
        let table: Vec<(&str, &str, Option<Value>, Gate)> = vec![
            ("GET", "/me", None, free),
            ("GET", "/maps", None, free),
            (
                "POST",
                "/maps",
                Some(json!({"slug": "m", "name": "M"})),
                free,
            ),
            ("GET", "/maps/m", None, free),
            (
                "PATCH",
                "/maps/m",
                Some(json!({"name": "M2"})),
                card(MapEdit),
            ),
            ("POST", "/images", None, free),
            (
                "POST",
                "/maps/m/versions",
                Some(json!({"label": "v1"})),
                free,
            ),
            ("GET", "/maps/m/versions/12", None, free),
            (
                "PATCH",
                "/maps/m/versions/12",
                Some(json!({"schema": [], "screenLang": "en"})),
                free,
            ),
            ("DELETE", "/maps/m/versions/12", None, card(VersionDelete)),
            (
                "POST",
                "/maps/m/versions/12/status",
                Some(json!({"status": "active"})),
                card(Release),
            ),
            (
                "POST",
                "/maps/m/versions/12/status",
                Some(json!({"status": "blocked"})),
                card(Block),
            ),
            (
                "POST",
                "/maps/m/versions/12/status",
                Some(json!({"status": "draft"})),
                card(StatusChange),
            ),
            (
                "POST",
                "/maps/m/versions/12/status",
                None,
                card(StatusChange),
            ),
            (
                "GET",
                "/maps/m/versions/12/sdk/mapvault.eps",
                None,
                Gate::Sdk,
            ),
            ("GET", "/maps/m/versions/12/sdk/msqc.edd", None, Gate::Sdk),
            (
                "GET",
                "/maps/m/versions/12/sdk/main.example.eps",
                None,
                Gate::Sdk,
            ),
            (
                "GET",
                "/maps/m/versions/12/SDK/mapvault.eps",
                None,
                Gate::Sdk,
            ),
            ("POST", "/maps/m/versions/12/sdk/x", None, Gate::Sdk),
            ("GET", "/maps/m/versions/12/sdk", None, Gate::Sdk),
            (
                "GET",
                "/maps/m/versions/12/sdk/mapvault.eps/",
                None,
                Gate::Sdk,
            ),
            ("GET", "/maps/sdk/versions/12/sdk/msqc.edd", None, Gate::Sdk),
            // A map whose slug is `sdk` stays usable.
            ("GET", "/maps/sdk", None, free),
            ("GET", "/maps/sdk/versions/3", None, free),
            (
                "PATCH",
                "/maps/sdk/versions/3",
                Some(json!({"screenLang": "en"})),
                free,
            ),
            ("GET", "/maps/sdk/players", None, free),
            (
                "PUT",
                "/maps/m/versions/12/file",
                Some(json!({"address": "abcd"})),
                card(FileLink),
            ),
            ("POST", "/maps/m/versions/12/file", None, card(FileUpload)),
            ("DELETE", "/maps/m/versions/12/file", None, card(FileUnlink)),
            ("GET", "/maps/m/players?q=a&page=2", None, free),
            ("GET", "/maps/m/players/7", None, free),
            (
                "PATCH",
                "/maps/m/players/7",
                Some(json!({"changes": []})),
                card(PlayerEdit),
            ),
            ("POST", "/maps/m/players/7/reset", None, card(PlayerReset)),
            (
                "POST",
                "/maps/m/players/7/block",
                Some(json!({"blocked": true})),
                card(PlayerBlock),
            ),
            (
                "POST",
                "/maps/m/players/7/block",
                Some(json!({"blocked": false})),
                card(PlayerUnblock),
            ),
            ("POST", "/maps/m/players/7/block", None, card(PlayerBlock)),
            (
                "POST",
                "/maps/m/import",
                Some(json!({"text": "nickname,level", "dryRun": true})),
                free,
            ),
            (
                "POST",
                "/maps/m/import",
                Some(json!({"text": "nickname,level"})),
                card(DataImport { overwrite: false }),
            ),
            (
                "POST",
                "/maps/m/import",
                Some(json!({"text": "x", "overwrite": true, "dryRun": false})),
                card(DataImport { overwrite: true }),
            ),
            ("DELETE", "/maps/m/waiting/3", None, card(WaitingDelete)),
            // The reference and unknown reads run; unknown changes ask.
            ("GET", "/", None, free),
            ("GET", "/maps/m/unknown", None, free),
            ("DELETE", "/maps/m", None, card(Unknown)),
            ("PUT", "/maps", None, card(Unknown)),
            ("POST", "/maps/m/everything", None, card(Unknown)),
            ("PATCH", "/Maps/m/players/7", None, card(Unknown)),
            ("POST", "/", None, card(Unknown)),
        ];
        for (method, path, body, expected) in table {
            assert_eq!(gate(method, path, body), expected, "{method} {path}");
        }
        // Every table route is reachable, so none is shadowed.
        for (method, pattern, _) in ROUTES {
            let concrete = pattern
                .split('/')
                .map(|part| if part.starts_with(':') { "x1" } else { part })
                .collect::<Vec<_>>()
                .join("/");
            let path = parse_api_path(&concrete).unwrap();
            assert_eq!(
                matching_route(method, &path.segments).map(|(found, _, _)| found),
                Some(*pattern)
            );
        }
        assert_eq!(ROUTES.len(), 22, "api.ts has 22 routes");
        assert!(Release.irreversible() && FileUpload.irreversible() && PlayerReset.irreversible());
        assert!(VersionDelete.irreversible() && DataImport { overwrite: true }.irreversible());
        assert!(MapEdit.irreversible(), "dropped images are deleted at once");
        assert!(!Block.irreversible() && !PlayerEdit.irreversible() && !FileLink.irreversible());
        let upload = parse_api_path("/maps/m/versions/12/file").unwrap();
        assert!(is_upload_route("POST", &upload));
        assert!(!is_upload_route("PUT", &upload));
        assert_eq!(
            route_params("POST", &parse_api_path("/maps/m/players/7/reset").unwrap()),
            [("slug", "m".to_string()), ("uid", "7".to_string())]
        );
    }

    #[test]
    fn only_generated_plugin_lines_pass() {
        let edd = "[eudTurbo]\n\n[MSQC]\nQCDebug: false\nNotTyping ; KeyDown(ESC) : mvCtrl, 0x1\nmvS1 ; val, mvS1 : mvR1\nmvS2 ; val, mvS2 : mvR2\n";
        assert_eq!(
            plugin_text(edd).unwrap(),
            edd.trim(),
            "the generated snippet passes as it is"
        );
        assert!(plugin_text(&edd.replace("\n", "\r\n")).is_some());
        for bad in [
            "[MSQC]\nQCDebug: false\n",
            "[eudTurbo]\n",
            &format!("{edd}const KEY = 0x1234;\n"),
            &edd.replace("mvS2 ; val, mvS2 : mvR2", "mvS2 ; val, mvS3 : mvR2"),
            &edd.replace("QCDebug: false", "QCDebug: true"),
        ] {
            assert!(plugin_text(bad).is_none(), "{bad}");
        }
        assert!(is_sdk_source(
            "// MapVault SDK — x\nconst VERSION_ID = 12;\n",
            12
        ));
        assert!(!is_sdk_source(
            "// MapVault SDK — x\nconst VERSION_ID = 12;\n",
            1
        ));
        assert!(!is_sdk_source("{\"error\":\"x\"}", 12));
    }

    #[test]
    fn the_header_gives_usage_and_accessors_and_never_code() {
        let sdk = concat!(
            "// MapVault SDK — generated for \"M\" version v1 (versionId 12).\n",
            "// DO NOT EDIT. Regenerate from the MapVault site instead. Contains this version's secret key.\n",
            "//\n",
            "// Usage (main.eps):\n",
            "//   import mapvault;\n",
            "//   function onPluginStart()     { mapvault.init(); }\n",
            "//   mapvault.origin(p)         // p's map file: 0 not loaded yet\n",
            "// Player data (index p = player 0..7):\n",
            "//   mapvault.d_level[p]  레벨 (0..100, saved as 100 if larger)\n",
            "//   mapvault.d_items[p * 6 + i]  Items (8 bits, 6 entries)\n",
            "\n",
            "const VERSION_ID = 12;\n",
            "const K0 = 0x12345678;\n",
            "// a later comment\n",
        );
        let (usage, fields) = sdk_header(sdk);
        assert_eq!(
            usage,
            [
                "import mapvault;",
                "function onPluginStart()     { mapvault.init(); }",
                "mapvault.origin(p)         // p's map file: 0 not loaded yet",
            ]
        );
        assert_eq!(
            fields,
            [
                "mapvault.d_level[p]  레벨 (0..100, saved as 100 if larger)",
                "mapvault.d_items[p * 6 + i]  Items (8 bits, 6 entries)",
            ]
        );
        let all = format!("{usage:?}{fields:?}");
        assert!(!all.contains("0x12345678") && !all.contains("later"));
    }

    #[test]
    fn sdk_key_lines_are_masked_wherever_they_appear_and_nothing_else_is() {
        let sdk = concat!(
            "var rke0 = 0x1A2B3C4D;\n",
            "var rkm26 = 0x0BADF00D;\n",
            "const T_SAVE = EUDArray(py_eval(\"[1,2,3]\"));\n",
            "const T_CH = EUDArray(py_eval(\"[4,5]\"));\n",
            "const T_KEY = EUDArray(py_eval(\"[6,7,8]\"));\n",
            "    if (macX != 0xAA000001 || macY != 0xAA000002) { ok = 0; }\n",
            "    if (a != 0xBB000001 || b != 0xBB000002 || c != 0xBB000003) { ok = 0; }\n",
            "const T_NICK = EUDArray(py_eval(\"[9]\"));\n",
            "    x ^= rke3;\n",
        );
        let masked = mask_sdk_secrets(sdk).unwrap();
        for secret in [
            "1A2B3C4D", "0BADF00D", "1,2,3", "4,5", "6,7,8", "AA000001", "AA000002", "BB000001",
            "BB000002", "BB000003",
        ] {
            assert!(!masked.contains(secret), "{secret}: {masked}");
        }
        assert!(masked.contains("var rke0 = 0x[masked];"), "{masked}");
        assert!(
            masked.contains("const T_KEY = EUDArray(py_eval(\"[masked]\"));"),
            "{masked}"
        );
        assert!(masked.contains("c != 0x[masked]) { ok = 0; }"), "{masked}");
        // The rest stays readable.
        assert!(
            masked.contains("[9]") && masked.contains("x ^= rke3;"),
            "{masked}"
        );
        // Inside grep or diff lines, and cut off at the end of an output.
        let grep = "src/copy.eps:412: var rke5 = 0x77;\n+var rkm1 = 0x12";
        let masked = mask_sdk_secrets(grep).unwrap();
        assert!(
            !masked.contains("0x77") && !masked.contains("0x12"),
            "{masked}"
        );
        // A probe cut off at the end of an output is masked as far as it goes.
        let cut = mask_sdk_secrets("x;\n    if (a != 0xBB000001 || b != 0xBB00").unwrap();
        assert!(
            !cut.contains("BB000001") && !cut.contains("0xBB00"),
            "{cut}"
        );
        // Ordinary code is left alone.
        for plain in [
            "if (a != 0x10) { b = 1; }",
            "if (a != 0x10 || b != 0x20) { ok = 0; }\nnext();",
            "if (a != 0x10 || b != 0x20) {",
            "if (macX != 0x10) { show(); }",
            "const T_LIMIT = 3;",
            "var rk = 0x5;",
            "avar rke0 = 0x5;",
        ] {
            assert_eq!(mask_sdk_secrets(plain), None, "{plain}");
        }
        let mut value = json!({"content": sdk, "lines": [sdk]});
        mask_sdk_secrets_value(&mut value);
        assert!(!value.to_string().contains("1A2B3C4D"));
    }

    #[test]
    fn json_escaped_sdk_text_is_masked_too() {
        let sdk = concat!(
            "var rke0 = 0x1A2B3C4D;\n",
            "var rkm1 = 0x0BADF00D;\r\n",
            "\tconst T_KEY = EUDArray(py_eval(\"[6,7,8]\"));\n",
            "    if (macX != 0xAA000001 || macY != 0xAA000002) { ok = 0; }\n",
            "    if (a != 0xBB000001 || b != 0xBB000002 || c != 0xBB000003) { ok = 0; }\n",
            "end\n",
        );
        // Text that holds the SDK as an escaped JSON string (`\n`, `\r\n`,
        // `\t`, `\"`), as a tool error or a nested answer would.
        let escaped = serde_json::to_string(&json!({"content": sdk})).unwrap();
        assert!(escaped.contains("\\nvar rkm1") && escaped.contains("\\\"[6,7,8]\\\""));
        let masked = mask_sdk_secrets(&escaped).unwrap();
        for secret in [
            "1A2B3C4D", "0BADF00D", "6,7,8", "AA000001", "AA000002", "BB000001", "BB000002",
            "BB000003",
        ] {
            assert!(!masked.contains(secret), "{secret}: {masked}");
        }
        assert!(
            masked.contains("\\nvar rkm1 = 0x[masked];\\r\\n"),
            "{masked}"
        );
        assert!(masked.contains("\\\"[masked]\\\""), "{masked}");
        assert!(masked.ends_with("end\\n\"}"), "{masked}");
    }

    #[test]
    fn the_upload_budget_follows_the_file_size() {
        assert_eq!(upload_send_time(1), Duration::from_secs(21));
        assert_eq!(upload_send_time(10 * 1024 * 1024), Duration::from_secs(30));
        assert_eq!(
            practical_upload_limit(Duration::from_secs(230), Duration::from_secs(30)),
            180 * (UPLOAD_PLAN_RATE * HASH_PLAN_RATE / (UPLOAD_PLAN_RATE + HASH_PLAN_RATE))
        );
        assert_eq!(
            practical_upload_limit(Duration::from_secs(230), Duration::from_secs(30)) / 1_000_000,
            185
        );
        assert_eq!(
            practical_upload_limit(Duration::from_secs(10), Duration::from_secs(30)),
            0
        );
        assert_eq!(
            practical_upload_limit(Duration::from_secs(10_000), Duration::ZERO),
            MAX_MAP_UPLOAD
        );
    }

    #[test]
    fn redaction_removes_every_copy_of_the_key() {
        let key = "mv_abcdefghabcdefghabcdefghabcdefgh";
        let mut value = json!({"a": format!("x {key} y {key}"), "b": [key], "c": 1});
        redact_value(&mut value, key);
        assert!(!value.to_string().contains(key));
        assert_eq!(value["a"], "x [redacted] y [redacted]");
    }

    #[derive(Default)]
    struct MemoryStore(parking_lot::Mutex<Option<String>>);

    impl MapVaultKeyStore for MemoryStore {
        fn has_key(&self) -> Result<bool, String> {
            Ok(self.0.lock().is_some())
        }
        fn read(&self) -> Result<Option<Zeroizing<String>>, String> {
            Ok(self.0.lock().clone().map(Zeroizing::new))
        }
        fn save(&self, key: &str) -> Result<(), String> {
            *self.0.lock() = Some(key.to_string());
            Ok(())
        }
        fn delete(&self) -> Result<(), String> {
            *self.0.lock() = None;
            Ok(())
        }
    }

    const GOOD: &str = "mv_abcdefghabcdefghabcdefghabcdefgh";
    const READ_ONLY: &str = "mv_rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr";

    /// A loopback stand-in of `GET /api/v1/me`: `GOOD` is a full key,
    /// `READ_ONLY` a read-only one, anything else 401. Returns its origin
    /// and the number of requests it saw.
    async fn me_site(
        answer: fn(&str) -> (u16, Vec<(&'static str, String)>, String),
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = std::sync::Arc::clone(&hits);
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let seen = std::sync::Arc::clone(&seen);
            async move {
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert_eq!(request.uri().path(), "/api/v1/me");
                let auth = request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                let (status, headers, body) = answer(&auth);
                let mut response = axum::response::Response::new(axum::body::Body::from(body));
                *response.status_mut() = axum::http::StatusCode::from_u16(status).unwrap();
                for (name, value) in headers {
                    response
                        .headers_mut()
                        .insert(name, axum::http::HeaderValue::from_str(&value).unwrap());
                }
                response
            }
        });
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        (origin, hits)
    }

    fn me_answer(auth: &str) -> (u16, Vec<(&'static str, String)>, String) {
        let json = vec![("content-type", "application/json".to_string())];
        let me = |read_only: bool| {
            json!({"username": "maker1", "nickname": "제작자", "key": {"name": "agent", "readOnly": read_only}})
                .to_string()
        };
        if auth == format!("Bearer {GOOD}") {
            (200, json, me(false))
        } else if auth == format!("Bearer {READ_ONLY}") {
            (200, json, me(true))
        } else {
            (
                401,
                json,
                json!({"error": "The API key is missing or not valid."}).to_string(),
            )
        }
    }

    fn settings_dirs() -> (std::path::PathBuf, DataDirs) {
        let base = std::env::temp_dir().join(format!(
            "eud-mapvault-settings-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        (base, dirs)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_key_is_stored_only_after_the_site_accepted_it() {
        let (origin, hits) = me_site(me_answer).await;
        let (base, dirs) = settings_dirs();
        let store = MemoryStore::default();
        assert_eq!(
            settings_payload(&dirs, &store).unwrap(),
            MapVaultSettingsView {
                address: DEFAULT_ADDRESS.to_string(),
                has_key: false,
                owner: None
            }
        );

        // Valid: stored, and the panel learns whose key it is.
        let saved = save_payload(
            &dirs,
            &store,
            &origin,
            Some(Zeroizing::new(format!("  {GOOD} "))),
        )
        .await
        .unwrap();
        assert_eq!(saved.address, origin);
        assert!(saved.has_key);
        assert_eq!(
            saved.owner,
            Some(KeyOwner {
                username: "maker1".into(),
                nickname: "제작자".into(),
                key_name: "agent".into(),
                read_only: false
            })
        );
        assert_eq!(store.0.lock().as_deref(), Some(GOOD));
        let config = std::fs::read_to_string(dirs.config_path()).unwrap();
        assert!(!config.contains(GOOD) && config.contains(&origin));
        assert!(!serde_json::to_string(&saved).unwrap().contains(GOOD));

        // Invalid (401): nothing changes, and the error is a code without the key.
        let wrong = "mv_zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        let error = save_payload(&dirs, &store, &origin, Some(Zeroizing::new(wrong.into())))
            .await
            .unwrap_err();
        assert_eq!(error, ERR_KEY_INVALID);
        assert_eq!(store.0.lock().as_deref(), Some(GOOD));

        // Read-only: stored, and said so.
        let saved = save_payload(
            &dirs,
            &store,
            &origin,
            Some(Zeroizing::new(READ_ONLY.into())),
        )
        .await
        .unwrap();
        assert!(saved.owner.unwrap().read_only);
        assert_eq!(store.0.lock().as_deref(), Some(READ_ONLY));

        // Nothing to check (same address, no new key): no request at all.
        let before = hits.load(std::sync::atomic::Ordering::SeqCst);
        let kept = save_payload(&dirs, &store, &origin, Some(Zeroizing::new(" ".into())))
            .await
            .unwrap();
        assert!(kept.has_key && kept.owner.is_none());
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), before);

        // A refused address or key shape is refused before any request.
        assert_eq!(
            save_payload(&dirs, &store, "http://example.com", None)
                .await
                .unwrap_err(),
            ERR_ADDRESS_INVALID
        );
        assert_eq!(
            save_payload(
                &dirs,
                &store,
                &origin,
                Some(Zeroizing::new("not-a-key".into()))
            )
            .await
            .unwrap_err(),
            ERR_KEY_FORMAT
        );
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), before);
        assert!(!clear_key_payload(&dirs, &store).unwrap().has_key);
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_new_address_is_kept_only_when_it_accepts_the_stored_key() {
        let (origin, _) = me_site(me_answer).await;
        let (base, dirs) = settings_dirs();
        let store = MemoryStore::default();
        save_payload(&dirs, &store, &origin, Some(Zeroizing::new(GOOD.into())))
            .await
            .unwrap();

        // Unreachable (a closed port, as a typo in the address would be).
        let closed = {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        let error = save_payload(&dirs, &store, &closed, None)
            .await
            .unwrap_err();
        assert_eq!(error, ERR_UNREACHABLE);
        assert_eq!(settings_payload(&dirs, &store).unwrap().address, origin);

        // A redirect is not followed and is not the API's answer.
        let (redirect, _) = me_site(|_| {
            (
                302,
                vec![("location", "https://example.com/api/v1/me".to_string())],
                String::new(),
            )
        })
        .await;
        let error = save_payload(&dirs, &store, &redirect, None)
            .await
            .unwrap_err();
        assert_eq!(error, ERR_UNAVAILABLE);
        // So is a page that is not the maker API.
        let (page, _) = me_site(|_| {
            (
                200,
                vec![("content-type", "text/html".to_string())],
                "<html>hello</html>".to_string(),
            )
        })
        .await;
        assert_eq!(
            save_payload(&dirs, &store, &page, None).await.unwrap_err(),
            ERR_UNAVAILABLE
        );
        assert_eq!(settings_payload(&dirs, &store).unwrap().address, origin);
        assert_eq!(store.0.lock().as_deref(), Some(GOOD));

        // A new address that accepts the stored key is kept.
        let (other, _) = me_site(me_answer).await;
        let moved = save_payload(&dirs, &store, &other, None).await.unwrap();
        assert_eq!(moved.address, other);
        assert_eq!(moved.owner.unwrap().username, "maker1");
        std::fs::remove_dir_all(base).ok();
    }
}
