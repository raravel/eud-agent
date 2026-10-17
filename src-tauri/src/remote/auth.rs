//! Device pairing and request authentication for the remote server.
//!
//! A pairing code (8 characters, 5 minutes, single use, at most one live, five
//! wrong guesses invalidate it) is exchanged for a device token: 32 random
//! bytes, base64url. Only the token's SHA-256 is persisted. Requests carry the
//! token in the `eud_remote` cookie or an `Authorization: Bearer` header.
//! A token unused for [`IDLE_EXPIRY_MS`] stops working; the device stays
//! listed as expired until it is removed or paired again.
//!
//! Through `tailscale serve` (a `*.ts.net` Host) a device is also bound to the
//! tailnet login that paired it: serve sets `Tailscale-User-Login` for tailnet
//! users and strips any client-supplied value
//! (<https://tailscale.com/kb/1312/serve>, "Identity headers"), so a token
//! copied to another tailnet user's device is refused.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::{header, HeaderMap, Method};
use base64::Engine as _;
use parking_lot::Mutex;
use serde::Serialize;

use super::settings::RemoteDevice;
use crate::config::DataDirs;

pub const COOKIE_NAME: &str = "eud_remote";
/// Unambiguous uppercase alphabet: no `I`, `O`, `0`, `1`. 32 symbols, so a
/// random byte modulo 32 is unbiased.
const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 8;
pub const CODE_TTL_MS: u64 = 5 * 60 * 1000;
pub const MAX_CODE_ATTEMPTS: u32 = 5;
/// `lastSeenAt` is kept current in memory but written to config at most this often.
pub const LAST_SEEN_PERSIST_MS: u64 = 60 * 1000;
/// A device token unused this long stops working.
pub const IDLE_EXPIRY_MS: u64 = 30 * 24 * 60 * 60 * 1000;
const TOKEN_BYTES: usize = 32;
/// `tailscale serve` names the tailnet user of a proxied request here.
pub const TAILSCALE_LOGIN_HEADER: &str = "tailscale-user-login";
/// `tailscale serve` marks a request that came in through Funnel here.
pub const TAILSCALE_FUNNEL_HEADER: &str = "tailscale-funnel-request";
const MAX_DEVICE_NAME_CHARS: usize = 64;
/// Browsers cap cookie lifetime at 400 days.
const COOKIE_MAX_AGE_SECS: u64 = 400 * 24 * 60 * 60;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

/// A freshly issued pairing code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingCode {
    pub code: String,
    pub expires_at: u64,
}

/// A paired device as the panel sees it (never its token hash).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub last_seen_at: Option<u64>,
    /// The tailnet login that paired it, when paired through Tailscale.
    pub owner_login: Option<String>,
    /// Unused for [`IDLE_EXPIRY_MS`]: its token no longer works.
    pub expired: bool,
}

impl DeviceView {
    fn of(device: &RemoteDevice, now: u64) -> Self {
        Self {
            id: device.id.clone(),
            name: device.name.clone(),
            created_at: device.created_at,
            last_seen_at: device.last_seen_at,
            owner_login: device.owner_login.clone(),
            expired: is_idle_expired(device, now),
        }
    }
}

/// Whether `device` went unused for [`IDLE_EXPIRY_MS`] by `now`.
fn is_idle_expired(device: &RemoteDevice, now: u64) -> bool {
    let used = device.last_seen_at.unwrap_or(device.created_at);
    now.saturating_sub(used) >= IDLE_EXPIRY_MS
}

/// Why a token was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRefusal {
    /// No device has this token (never paired, or removed).
    Unknown,
    /// The device went unused for [`IDLE_EXPIRY_MS`].
    Expired,
}

pub const UNPAIRED_MESSAGE: &str =
    "연결된 기기가 아닙니다. PC의 설정 → 원격 접속에서 기기를 연결해 주세요.";
pub const EXPIRED_MESSAGE: &str = "30일 동안 사용하지 않아 기기 연결이 만료되었습니다. PC의 설정 → 원격 접속에서 새 코드를 만들어 다시 연결해 주세요.";

impl AuthRefusal {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unknown => UNPAIRED_MESSAGE,
            Self::Expired => EXPIRED_MESSAGE,
        }
    }
}

/// What happened to pairing, for the desktop to show and sockets to obey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthEvent {
    /// A device paired.
    Paired(DeviceView),
    /// Repeated wrong guesses invalidated the live code.
    CodeLocked,
    /// A device was removed; its sockets close.
    Revoked(String),
}

/// The result of a successful pairing: the token leaves the process exactly once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedDevice {
    pub device: DeviceView,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    /// No live code (never issued, consumed, or invalidated).
    NoCode,
    Expired,
    Mismatch,
    /// The fifth wrong guess invalidated the code.
    TooManyAttempts,
    /// Paired through `tailscale serve` without a tailnet login (a tagged
    /// device, or Funnel).
    MissingIdentity,
    Persist(String),
}

impl PairError {
    /// Korean text with a recovery action; never says which part was wrong.
    pub fn message(&self) -> String {
        match self {
            Self::MissingIdentity => {
                "Tailscale 로그인 정보를 확인할 수 없습니다. 태그가 없는 Tailscale 계정으로 로그인한 기기에서 tailscale serve 주소로 다시 연결해 주세요."
                    .to_string()
            }
            Self::NoCode | Self::Expired | Self::Mismatch => {
                "연결 코드가 올바르지 않거나 만료되었습니다. PC의 설정 → 원격 접속에서 새 코드를 만들어 주세요."
                    .to_string()
            }
            Self::TooManyAttempts => {
                "잘못된 코드를 여러 번 입력해 코드가 폐기되었습니다. PC에서 새 코드를 만들어 주세요."
                    .to_string()
            }
            Self::Persist(error) => {
                format!("기기를 저장하지 못했습니다: {error}. PC에서 다시 시도해 주세요.")
            }
        }
    }
}

struct LiveCode {
    code: String,
    expires_at: u64,
    failures: u32,
}

struct AuthState {
    code: Option<LiveCode>,
    devices: Vec<RemoteDevice>,
    /// Device id -> when its `lastSeenAt` was last written to config.
    seen_persisted_at: HashMap<String, u64>,
}

/// Pairing codes and the paired-device list. Devices are cached in memory and
/// written through to `Config.remote.devices`.
pub struct RemoteAuth {
    dirs: DataDirs,
    state: Mutex<AuthState>,
    events: tokio::sync::broadcast::Sender<AuthEvent>,
}

impl RemoteAuth {
    /// Load paired devices from config. An unreadable config starts empty.
    pub fn load(dirs: DataDirs) -> Self {
        let devices = match dirs.load_config() {
            Ok(config) => config.remote.devices,
            Err(error) => {
                eprintln!("eud-agent: remote devices unavailable: {error}");
                Vec::new()
            }
        };
        Self {
            dirs,
            state: Mutex::new(AuthState {
                code: None,
                devices,
                seen_persisted_at: HashMap::new(),
            }),
            events: tokio::sync::broadcast::channel(64).0,
        }
    }

    /// Pairings, code locks and revocations as they happen.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AuthEvent> {
        self.events.subscribe()
    }

    fn publish(&self, event: AuthEvent) {
        // Nobody listening is fine.
        let _ = self.events.send(event);
    }

    pub fn devices(&self) -> Vec<DeviceView> {
        let now = now_ms();
        self.state
            .lock()
            .devices
            .iter()
            .map(|device| DeviceView::of(device, now))
            .collect()
    }

    /// Whether device `id` is paired and its token still works.
    pub fn is_active(&self, id: &str) -> bool {
        let now = now_ms();
        self.state
            .lock()
            .devices
            .iter()
            .any(|device| device.id == id && !is_idle_expired(device, now))
    }

    pub fn begin_pairing(&self) -> Result<PairingCode, String> {
        self.begin_pairing_at(now_ms())
    }

    /// Issue a new code, replacing any live one.
    pub fn begin_pairing_at(&self, now: u64) -> Result<PairingCode, String> {
        let mut bytes = [0u8; CODE_LEN];
        fill_random(&mut bytes)?;
        let code: String = bytes
            .iter()
            .map(|byte| CODE_ALPHABET[usize::from(*byte) % CODE_ALPHABET.len()] as char)
            .collect();
        let expires_at = now.saturating_add(CODE_TTL_MS);
        self.state.lock().code = Some(LiveCode {
            code: code.clone(),
            expires_at,
            failures: 0,
        });
        Ok(PairingCode { code, expires_at })
    }

    pub fn pair(&self, code: &str, device_name: &str) -> Result<IssuedDevice, PairError> {
        self.pair_at(now_ms(), code, device_name)
    }

    /// Pair a device whose request named tailnet login `owner`.
    pub fn pair_as(
        &self,
        code: &str,
        device_name: &str,
        owner: Option<String>,
    ) -> Result<IssuedDevice, PairError> {
        self.pair_owned_at(now_ms(), code, device_name, owner)
    }

    pub fn pair_at(
        &self,
        now: u64,
        code: &str,
        device_name: &str,
    ) -> Result<IssuedDevice, PairError> {
        self.pair_owned_at(now, code, device_name, None)
    }

    fn pair_owned_at(
        &self,
        now: u64,
        code: &str,
        device_name: &str,
        owner: Option<String>,
    ) -> Result<IssuedDevice, PairError> {
        let result = self.try_pair(now, code, device_name, owner);
        match &result {
            Ok(issued) => self.publish(AuthEvent::Paired(issued.device.clone())),
            Err(PairError::TooManyAttempts) => self.publish(AuthEvent::CodeLocked),
            Err(_) => {}
        }
        result
    }

    fn try_pair(
        &self,
        now: u64,
        code: &str,
        device_name: &str,
        owner: Option<String>,
    ) -> Result<IssuedDevice, PairError> {
        let mut state = self.state.lock();
        let Some(live) = state.code.as_mut() else {
            return Err(PairError::NoCode);
        };
        if now >= live.expires_at {
            state.code = None;
            return Err(PairError::Expired);
        }
        let given = normalize_code(code);
        if !constant_time_eq(given.as_bytes(), live.code.as_bytes()) {
            live.failures += 1;
            if live.failures >= MAX_CODE_ATTEMPTS {
                state.code = None;
                return Err(PairError::TooManyAttempts);
            }
            return Err(PairError::Mismatch);
        }
        state.code = None;

        let mut token_bytes = [0u8; TOKEN_BYTES];
        fill_random(&mut token_bytes).map_err(PairError::Persist)?;
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes);
        let device = RemoteDevice {
            id: uuid::Uuid::new_v4().simple().to_string(),
            name: clean_device_name(device_name),
            token_sha256: token_hash(&token),
            created_at: now,
            last_seen_at: Some(now),
            owner_login: owner,
        };
        let mut devices = state.devices.clone();
        devices.push(device.clone());
        self.persist(&devices).map_err(PairError::Persist)?;
        state.devices = devices;
        state.seen_persisted_at.insert(device.id.clone(), now);
        Ok(IssuedDevice {
            device: DeviceView::of(&device, now),
            token,
        })
    }

    pub fn authenticate(&self, token: &str) -> Option<DeviceView> {
        self.authenticate_at(now_ms(), token)
    }

    pub fn authenticate_at(&self, now: u64, token: &str) -> Option<DeviceView> {
        self.check_at(now, token).ok()
    }

    pub fn check(&self, token: &str) -> Result<DeviceView, AuthRefusal> {
        self.check_at(now_ms(), token)
    }

    /// Resolve a token to its device and record the visit; a device unused
    /// for [`IDLE_EXPIRY_MS`] is refused and its visit not recorded. Every
    /// device hash is compared in constant time.
    pub fn check_at(&self, now: u64, token: &str) -> Result<DeviceView, AuthRefusal> {
        let hash = token_hash(token);
        let mut state = self.state.lock();
        let mut found = None;
        for (index, device) in state.devices.iter().enumerate() {
            if constant_time_eq(hash.as_bytes(), device.token_sha256.as_bytes()) {
                found = Some(index);
            }
        }
        let index = found.ok_or(AuthRefusal::Unknown)?;
        if is_idle_expired(&state.devices[index], now) {
            return Err(AuthRefusal::Expired);
        }
        state.devices[index].last_seen_at = Some(now);
        let device = state.devices[index].clone();
        let persisted_at = state.seen_persisted_at.get(&device.id).copied();
        if !persisted_at.is_some_and(|at| now.saturating_sub(at) < LAST_SEEN_PERSIST_MS) {
            match self.persist(&state.devices) {
                Ok(()) => {
                    state.seen_persisted_at.insert(device.id.clone(), now);
                }
                Err(error) => eprintln!("eud-agent: remote last-seen save failed: {error}"),
            }
        }
        Ok(DeviceView::of(&device, now))
    }

    /// Remove one device; its token stops working at once. `false` when unknown.
    pub fn revoke(&self, id: &str) -> Result<bool, String> {
        let mut state = self.state.lock();
        if !state.devices.iter().any(|device| device.id == id) {
            return Ok(false);
        }
        let devices: Vec<RemoteDevice> = state
            .devices
            .iter()
            .filter(|device| device.id != id)
            .cloned()
            .collect();
        self.persist(&devices)?;
        state.devices = devices;
        state.seen_persisted_at.remove(id);
        drop(state);
        self.publish(AuthEvent::Revoked(id.to_string()));
        Ok(true)
    }

    fn persist(&self, devices: &[RemoteDevice]) -> Result<(), String> {
        let _config_guard = crate::config::PROJECT_CONFIG_LOCK.lock();
        let mut config = self.dirs.load_config().map_err(|error| error.to_string())?;
        config.remote.devices = devices.to_vec();
        self.dirs
            .save_config(&config)
            .map_err(|error| error.to_string())
    }
}

fn fill_random(buffer: &mut [u8]) -> Result<(), String> {
    getrandom::fill(buffer).map_err(|error| format!("OS 난수 생성기를 사용할 수 없습니다: {error}"))
}

/// Lowercase hex SHA-256 of a device token.
pub fn token_hash(token: &str) -> String {
    crate::bootstrap::sha256_hex_bytes(token.as_bytes())
}

pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let difference = left
        .iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    std::hint::black_box(difference) == 0
}

/// Accept `abcd-efgh`, `ABCD EFGH`, and the bare code.
fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-')
        .map(|ch| ch.to_ascii_uppercase())
        .collect()
}

fn clean_device_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_DEVICE_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "이름 없는 기기".to_string()
    } else {
        cleaned.to_string()
    }
}

/// The token from the `eud_remote` cookie, else from `Authorization: Bearer`.
pub fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    let from_cookie = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty());
    from_cookie.or_else(|| {
        headers
            .get(header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
    })
}

pub fn session_cookie(token: &str) -> String {
    format!(
        "{COOKIE_NAME}={token}; HttpOnly; Secure; SameSite=Strict; Path=/remote; Max-Age={COOKIE_MAX_AGE_SECS}"
    )
}

pub fn cleared_cookie() -> String {
    format!("{COOKIE_NAME}=; HttpOnly; Secure; SameSite=Strict; Path=/remote; Max-Age=0")
}

/// Only loopback names and Tailscale MagicDNS names may address the server,
/// so a DNS-rebinding page cannot reach it under its own host name.
pub fn host_allowed(headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let name = host_name(host).to_ascii_lowercase();
    matches!(name.as_str(), "127.0.0.1" | "localhost" | "[::1]")
        || (name.ends_with(".ts.net") && name.len() > ".ts.net".len())
}

/// Whether the request's Host is a loopback name (the PC itself, not a
/// request `tailscale serve` proxied in).
pub fn is_loopback_host(headers: &HeaderMap) -> bool {
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| {
            matches!(
                host_name(host).to_ascii_lowercase().as_str(),
                "127.0.0.1" | "localhost" | "[::1]"
            )
        })
}

/// The tailnet login `tailscale serve` named for this request.
pub fn tailnet_login(headers: &HeaderMap) -> Option<String> {
    headers
        .get(TAILSCALE_LOGIN_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|login| !login.is_empty())
        .map(str::to_string)
}

/// Whether `device` may be used by this request: on the PC itself (a
/// loopback Host) the token alone suffices; through `tailscale serve` the
/// request must name the tailnet login that paired the device.
pub fn identity_allowed(headers: &HeaderMap, device: &DeviceView) -> bool {
    if is_loopback_host(headers) {
        return true;
    }
    match (tailnet_login(headers), device.owner_login.as_deref()) {
        (Some(login), Some(owner)) => constant_time_eq(login.as_bytes(), owner.as_bytes()),
        _ => false,
    }
}

/// Whether `tailscale serve` received this request through Funnel (from the
/// public internet). Serve sets this header itself and strips a client's.
pub fn is_funnel_request(headers: &HeaderMap) -> bool {
    headers.contains_key(TAILSCALE_FUNNEL_HEADER)
}

fn host_name(host: &str) -> &str {
    if host.starts_with('[') {
        return host.split_inclusive(']').next().unwrap_or(host);
    }
    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

/// Whether a request's origin is acceptable. Safe methods pass; a
/// state-changing request or a WebSocket upgrade must carry no `Origin`, or an
/// `http(s)` origin naming the request's own host (`Host`, or the
/// `X-Forwarded-Host` a local reverse proxy such as `tailscale serve` sets).
pub fn origin_allowed(method: &Method, headers: &HeaderMap) -> bool {
    let upgrade = headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    let safe = matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    if safe && !upgrade {
        return true;
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Some(authority) = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
    else {
        return false;
    };
    if authority.is_empty() || authority.contains('/') {
        return false;
    }
    [
        header::HOST,
        header::HeaderName::from_static("x-forwarded-host"),
    ]
    .iter()
    .filter_map(|name| headers.get(name)?.to_str().ok())
    .any(|host| host.eq_ignore_ascii_case(authority))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn temp_dirs(label: &str) -> DataDirs {
        let base = std::env::temp_dir().join(format!(
            "eud-remote-{label}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        DataDirs::from_bases(&base.join("roaming"), &base.join("local"))
    }

    const T0: u64 = 1_700_000_000_000;

    #[test]
    fn code_uses_the_unambiguous_alphabet_and_expires_in_five_minutes() {
        let auth = RemoteAuth::load(temp_dirs("code"));
        let code = auth.begin_pairing_at(T0).unwrap();
        assert_eq!(code.code.len(), CODE_LEN);
        assert!(code.code.bytes().all(|byte| CODE_ALPHABET.contains(&byte)));
        assert_eq!(code.expires_at, T0 + CODE_TTL_MS);
    }

    #[test]
    fn expired_code_is_refused_and_discarded() {
        let auth = RemoteAuth::load(temp_dirs("expiry"));
        let code = auth.begin_pairing_at(T0).unwrap();
        assert_eq!(
            auth.pair_at(T0 + CODE_TTL_MS, &code.code, "폰"),
            Err(PairError::Expired)
        );
        assert_eq!(
            auth.pair_at(T0 + 1, &code.code, "폰"),
            Err(PairError::NoCode)
        );
    }

    #[test]
    fn code_is_single_use() {
        let auth = RemoteAuth::load(temp_dirs("single"));
        let code = auth.begin_pairing_at(T0).unwrap();
        let issued = auth
            .pair_at(T0 + 1, &code.code.to_lowercase(), " 내 폰 ")
            .unwrap();
        assert_eq!(issued.device.name, "내 폰");
        assert_eq!(
            auth.pair_at(T0 + 2, &code.code, "다른 폰"),
            Err(PairError::NoCode)
        );
        assert_eq!(auth.devices().len(), 1);
    }

    #[test]
    fn new_code_replaces_the_live_one() {
        let auth = RemoteAuth::load(temp_dirs("replace"));
        let first = auth.begin_pairing_at(T0).unwrap();
        let second = auth.begin_pairing_at(T0).unwrap();
        if first.code != second.code {
            assert_eq!(
                auth.pair_at(T0 + 1, &first.code, "폰"),
                Err(PairError::Mismatch)
            );
        }
        assert!(auth.pair_at(T0 + 1, &second.code, "폰").is_ok());
    }

    #[test]
    fn fifth_wrong_guess_invalidates_the_code() {
        let auth = RemoteAuth::load(temp_dirs("attempts"));
        let code = auth.begin_pairing_at(T0).unwrap();
        let wrong = if code.code == "AAAAAAAA" {
            "BBBBBBBB"
        } else {
            "AAAAAAAA"
        };
        for _ in 1..MAX_CODE_ATTEMPTS {
            assert_eq!(auth.pair_at(T0 + 1, wrong, "폰"), Err(PairError::Mismatch));
        }
        assert_eq!(
            auth.pair_at(T0 + 1, wrong, "폰"),
            Err(PairError::TooManyAttempts)
        );
        assert_eq!(
            auth.pair_at(T0 + 1, &code.code, "폰"),
            Err(PairError::NoCode)
        );
    }

    #[test]
    fn only_the_token_hash_is_persisted_and_it_authenticates() {
        let dirs = temp_dirs("hash");
        let auth = RemoteAuth::load(dirs.clone());
        let code = auth.begin_pairing_at(T0).unwrap();
        let issued = auth.pair_at(T0 + 1, &code.code, "폰").unwrap();
        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&issued.token)
                .unwrap()
                .len(),
            TOKEN_BYTES
        );

        let saved = std::fs::read_to_string(dirs.config_path()).unwrap();
        assert!(!saved.contains(&issued.token));
        let config = dirs.load_config().unwrap();
        assert_eq!(config.remote.devices.len(), 1);
        assert_eq!(
            config.remote.devices[0].token_sha256,
            token_hash(&issued.token)
        );
        assert_eq!(config.remote.devices[0].token_sha256.len(), 64);

        // A fresh process reads the same device back.
        let reloaded = RemoteAuth::load(dirs);
        let device = reloaded.authenticate_at(T0 + 2, &issued.token).unwrap();
        assert_eq!(device.id, issued.device.id);
        assert!(reloaded.authenticate_at(T0 + 2, "not-a-token").is_none());
        assert!(reloaded.authenticate_at(T0 + 2, "").is_none());
    }

    #[test]
    fn last_seen_is_written_at_most_once_a_minute() {
        let dirs = temp_dirs("seen");
        let auth = RemoteAuth::load(dirs.clone());
        let code = auth.begin_pairing_at(T0).unwrap();
        let issued = auth.pair_at(T0, &code.code, "폰").unwrap();
        let persisted =
            |dirs: &DataDirs| dirs.load_config().unwrap().remote.devices[0].last_seen_at;

        let seen = auth.authenticate_at(T0 + 10_000, &issued.token).unwrap();
        assert_eq!(seen.last_seen_at, Some(T0 + 10_000));
        assert_eq!(persisted(&dirs), Some(T0));

        auth.authenticate_at(T0 + LAST_SEEN_PERSIST_MS, &issued.token)
            .unwrap();
        assert_eq!(persisted(&dirs), Some(T0 + LAST_SEEN_PERSIST_MS));
    }

    #[test]
    fn revoke_removes_the_device_and_its_token() {
        let dirs = temp_dirs("revoke");
        let auth = RemoteAuth::load(dirs.clone());
        let code = auth.begin_pairing_at(T0).unwrap();
        let issued = auth.pair_at(T0, &code.code, "폰").unwrap();
        assert!(auth.revoke(&issued.device.id).unwrap());
        assert!(!auth.revoke(&issued.device.id).unwrap());
        assert!(auth.authenticate_at(T0 + 1, &issued.token).is_none());
        assert!(dirs.load_config().unwrap().remote.devices.is_empty());
    }

    #[test]
    fn pairing_keeps_unrelated_config_fields() {
        let dirs = temp_dirs("keep");
        dirs.save_config(&crate::config::Config {
            deep_planning: true,
            remote: super::super::RemoteSettings {
                enabled: true,
                port: 50001,
                devices: Vec::new(),
            },
            ..Default::default()
        })
        .unwrap();
        let auth = RemoteAuth::load(dirs.clone());
        let code = auth.begin_pairing_at(T0).unwrap();
        auth.pair_at(T0, &code.code, "폰").unwrap();
        let config = dirs.load_config().unwrap();
        assert!(config.deep_planning);
        assert!(config.remote.enabled);
        assert_eq!(config.remote.port, 50001);
        assert_eq!(config.remote.devices.len(), 1);
    }

    #[test]
    fn token_comes_from_cookie_or_bearer() {
        let mut headers = HeaderMap::new();
        assert_eq!(token_from_headers(&headers), None);
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer bearer-token"),
        );
        assert_eq!(
            token_from_headers(&headers).as_deref(),
            Some("bearer-token")
        );
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=1; eud_remote=cookie-token"),
        );
        assert_eq!(
            token_from_headers(&headers).as_deref(),
            Some("cookie-token")
        );
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn origin_must_match_the_host_for_state_changes() {
        let host = "pc.tail1234.ts.net";
        let post = Method::POST;
        assert!(origin_allowed(&post, &headers(&[("host", host)])));
        assert!(origin_allowed(
            &post,
            &headers(&[("host", host), ("origin", "https://pc.tail1234.ts.net")])
        ));
        assert!(origin_allowed(
            &post,
            &headers(&[
                ("host", "127.0.0.1:47821"),
                ("origin", "http://127.0.0.1:47821")
            ])
        ));
        assert!(origin_allowed(
            &post,
            &headers(&[
                ("host", "127.0.0.1:47821"),
                ("x-forwarded-host", host),
                ("origin", "https://pc.tail1234.ts.net")
            ])
        ));
        assert!(!origin_allowed(
            &post,
            &headers(&[("host", host), ("origin", "https://evil.example")])
        ));
        assert!(!origin_allowed(
            &post,
            &headers(&[("host", host), ("origin", "null")])
        ));
        assert!(!origin_allowed(
            &post,
            &headers(&[("host", host), ("origin", "ftp://pc.tail1234.ts.net")])
        ));
        // Safe reads pass; a cross-origin WebSocket upgrade does not.
        assert!(origin_allowed(
            &Method::GET,
            &headers(&[("host", host), ("origin", "https://evil.example")])
        ));
        assert!(!origin_allowed(
            &Method::GET,
            &headers(&[
                ("host", host),
                ("origin", "https://evil.example"),
                ("upgrade", "websocket")
            ])
        ));
    }

    #[test]
    fn only_loopback_and_tailnet_hosts_are_served() {
        assert!(host_allowed(&headers(&[("host", "127.0.0.1:47821")])));
        assert!(host_allowed(&headers(&[("host", "localhost:47821")])));
        assert!(host_allowed(&headers(&[("host", "[::1]:47821")])));
        assert!(host_allowed(&headers(&[("host", "pc.tail1234.ts.net")])));
        assert!(!host_allowed(&headers(&[("host", "evil.example:47821")])));
        assert!(!host_allowed(&headers(&[("host", ".ts.net")])));
        assert!(!host_allowed(&HeaderMap::new()));
    }

    #[test]
    fn a_token_unused_for_thirty_days_expires() {
        let auth = RemoteAuth::load(temp_dirs("idle"));
        let code = auth.begin_pairing_at(T0).unwrap();
        let issued = auth.pair_at(T0, &code.code, "폰").unwrap();
        // Use keeps it alive.
        let later = T0 + IDLE_EXPIRY_MS - 1;
        assert!(auth.check_at(later, &issued.token).is_ok());
        assert!(auth
            .check_at(later + IDLE_EXPIRY_MS - 1, &issued.token)
            .is_ok());
        // Thirty idle days: refused as expired, not unknown, and the refused
        // visit does not revive it.
        let idle = later + 2 * IDLE_EXPIRY_MS;
        assert_eq!(
            auth.check_at(idle, &issued.token),
            Err(AuthRefusal::Expired)
        );
        assert_eq!(
            auth.check_at(idle + 1, &issued.token),
            Err(AuthRefusal::Expired)
        );
        assert_eq!(
            auth.check_at(idle, "not-a-token"),
            Err(AuthRefusal::Unknown)
        );
        assert!(AuthRefusal::Expired.message().contains("만료"));
        // Settings still lists it, marked expired.
        let device = &auth.devices()[0];
        assert!(device.expired);
        assert!(!auth.is_active(&device.id));
    }

    #[test]
    fn pairing_and_revocation_are_announced() {
        let auth = RemoteAuth::load(temp_dirs("events"));
        let mut events = auth.subscribe();
        let code = auth.begin_pairing_at(T0).unwrap();
        let issued = auth
            .pair_owned_at(T0, &code.code, "폰", Some("me@example.com".to_string()))
            .unwrap();
        assert_eq!(issued.device.owner_login.as_deref(), Some("me@example.com"));
        assert_eq!(
            events.try_recv().unwrap(),
            AuthEvent::Paired(issued.device.clone())
        );
        assert!(auth.revoke(&issued.device.id).unwrap());
        assert_eq!(
            events.try_recv().unwrap(),
            AuthEvent::Revoked(issued.device.id.clone())
        );
        let code = auth.begin_pairing_at(T0).unwrap();
        let wrong = if code.code == "AAAAAAAA" {
            "BBBBBBBB"
        } else {
            "AAAAAAAA"
        };
        for _ in 0..MAX_CODE_ATTEMPTS {
            let _ = auth.pair_at(T0 + 1, wrong, "폰");
        }
        assert_eq!(events.try_recv().unwrap(), AuthEvent::CodeLocked);
    }

    #[test]
    fn a_tailnet_request_must_come_from_the_pairing_login() {
        let device = |owner: Option<&str>| DeviceView {
            id: "d".to_string(),
            name: "폰".to_string(),
            created_at: 0,
            last_seen_at: None,
            owner_login: owner.map(str::to_string),
            expired: false,
        };
        let tailnet = headers(&[("host", "pc.tail1234.ts.net")]);
        let mine = headers(&[
            ("host", "pc.tail1234.ts.net"),
            ("tailscale-user-login", "me@example.com"),
        ]);
        let theirs = headers(&[
            ("host", "pc.tail1234.ts.net"),
            ("tailscale-user-login", "other@example.com"),
        ]);
        let local = headers(&[("host", "127.0.0.1:47821")]);
        assert!(identity_allowed(&mine, &device(Some("me@example.com"))));
        assert!(!identity_allowed(&theirs, &device(Some("me@example.com"))));
        assert!(!identity_allowed(&tailnet, &device(Some("me@example.com"))));
        // A device paired on the PC has no tailnet owner to match.
        assert!(!identity_allowed(&mine, &device(None)));
        // On the PC itself the token alone is enough.
        assert!(identity_allowed(&local, &device(None)));
        assert!(is_loopback_host(&local));
        assert!(!is_loopback_host(&tailnet));
        assert_eq!(tailnet_login(&mine).as_deref(), Some("me@example.com"));
        assert!(is_funnel_request(&headers(&[(
            "tailscale-funnel-request",
            "?1"
        )])));
        assert!(!is_funnel_request(&mine));
    }

    #[test]
    fn constant_time_eq_compares_whole_values() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
