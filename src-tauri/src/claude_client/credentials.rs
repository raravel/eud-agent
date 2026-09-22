//! App-profile Claude subscription credential: read, expiry check, and OAuth refresh.
//!
//! The Claude CLI stores its OAuth pair in `.credentials.json` and refreshes it only while a
//! turn runs, so an idle profile ages into an expired access token between turns. The catalog
//! therefore refreshes the pair itself with the same public client id, token endpoint, and
//! `.oauth_refresh.lock` directory protocol the CLI uses, and writes the rotated pair back so
//! the CLI's next turn keeps working. Tokens are never logged or persisted anywhere else.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use zeroize::Zeroizing;

const CREDENTIALS_FILENAME: &str = ".credentials.json";
const OAUTH_OBJECT_KEY: &str = "claudeAiOauth";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const REFRESH_LOCK_FILENAME: &str = ".oauth_refresh.lock";
/// The CLI's `proper-lockfile` `stale` setting: an older lock directory belongs to a dead process.
const REFRESH_LOCK_STALE: Duration = Duration::from_secs(60);
const REFRESH_LOCK_WAIT: Duration = Duration::from_secs(10);
const REFRESH_LOCK_POLL: Duration = Duration::from_millis(250);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);
/// Refresh ahead of expiry so a paginated catalog fetch never straddles the boundary.
const REFRESH_SKEW: Duration = Duration::from_secs(5 * 60);
const MAX_CREDENTIAL_BYTES: u64 = 1024 * 1024;
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;

/// Reads the stored access token as-is, without refresh.
#[cfg(test)]
fn read_access_token(profile_dir: &Path) -> Result<Zeroizing<String>, String> {
    let document = read_document(profile_dir)?;
    access_token_valid_at(oauth_object(&document)?, now_ms())
}

/// The access token for one catalog request, refreshed through the CLI's OAuth client when
/// the stored one is expired or about to expire. The caller holds the profile lock so a
/// concurrent logout cannot be resurrected by the write-back.
pub(crate) async fn access_token(
    client: &reqwest::Client,
    profile_dir: &Path,
) -> Result<Zeroizing<String>, String> {
    access_token_at(client, TOKEN_URL, profile_dir).await
}

pub(crate) async fn access_token_at(
    client: &reqwest::Client,
    token_url: &str,
    profile_dir: &Path,
) -> Result<Zeroizing<String>, String> {
    let deadline = now_ms().saturating_add(REFRESH_SKEW.as_millis() as u64);
    let document = read_document(profile_dir)?;
    match access_token_valid_at(oauth_object(&document)?, deadline) {
        Ok(token) => return Ok(token),
        Err(code) if code != "provider_auth_expired" => return Err(code),
        Err(_) => {}
    }
    drop(document);

    // Same acquisition order as the CLI: the shared refresh lock, then the legacy per-file lock.
    let _refresh_lock = RefreshLock::acquire(profile_dir.join(REFRESH_LOCK_FILENAME)).await?;
    let _legacy_lock =
        RefreshLock::acquire(profile_dir.join(format!("{CREDENTIALS_FILENAME}.lock"))).await?;

    // A sibling process may have refreshed while this one waited for the lock.
    let document = read_document(profile_dir)?;
    let oauth = oauth_object(&document)?;
    if let Ok(token) = access_token_valid_at(oauth, deadline) {
        return Ok(token);
    }
    let refresh_token =
        token_field(oauth, "refreshToken").ok_or_else(|| "provider_auth_expired".to_string())?;
    if oauth
        .get("refreshTokenExpiresAt")
        .and_then(Value::as_u64)
        .is_some_and(|expires_at| expires_at <= now_ms())
    {
        return Err("provider_auth_expired".to_string());
    }
    let scopes = oauth
        .get("scopes")
        .and_then(Value::as_array)
        .map(|scopes| {
            scopes
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    drop(document);

    let refreshed = request_refresh(client, token_url, &refresh_token, &scopes).await?;

    // Write back only over the exact pair that was refreshed: a credential removed by logout or
    // rotated by a lock-ignoring process is adopted as-is rather than overwritten.
    let mut document = read_document(profile_dir)?;
    let oauth = document
        .get_mut(OAUTH_OBJECT_KEY)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "provider_not_authenticated".to_string())?;
    if token_field(oauth, "refreshToken").map(|stored| stored.as_str() == refresh_token.as_str())
        != Some(true)
    {
        return Ok(refreshed.access_token);
    }
    let issued_ms = now_ms();
    oauth.insert(
        "accessToken".to_string(),
        Value::String(refreshed.access_token.to_string()),
    );
    oauth.insert(
        "expiresAt".to_string(),
        Value::from(issued_ms.saturating_add(refreshed.expires_in_ms)),
    );
    if let Some(rotated) = refreshed.refresh_token.as_deref() {
        oauth.insert(
            "refreshToken".to_string(),
            Value::String(rotated.to_string()),
        );
    }
    if let Some(refresh_expires_in_ms) = refreshed.refresh_expires_in_ms {
        oauth.insert(
            "refreshTokenExpiresAt".to_string(),
            Value::from(issued_ms.saturating_add(refresh_expires_in_ms)),
        );
    }
    if !refreshed.scopes.is_empty() {
        oauth.insert(
            "scopes".to_string(),
            Value::Array(
                refreshed
                    .scopes
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }
    let bytes = Zeroizing::new(
        serde_json::to_vec(&document).map_err(|_| "provider_protocol_changed".to_string())?,
    );
    drop(document);
    let path = credential_path(profile_dir);
    crate::memory::write_atomic_bytes(&path, &bytes)
        .map_err(|_| "provider_catalog_unavailable".to_string())?;
    crate::provider_secrets::harden_private_path(&path)?;
    Ok(refreshed.access_token)
}

struct RefreshedCredential {
    access_token: Zeroizing<String>,
    refresh_token: Option<Zeroizing<String>>,
    expires_in_ms: u64,
    refresh_expires_in_ms: Option<u64>,
    scopes: Vec<String>,
}

async fn request_refresh(
    client: &reqwest::Client,
    token_url: &str,
    refresh_token: &str,
    scopes: &[String],
) -> Result<RefreshedCredential, String> {
    let mut body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": OAUTH_CLIENT_ID,
    });
    if !scopes.is_empty() {
        body["scope"] = Value::String(scopes.join(" "));
    }
    let response = client
        .post(token_url)
        .timeout(REFRESH_TIMEOUT)
        .json(&body)
        .send()
        .await
        .map_err(|_| "provider_transport_closed".to_string())?;
    drop(body);
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_TOKEN_RESPONSE_BYTES as u64)
    {
        return Err("provider_protocol_changed".to_string());
    }
    let bytes = Zeroizing::new(
        response
            .bytes()
            .await
            .map_err(|_| "provider_transport_closed".to_string())?
            .to_vec(),
    );
    if bytes.len() > MAX_TOKEN_RESPONSE_BYTES {
        return Err("provider_protocol_changed".to_string());
    }
    if !status.is_success() {
        return Err(refresh_status_error(status, &bytes));
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "provider_protocol_changed".to_string())?;
    let access_token = value
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|token| is_token_shaped(token))
        .map(|token| Zeroizing::new(token.to_string()))
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    let expires_in_ms = value
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|seconds| *seconds > 0)
        .and_then(|seconds| seconds.checked_mul(1000))
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    let refresh_token = value
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|token| is_token_shaped(token))
        .map(|token| Zeroizing::new(token.to_string()));
    let refresh_expires_in_ms = value
        .get("refresh_token_expires_in")
        .and_then(Value::as_u64)
        .and_then(|seconds| seconds.checked_mul(1000));
    let scopes = value
        .get("scope")
        .and_then(Value::as_str)
        .map(|scope| scope.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    Ok(RefreshedCredential {
        access_token,
        refresh_token,
        expires_in_ms,
        refresh_expires_in_ms,
        scopes,
    })
}

fn refresh_status_error(status: reqwest::StatusCode, body: &[u8]) -> String {
    let invalid_grant = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|error| error == "invalid_grant");
    match status.as_u16() {
        400 if invalid_grant => "provider_not_authenticated",
        401 => "provider_not_authenticated",
        429 => "provider_rate_limited",
        500..=599 => "provider_transport_closed",
        _ => "provider_protocol_changed",
    }
    .to_string()
}

/// A held `proper-lockfile`-style lock directory, removed on drop.
struct RefreshLock {
    path: PathBuf,
}

impl RefreshLock {
    async fn acquire(path: PathBuf) -> Result<Self, String> {
        let started = std::time::Instant::now();
        loop {
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err("provider_catalog_unavailable".to_string()),
            }
            let stale = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                .is_some_and(|age| age > REFRESH_LOCK_STALE);
            if stale {
                let _ = std::fs::remove_dir(&path);
                continue;
            }
            if started.elapsed() >= REFRESH_LOCK_WAIT {
                return Err("provider_catalog_unavailable".to_string());
            }
            tokio::time::sleep(REFRESH_LOCK_POLL).await;
        }
    }
}

impl Drop for RefreshLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.path);
    }
}

fn credential_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join(CREDENTIALS_FILENAME)
}

fn read_document(profile_dir: &Path) -> Result<Value, String> {
    let path = credential_path(profile_dir);
    let metadata =
        std::fs::metadata(&path).map_err(|_| "provider_not_authenticated".to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_CREDENTIAL_BYTES {
        return Err("provider_not_authenticated".to_string());
    }
    let bytes =
        Zeroizing::new(std::fs::read(&path).map_err(|_| "provider_not_authenticated".to_string())?);
    serde_json::from_slice(&bytes).map_err(|_| "provider_not_authenticated".to_string())
}

fn oauth_object(document: &Value) -> Result<&Map<String, Value>, String> {
    document
        .get(OAUTH_OBJECT_KEY)
        .and_then(Value::as_object)
        .ok_or_else(|| "provider_not_authenticated".to_string())
}

fn access_token_valid_at(
    oauth: &Map<String, Value>,
    at_ms: u64,
) -> Result<Zeroizing<String>, String> {
    if let Some(expires_at) = oauth.get("expiresAt").and_then(Value::as_u64) {
        if expires_at <= at_ms {
            return Err("provider_auth_expired".to_string());
        }
    }
    token_field(oauth, "accessToken").ok_or_else(|| "provider_not_authenticated".to_string())
}

fn token_field(oauth: &Map<String, Value>, key: &str) -> Option<Zeroizing<String>> {
    oauth
        .get(key)
        .and_then(Value::as_str)
        .filter(|token| is_token_shaped(token))
        .map(|token| Zeroizing::new(token.to_string()))
}

fn is_token_shaped(token: &str) -> bool {
    !token.is_empty() && !token.chars().any(char::is_control)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    const FAR_FUTURE_MS: u64 = 32_503_680_000_000;

    fn profile() -> PathBuf {
        let base = std::env::temp_dir().join(format!("eud-claude-token-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn write_credential(profile: &Path, oauth: Value) {
        std::fs::write(
            credential_path(profile),
            serde_json::to_vec(&json!({ OAUTH_OBJECT_KEY: oauth })).unwrap(),
        )
        .unwrap();
    }

    fn expired_credential() -> Value {
        json!({
            "accessToken": "stale-access",
            "refreshToken": "refresh-1",
            "expiresAt": 1,
            "refreshTokenExpiresAt": FAR_FUTURE_MS,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max",
            "rateLimitTier": "default_claude_max_5x"
        })
    }

    fn read_oauth(profile: &Path) -> Map<String, Value> {
        let document: Value =
            serde_json::from_slice(&std::fs::read(credential_path(profile)).unwrap()).unwrap();
        document[OAUTH_OBJECT_KEY].as_object().unwrap().clone()
    }

    /// One-shot HTTP server that answers every request with `status` and `body`, returning the
    /// raw requests it saw.
    fn token_server(
        status: &'static str,
        body: String,
        expected_requests: usize,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let mut seen = Vec::new();
            for _ in 0..expected_requests {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = vec![0_u8; 16384];
                let read = socket.read(&mut buffer).await.unwrap();
                seen.push(String::from_utf8_lossy(&buffer[..read]).to_string());
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            seen
        });
        (format!("http://{address}/v1/oauth/token"), handle)
    }

    #[test]
    fn access_token_requires_unexpired_oauth_credential() {
        let base = profile();
        assert_eq!(
            read_access_token(&base),
            Err("provider_not_authenticated".to_string())
        );
        write_credential(&base, json!({"accessToken": "live", "expiresAt": 1}));
        assert_eq!(
            read_access_token(&base),
            Err("provider_auth_expired".to_string())
        );
        write_credential(
            &base,
            json!({"accessToken": "live", "expiresAt": FAR_FUTURE_MS}),
        );
        assert_eq!(read_access_token(&base).unwrap().as_str(), "live");
        std::fs::write(credential_path(&base), br#"{"primaryApiKey":"sk"}"#).unwrap();
        assert_eq!(
            read_access_token(&base),
            Err("provider_not_authenticated".to_string())
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn unexpired_token_is_returned_without_a_refresh_request() {
        let base = profile();
        write_credential(
            &base,
            json!({"accessToken": "live", "refreshToken": "refresh-1", "expiresAt": FAR_FUTURE_MS}),
        );
        let client = reqwest::Client::builder().build().unwrap();
        let token = access_token_at(&client, "http://127.0.0.1:9/v1/oauth/token", &base)
            .await
            .unwrap();
        assert_eq!(token.as_str(), "live");
        assert_eq!(read_oauth(&base)["accessToken"], "live");
        assert!(!base.join(REFRESH_LOCK_FILENAME).exists());
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn expired_token_is_refreshed_and_rotated_pair_written_back() {
        let base = profile();
        write_credential(&base, expired_credential());
        let (url, server) = token_server(
            "200 OK",
            json!({
                "access_token": "fresh-access",
                "refresh_token": "refresh-2",
                "expires_in": 28800,
                "refresh_token_expires_in": 2592000,
                "scope": "user:inference user:profile user:sessions:claude_code",
                "token_type": "Bearer"
            })
            .to_string(),
            1,
        );
        let client = reqwest::Client::builder().build().unwrap();
        let before_ms = now_ms();
        let token = access_token_at(&client, &url, &base).await.unwrap();
        assert_eq!(token.as_str(), "fresh-access");

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(request.starts_with("POST /v1/oauth/token "));
        assert!(request
            .to_ascii_lowercase()
            .contains("content-type: application/json"));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["grant_type"], "refresh_token");
        assert_eq!(body["refresh_token"], "refresh-1");
        assert_eq!(body["client_id"], OAUTH_CLIENT_ID);
        assert_eq!(body["scope"], "user:inference user:profile");

        let oauth = read_oauth(&base);
        assert_eq!(oauth["accessToken"], "fresh-access");
        assert_eq!(oauth["refreshToken"], "refresh-2");
        let expires_at = oauth["expiresAt"].as_u64().unwrap();
        assert!(expires_at >= before_ms + 28_800_000);
        assert!(oauth["refreshTokenExpiresAt"].as_u64().unwrap() >= before_ms + 2_592_000_000);
        assert_eq!(
            oauth["scopes"],
            json!([
                "user:inference",
                "user:profile",
                "user:sessions:claude_code"
            ])
        );
        assert_eq!(oauth["subscriptionType"], "max");
        assert_eq!(oauth["rateLimitTier"], "default_claude_max_5x");
        assert!(!base.join(REFRESH_LOCK_FILENAME).exists());
        assert!(!base.join(format!("{CREDENTIALS_FILENAME}.lock")).exists());
        assert_eq!(read_access_token(&base).unwrap().as_str(), "fresh-access");
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn refresh_without_rotation_keeps_the_stored_refresh_token() {
        let base = profile();
        write_credential(&base, expired_credential());
        let (url, server) = token_server(
            "200 OK",
            json!({"access_token": "fresh-access", "expires_in": 3600}).to_string(),
            1,
        );
        let client = reqwest::Client::builder().build().unwrap();
        assert_eq!(
            access_token_at(&client, &url, &base)
                .await
                .unwrap()
                .as_str(),
            "fresh-access"
        );
        server.await.unwrap();
        let oauth = read_oauth(&base);
        assert_eq!(oauth["refreshToken"], "refresh-1");
        assert_eq!(oauth["refreshTokenExpiresAt"], FAR_FUTURE_MS);
        assert_eq!(oauth["scopes"], json!(["user:inference", "user:profile"]));
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn rejected_refresh_leaves_the_stored_credential_untouched() {
        let base = profile();
        write_credential(&base, expired_credential());
        let (url, server) = token_server(
            "400 Bad Request",
            json!({"error": "invalid_grant", "error_description": "revoked"}).to_string(),
            1,
        );
        let client = reqwest::Client::builder().build().unwrap();
        assert_eq!(
            access_token_at(&client, &url, &base).await,
            Err("provider_not_authenticated".to_string())
        );
        server.await.unwrap();
        let oauth = read_oauth(&base);
        assert_eq!(oauth["accessToken"], "stale-access");
        assert_eq!(oauth["refreshToken"], "refresh-1");
        assert!(!base.join(REFRESH_LOCK_FILENAME).exists());
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn expired_refresh_token_does_not_call_the_token_endpoint() {
        let base = profile();
        let mut oauth = expired_credential();
        oauth["refreshTokenExpiresAt"] = Value::from(1);
        write_credential(&base, oauth);
        let client = reqwest::Client::builder().build().unwrap();
        assert_eq!(
            access_token_at(&client, "http://127.0.0.1:9/v1/oauth/token", &base).await,
            Err("provider_auth_expired".to_string())
        );
        assert_eq!(read_oauth(&base)["accessToken"], "stale-access");
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn sibling_refresh_under_the_lock_is_adopted_without_a_request() {
        let base = profile();
        write_credential(&base, expired_credential());
        let lock = base.join(REFRESH_LOCK_FILENAME);
        std::fs::create_dir(&lock).unwrap();
        let sibling_profile = base.clone();
        let sibling = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            write_credential(
                &sibling_profile,
                json!({"accessToken": "sibling-access", "refreshToken": "refresh-9", "expiresAt": FAR_FUTURE_MS}),
            );
            std::fs::remove_dir(sibling_profile.join(REFRESH_LOCK_FILENAME)).unwrap();
        });
        let client = reqwest::Client::builder().build().unwrap();
        let token = access_token_at(&client, "http://127.0.0.1:9/v1/oauth/token", &base)
            .await
            .unwrap();
        sibling.await.unwrap();
        assert_eq!(token.as_str(), "sibling-access");
        assert_eq!(read_oauth(&base)["refreshToken"], "refresh-9");
        assert!(!lock.exists());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn refresh_status_errors_map_to_provider_codes() {
        use reqwest::StatusCode;
        assert_eq!(
            refresh_status_error(StatusCode::BAD_REQUEST, br#"{"error":"invalid_grant"}"#),
            "provider_not_authenticated"
        );
        assert_eq!(
            refresh_status_error(StatusCode::BAD_REQUEST, br#"{"error":"invalid_request"}"#),
            "provider_protocol_changed"
        );
        assert_eq!(
            refresh_status_error(StatusCode::UNAUTHORIZED, b""),
            "provider_not_authenticated"
        );
        assert_eq!(
            refresh_status_error(StatusCode::TOO_MANY_REQUESTS, b""),
            "provider_rate_limited"
        );
        assert_eq!(
            refresh_status_error(StatusCode::BAD_GATEWAY, b""),
            "provider_transport_closed"
        );
    }
}
