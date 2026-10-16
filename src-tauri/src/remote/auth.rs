//! Device pairing and request authentication for the remote server.
//!
//! A pairing code (8 characters, 5 minutes, single use, at most one live, five
//! wrong guesses invalidate it) is exchanged for a device token: 32 random
//! bytes, base64url. Only the token's SHA-256 is persisted. Requests carry the
//! token in the `eud_remote` cookie or an `Authorization: Bearer` header.

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
const TOKEN_BYTES: usize = 32;
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
}

impl From<&RemoteDevice> for DeviceView {
    fn from(device: &RemoteDevice) -> Self {
        Self {
            id: device.id.clone(),
            name: device.name.clone(),
            created_at: device.created_at,
            last_seen_at: device.last_seen_at,
        }
    }
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
    Persist(String),
}

impl PairError {
    /// Korean text with a recovery action; never says which part was wrong.
    pub fn message(&self) -> String {
        match self {
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
        }
    }

    pub fn devices(&self) -> Vec<DeviceView> {
        self.state
            .lock()
            .devices
            .iter()
            .map(DeviceView::from)
            .collect()
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

    pub fn pair_at(
        &self,
        now: u64,
        code: &str,
        device_name: &str,
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
        };
        let mut devices = state.devices.clone();
        devices.push(device.clone());
        self.persist(&devices).map_err(PairError::Persist)?;
        state.devices = devices;
        state.seen_persisted_at.insert(device.id.clone(), now);
        Ok(IssuedDevice {
            device: DeviceView::from(&device),
            token,
        })
    }

    pub fn authenticate(&self, token: &str) -> Option<DeviceView> {
        self.authenticate_at(now_ms(), token)
    }

    /// Resolve a token to its device and record the visit. Every device hash
    /// is compared in constant time.
    pub fn authenticate_at(&self, now: u64, token: &str) -> Option<DeviceView> {
        let hash = token_hash(token);
        let mut state = self.state.lock();
        let mut found = None;
        for (index, device) in state.devices.iter().enumerate() {
            if constant_time_eq(hash.as_bytes(), device.token_sha256.as_bytes()) {
                found = Some(index);
            }
        }
        let index = found?;
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
        Some(DeviceView::from(&device))
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
    fn constant_time_eq_compares_whole_values() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
