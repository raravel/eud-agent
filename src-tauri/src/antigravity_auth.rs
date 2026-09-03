//! Google OAuth and Cloud Code Assist onboarding.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use zeroize::Zeroizing;

use crate::provider::{ProviderAvailability, ProviderId, ProviderStatusCode};
use crate::provider_secrets::ProviderSecretStore;

pub const CLOUD_CODE_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com";
pub const ANTIGRAVITY_USER_AGENT: &str =
    "antigravity/hub/2.10.0 (aidev_client; os_type=darwin; arch=arm64; cl=963137146)";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const CALLBACK_PATH: &str = "/oauth-callback";
const CALLBACK_PORT: u16 = 51_121;
const OAUTH_TIMEOUT: Duration = Duration::from_secs(300);
const ONBOARD_TIMEOUT: Duration = Duration::from_secs(30);
const ONBOARD_POLL: Duration = Duration::from_secs(1);
const SECRET_NAME: &str = "oauth";
const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/cclog",
    "https://www.googleapis.com/auth/experimentsandconfigs",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AntigravityCredential {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub granted_scopes: Vec<String>,
    pub project_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityStatus {
    pub availability: ProviderAvailability,
    pub detail_code: Option<ProviderStatusCode>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: String,
    token_type: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallbackOutcome {
    Completed,
    Cancelled,
    Failed,
}

enum CallbackPayload {
    Code(String),
    Cancelled,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tier {
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IneligibleTier {
    #[serde(default)]
    tier_id: Option<String>,
    #[serde(default)]
    reason_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadCodeAssistResponse {
    #[serde(default)]
    current_tier: Option<Tier>,
    #[serde(default)]
    paid_tier: Option<Tier>,
    #[serde(default)]
    allowed_tiers: Vec<Tier>,
    #[serde(default)]
    ineligible_tiers: Vec<IneligibleTier>,
    #[serde(default)]
    cloudaicompanion_project: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OperationError {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OnboardResponse {
    #[serde(rename = "@type")]
    response_type: String,
    #[serde(default, rename = "cloudaicompanionProject")]
    project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OnboardOperation {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    error: Option<OperationError>,
    #[serde(default)]
    response: Option<OnboardResponse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OAuthClientIdentity<'a> {
    client_id: &'a str,
    client_secret: &'a str,
}

fn resolve_oauth_client_identity<'a>(
    client_id: Option<&'a str>,
    client_secret: Option<&'a str>,
) -> Result<OAuthClientIdentity<'a>, String> {
    let client_id = client_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "provider_oauth_client_unconfigured".to_string())?;
    let client_secret = client_secret
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "provider_oauth_client_unconfigured".to_string())?;
    Ok(OAuthClientIdentity {
        client_id,
        client_secret,
    })
}

fn oauth_client_identity() -> Result<OAuthClientIdentity<'static>, String> {
    resolve_oauth_client_identity(
        option_env!("EUD_ANTIGRAVITY_OAUTH_CLIENT_ID"),
        option_env!("EUD_ANTIGRAVITY_OAUTH_CLIENT_SECRET"),
    )
}

fn build_authorization_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
) -> Result<reqwest::Url, String> {
    let mut url =
        reqwest::Url::parse(AUTH_URL).map_err(|_| "provider_protocol_changed".to_string())?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", &SCOPES.join(" "))
        .append_pair("state", state)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent");
    Ok(url)
}

fn token_exchange_form(
    oauth_client: OAuthClientIdentity<'_>,
    code: String,
    redirect_uri: String,
) -> [(&'static str, String); 5] {
    [
        ("client_id", oauth_client.client_id.to_string()),
        ("client_secret", oauth_client.client_secret.to_string()),
        ("code", code),
        ("grant_type", "authorization_code".to_string()),
        ("redirect_uri", redirect_uri),
    ]
}

pub async fn login(
    dirs: &crate::config::DataDirs,
    cancellation: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    let oauth_client = oauth_client_identity()?;
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", CALLBACK_PORT)).await {
        Ok(listener) => listener,
        Err(_) => tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|_| "provider_transport_closed".to_string())?,
    };
    let port = listener
        .local_addr()
        .map_err(|_| "provider_transport_closed".to_string())?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let state = random_urlsafe(32);
    let url = build_authorization_url(oauth_client.client_id, &redirect_uri, &state)?;
    open_system_browser(url.as_str())?;

    let accept = tokio::time::timeout(OAUTH_TIMEOUT, async {
        tokio::select! {
            accepted = listener.accept() => accepted.map_err(|_| "provider_transport_closed".to_string()),
            changed = cancellation.changed() => {
                if changed.is_ok() && *cancellation.borrow() {
                    Err("provider_cancelled".to_string())
                } else {
                    Err("provider_transport_closed".to_string())
                }
            }
        }
    })
    .await
    .map_err(|_| "provider_cancelled".to_string())??;
    let (mut stream, address) = accept;
    if !address.ip().is_loopback() {
        let _ = write_callback(&mut stream, CallbackOutcome::Failed).await;
        return Err("provider_protocol_changed".to_string());
    }
    let buffer = match read_callback_request(&mut stream).await {
        Ok(buffer) => buffer,
        Err(error) => {
            let _ = write_callback(&mut stream, CallbackOutcome::Failed).await;
            return Err(error);
        }
    };
    let request = match std::str::from_utf8(&buffer) {
        Ok(request) => request,
        Err(_) => {
            let _ = write_callback(&mut stream, CallbackOutcome::Failed).await;
            return Err("provider_protocol_changed".to_string());
        }
    };
    let code = match parse_callback_payload(request, port, &state) {
        Ok(CallbackPayload::Code(code)) => code,
        Ok(CallbackPayload::Cancelled) => {
            let _ = write_callback(&mut stream, CallbackOutcome::Cancelled).await;
            return Err("provider_cancelled".to_string());
        }
        Err(error) => {
            let _ = write_callback(&mut stream, CallbackOutcome::Failed).await;
            return Err(error);
        }
    };

    let result = tokio::select! {
        result = exchange_and_store(dirs, oauth_client, code, redirect_uri) => result,
        changed = cancellation.changed() => {
            if changed.is_ok() && *cancellation.borrow() {
                Err("provider_cancelled".to_string())
            } else {
                Err("provider_transport_closed".to_string())
            }
        }
    };
    let outcome = match &result {
        Ok(()) => CallbackOutcome::Completed,
        Err(error) if error == "provider_cancelled" => CallbackOutcome::Cancelled,
        Err(_) => CallbackOutcome::Failed,
    };
    let _ = write_callback(&mut stream, outcome).await;
    result
}

fn parse_callback_payload(
    request: &str,
    port: u16,
    expected_state: &str,
) -> Result<CallbackPayload, String> {
    let target = request
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("GET "))
        .and_then(|line| line.strip_suffix(" HTTP/1.1"))
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    let callback = reqwest::Url::parse(&format!("http://127.0.0.1:{port}{target}"))
        .map_err(|_| "provider_protocol_changed".to_string())?;
    if callback.path() != CALLBACK_PATH {
        return Err("provider_protocol_changed".to_string());
    }
    let query = callback
        .query_pairs()
        .collect::<std::collections::BTreeMap<_, _>>();
    if query.get("state").map(|value| value.as_ref()) != Some(expected_state) {
        return Err("provider_protocol_changed".to_string());
    }
    if query.contains_key("error") {
        return Ok(CallbackPayload::Cancelled);
    }
    query
        .get("code")
        .map(|value| value.to_string())
        .filter(|code| !code.is_empty() && code.len() <= 4096)
        .map(CallbackPayload::Code)
        .ok_or_else(|| "provider_protocol_changed".to_string())
}

async fn exchange_and_store(
    dirs: &crate::config::DataDirs,
    oauth_client: OAuthClientIdentity<'_>,
    code: String,
    redirect_uri: String,
) -> Result<(), String> {
    let client = http_client()?;
    let form = token_exchange_form(oauth_client, code, redirect_uri);
    let response = client
        .post(TOKEN_URL)
        .form(&form)
        .send()
        .await
        .map_err(|_| "provider_transport_closed".to_string())?;
    let token = parse_token_response(response).await?;
    let granted_scopes = parse_scopes(&token.scope)?;
    let project_id = discover_project(&client, &token.access_token).await?;
    let credential = AntigravityCredential {
        access_token: token.access_token,
        refresh_token: token
            .refresh_token
            .ok_or_else(|| "provider_oauth_exchange_failed".to_string())?,
        expires_at: now_seconds()
            .saturating_add(token.expires_in)
            .saturating_sub(60),
        granted_scopes,
        project_id,
    };
    let serialized = Zeroizing::new(
        serde_json::to_string(&credential).map_err(|_| "provider_protocol_changed".to_string())?,
    );
    let store = ProviderSecretStore::new(dirs.clone())
        .map_err(|_| "provider_credential_store_unavailable".to_string())?;
    store
        .save_secret(ProviderId::Antigravity, SECRET_NAME, &serialized)
        .map_err(|_| "provider_credential_store_unavailable".to_string())
}

pub async fn status(dirs: &crate::config::DataDirs) -> AntigravityStatus {
    let credential = match access_credential(dirs).await {
        Ok(credential) => credential,
        Err(code) => {
            let (availability, detail_code) = match code.as_str() {
                "provider_credential_missing" => (
                    ProviderAvailability::NeedsAuthentication,
                    ProviderStatusCode::ProviderCredentialMissing,
                ),
                "provider_oauth_client_unconfigured" => (
                    ProviderAvailability::Unavailable,
                    ProviderStatusCode::ProviderOauthClientUnconfigured,
                ),
                _ => (
                    ProviderAvailability::Unavailable,
                    ProviderStatusCode::ProviderProtocolChanged,
                ),
            };
            return AntigravityStatus {
                availability,
                detail_code: Some(detail_code),
            };
        }
    };
    let client = match http_client() {
        Ok(client) => client,
        Err(_) => {
            return AntigravityStatus {
                availability: ProviderAvailability::Unavailable,
                detail_code: Some(ProviderStatusCode::ProviderTransportClosed),
            }
        }
    };
    match load_code_assist(&client, &credential.access_token).await {
        Ok(_) => AntigravityStatus {
            availability: ProviderAvailability::Ready,
            detail_code: None,
        },
        Err(code) => AntigravityStatus {
            availability: ProviderAvailability::Unavailable,
            detail_code: Some(status_detail_code(&code)),
        },
    }
}

pub fn logout(dirs: &crate::config::DataDirs) -> Result<(), String> {
    ProviderSecretStore::new(dirs.clone())?.delete_secret(ProviderId::Antigravity, SECRET_NAME)
}

pub async fn access_credential(
    dirs: &crate::config::DataDirs,
) -> Result<AntigravityCredential, String> {
    let _ = oauth_client_identity()?;
    let store = ProviderSecretStore::new(dirs.clone())?;
    let serialized = Zeroizing::new(
        store
            .read_secret(ProviderId::Antigravity, SECRET_NAME)?
            .ok_or_else(|| "provider_credential_missing".to_string())?,
    );
    let mut credential: AntigravityCredential =
        serde_json::from_str(&serialized).map_err(|_| "provider_not_authenticated".to_string())?;
    if credential.expires_at > now_seconds().saturating_add(60) {
        return Ok(credential);
    }
    let lock = refresh_lock();
    let _guard = lock.lock().await;
    let serialized = Zeroizing::new(
        store
            .read_secret(ProviderId::Antigravity, SECRET_NAME)?
            .ok_or_else(|| "provider_credential_missing".to_string())?,
    );
    credential =
        serde_json::from_str(&serialized).map_err(|_| "provider_not_authenticated".to_string())?;
    if credential.expires_at > now_seconds().saturating_add(60) {
        return Ok(credential);
    }
    let refreshed = refresh_token(&credential).await?;
    let serialized = Zeroizing::new(
        serde_json::to_string(&refreshed).map_err(|_| "provider_protocol_changed".to_string())?,
    );
    store.save_secret(ProviderId::Antigravity, SECRET_NAME, &serialized)?;
    Ok(refreshed)
}

pub async fn force_refresh(
    dirs: &crate::config::DataDirs,
) -> Result<AntigravityCredential, String> {
    let lock = refresh_lock();
    let _guard = lock.lock().await;
    let store = ProviderSecretStore::new(dirs.clone())?;
    let serialized = Zeroizing::new(
        store
            .read_secret(ProviderId::Antigravity, SECRET_NAME)?
            .ok_or_else(|| "provider_credential_missing".to_string())?,
    );
    let credential: AntigravityCredential =
        serde_json::from_str(&serialized).map_err(|_| "provider_not_authenticated".to_string())?;
    let refreshed = refresh_token(&credential).await?;
    let serialized = Zeroizing::new(
        serde_json::to_string(&refreshed).map_err(|_| "provider_protocol_changed".to_string())?,
    );
    store.save_secret(ProviderId::Antigravity, SECRET_NAME, &serialized)?;
    Ok(refreshed)
}

async fn refresh_token(
    credential: &AntigravityCredential,
) -> Result<AntigravityCredential, String> {
    let oauth_client = oauth_client_identity()?;
    let client = http_client()?;
    let mut form = vec![
        ("client_id", oauth_client.client_id.to_string()),
        ("refresh_token", credential.refresh_token.clone()),
        ("grant_type", "refresh_token".to_string()),
    ];
    form.push(("client_secret", oauth_client.client_secret.to_string()));
    let response = client
        .post(TOKEN_URL)
        .form(&form)
        .send()
        .await
        .map_err(|_| "provider_transport_closed".to_string())?;
    let token = parse_token_response(response).await?;
    let scopes = if token.scope.trim().is_empty() {
        credential.granted_scopes.clone()
    } else {
        parse_scopes(&token.scope)?
    };
    Ok(AntigravityCredential {
        access_token: token.access_token,
        refresh_token: token
            .refresh_token
            .unwrap_or_else(|| credential.refresh_token.clone()),
        expires_at: now_seconds()
            .saturating_add(token.expires_in)
            .saturating_sub(60),
        granted_scopes: scopes,
        project_id: credential.project_id.clone(),
    })
}

async fn parse_token_response(response: reqwest::Response) -> Result<TokenResponse, String> {
    if !response.status().is_success() {
        return Err("provider_oauth_exchange_failed".to_string());
    }
    let bytes = bounded_body(response, 1024 * 1024).await?;
    let token: TokenResponse =
        serde_json::from_slice(&bytes).map_err(|_| "provider_protocol_changed".to_string())?;
    if token.access_token.is_empty()
        || token.access_token.len() > 16 * 1024
        || token.expires_in == 0
        || !token.token_type.eq_ignore_ascii_case("bearer")
    {
        return Err("provider_protocol_changed".to_string());
    }
    Ok(token)
}

fn parse_scopes(scopes: &str) -> Result<Vec<String>, String> {
    if scopes.len() > 16 * 1024 {
        return Err("provider_protocol_changed".to_string());
    }
    let granted = scopes
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if granted.len() > 64 || granted.iter().any(|scope| scope.len() > 512) {
        return Err("provider_protocol_changed".to_string());
    }
    Ok(granted)
}

async fn discover_project(client: &reqwest::Client, access_token: &str) -> Result<String, String> {
    let initial = load_code_assist(client, access_token).await?;
    ensure_free_tier_eligible(&initial)?;
    if initial.current_tier.is_none() {
        onboard_user(client, access_token).await?;
    }
    let refreshed = load_code_assist(client, access_token).await?;
    refreshed
        .cloudaicompanion_project
        .filter(|project| !project.is_empty() && project.len() <= 256)
        .ok_or_else(|| "provider_onboarding_required".to_string())
}
fn load_code_assist_followup_project(response: &LoadCodeAssistResponse) -> Option<&str> {
    response
        .paid_tier
        .is_none()
        .then_some(response.cloudaicompanion_project.as_deref())
        .flatten()
}

async fn load_code_assist(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<LoadCodeAssistResponse, String> {
    let initial = post_load_code_assist(client, access_token, None).await?;
    if let Some(project) = load_code_assist_followup_project(&initial) {
        return post_load_code_assist(client, access_token, Some(project)).await;
    }
    Ok(initial)
}

async fn post_load_code_assist(
    client: &reqwest::Client,
    access_token: &str,
    project: Option<&str>,
) -> Result<LoadCodeAssistResponse, String> {
    let mut body = json_metadata();
    if let Some(project) = project {
        body["cloudaicompanionProject"] = serde_json::Value::String(project.to_string());
    }
    let response = client
        .post(format!("{CLOUD_CODE_ENDPOINT}/v1internal:loadCodeAssist"))
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|_| "provider_transport_closed".to_string())?;
    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }
    let bytes = bounded_body(response, 2 * 1024 * 1024).await?;
    serde_json::from_slice(&bytes).map_err(|_| "provider_protocol_changed".to_string())
}

async fn onboard_user(client: &reqwest::Client, access_token: &str) -> Result<(), String> {
    let response = client
        .post(format!("{CLOUD_CODE_ENDPOINT}/v1internal:onboardUser"))
        .bearer_auth(access_token)
        .json(&serde_json::json!({"tierId":"free-tier","metadata":{"ideType":"ANTIGRAVITY"}}))
        .send()
        .await
        .map_err(|_| "provider_transport_closed".to_string())?;
    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }
    let bytes = bounded_body(response, 2 * 1024 * 1024).await?;
    let mut operation: OnboardOperation =
        serde_json::from_slice(&bytes).map_err(|_| "provider_protocol_changed".to_string())?;
    let deadline = tokio::time::Instant::now() + ONBOARD_TIMEOUT;
    loop {
        if operation.done {
            if let Some(error) = operation.error {
                let _ = (error.code, error.message);
                return Err("provider_onboarding_required".to_string());
            }
            let response = operation
                .response
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            if response.response_type.is_empty() {
                return Err("provider_protocol_changed".to_string());
            }
            let _ = response.project_id;
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("provider_transport_closed".to_string());
        }
        tokio::time::sleep(ONBOARD_POLL).await;
        let name = operation
            .name
            .as_deref()
            .filter(|name| !name.is_empty() && name.len() <= 512 && !name.contains(".."))
            .ok_or_else(|| "provider_protocol_changed".to_string())?;
        let response = client
            .get(format!("{CLOUD_CODE_ENDPOINT}/v1internal/{name}"))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| "provider_transport_closed".to_string())?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        let bytes = bounded_body(response, 2 * 1024 * 1024).await?;
        operation =
            serde_json::from_slice(&bytes).map_err(|_| "provider_protocol_changed".to_string())?;
    }
}

fn ensure_free_tier_eligible(response: &LoadCodeAssistResponse) -> Result<(), String> {
    let _ = response
        .paid_tier
        .as_ref()
        .and_then(|tier| tier.id.as_deref());
    if response
        .allowed_tiers
        .iter()
        .any(|tier| tier.id.as_deref() == Some("free-tier"))
    {
        return Ok(());
    }
    if response
        .ineligible_tiers
        .iter()
        .any(|tier| tier.tier_id.as_deref() == Some("free-tier") && tier.reason_message.is_some())
    {
        return Err("provider_account_ineligible".to_string());
    }
    Ok(())
}

fn json_metadata() -> serde_json::Value {
    serde_json::json!({"metadata":{"ideType":"ANTIGRAVITY"}})
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .user_agent(ANTIGRAVITY_USER_AGENT)
        .build()
        .map_err(|_| "provider_transport_closed".to_string())
}

async fn bounded_body(response: reqwest::Response, cap: usize) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > cap as u64)
    {
        return Err("provider_protocol_changed".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "provider_transport_closed".to_string())?;
    if bytes.len() > cap {
        return Err("provider_protocol_changed".to_string());
    }
    Ok(bytes.to_vec())
}

fn status_error(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        401 => "provider_cloud_code_unauthorized",
        403 => "provider_account_ineligible",
        429 => "provider_rate_limited",
        500..=599 => "provider_transport_closed",
        _ => "provider_protocol_changed",
    }
    .to_string()
}

fn status_detail_code(code: &str) -> ProviderStatusCode {
    match code {
        "provider_cloud_code_unauthorized" => ProviderStatusCode::ProviderCloudCodeUnauthorized,
        "provider_account_ineligible" => ProviderStatusCode::ProviderAccountIneligible,
        "provider_onboarding_required" => ProviderStatusCode::ProviderOnboardingRequired,
        "provider_rate_limited" => ProviderStatusCode::ProviderRateLimited,
        "provider_transport_closed" => ProviderStatusCode::ProviderTransportClosed,
        _ => ProviderStatusCode::ProviderProtocolChanged,
    }
}

fn random_urlsafe(bytes: usize) -> String {
    let mut material = Vec::with_capacity(bytes + 16);
    while material.len() < bytes {
        material.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    material.truncate(bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(material)
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

static REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn refresh_lock() -> &'static tokio::sync::Mutex<()> {
    &REFRESH_LOCK
}

async fn read_callback_request(stream: &mut tokio::net::TcpStream) -> Result<Vec<u8>, String> {
    let mut request = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    loop {
        let size = stream
            .read(&mut chunk)
            .await
            .map_err(|_| "provider_transport_closed".to_string())?;
        if size == 0 {
            return Err("provider_protocol_changed".to_string());
        }
        request.extend_from_slice(&chunk[..size]);
        if request.len() > 16 * 1024 {
            return Err("provider_protocol_changed".to_string());
        }
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(request);
        }
    }
}

async fn write_callback(
    stream: &mut tokio::net::TcpStream,
    outcome: CallbackOutcome,
) -> Result<(), String> {
    let body = callback_body(outcome);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(), body
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|_| "provider_transport_closed".to_string())
}

fn callback_body(outcome: CallbackOutcome) -> &'static str {
    match outcome {
        CallbackOutcome::Completed => {
            "eud-agent Antigravity connection completed. You can close this window."
        }
        CallbackOutcome::Cancelled => "eud-agent Antigravity connection was cancelled.",
        CallbackOutcome::Failed => {
            "eud-agent Antigravity connection failed. Return to the app and try again."
        }
    }
}

#[cfg(windows)]
fn open_system_browser(url: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let operation = std::ffi::OsStr::new("open")
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let target = std::ffi::OsStr::new(url)
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        return Err("provider_transport_closed".to_string());
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_system_browser(_url: &str) -> Result<(), String> {
    Err("provider_transport_closed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oauth_identity_requires_both_build_values() {
        assert_eq!(
            resolve_oauth_client_identity(None, Some("secret")).unwrap_err(),
            "provider_oauth_client_unconfigured"
        );
        assert_eq!(
            resolve_oauth_client_identity(Some("client"), None).unwrap_err(),
            "provider_oauth_client_unconfigured"
        );
        let identity = resolve_oauth_client_identity(Some(" client "), Some(" secret ")).unwrap();
        assert_eq!(identity.client_id, "client");
        assert_eq!(identity.client_secret, "secret");
    }

    #[test]
    fn oauth_state_is_urlsafe() {
        let state = random_urlsafe(32);
        assert!(!state.contains(['+', '/', '=']));
    }

    #[test]
    fn compatibility_oauth_request_matches_the_registered_desktop_flow() {
        let redirect_uri = format!("http://127.0.0.1:{CALLBACK_PORT}{CALLBACK_PATH}");
        let url = build_authorization_url("client", &redirect_uri, "state").unwrap();
        let query = url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            query.get("redirect_uri").map(|value| value.as_ref()),
            Some(redirect_uri.as_str())
        );
        assert!(!query.contains_key("code_challenge"));

        let form = token_exchange_form(
            OAuthClientIdentity {
                client_id: "client",
                client_secret: "secret",
            },
            "code".to_string(),
            redirect_uri,
        );
        let keys = form.iter().map(|(key, _)| *key).collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "client_id",
                "client_secret",
                "code",
                "grant_type",
                "redirect_uri",
            ]
        );
    }

    #[test]
    fn cloud_code_identity_and_followup_match_the_antigravity_client() {
        assert_eq!(
            ANTIGRAVITY_USER_AGENT,
            "antigravity/hub/2.10.0 (aidev_client; os_type=darwin; arch=arm64; cl=963137146)"
        );
        let response: LoadCodeAssistResponse = serde_json::from_value(serde_json::json!({
            "cloudaicompanionProject": "project-1"
        }))
        .unwrap();
        assert_eq!(
            load_code_assist_followup_project(&response),
            Some("project-1")
        );
        let paid: LoadCodeAssistResponse = serde_json::from_value(serde_json::json!({
            "paidTier": {"id": "paid"},
            "cloudaicompanionProject": "project-1"
        }))
        .unwrap();
        assert_eq!(load_code_assist_followup_project(&paid), None);
    }

    #[test]
    fn cloud_code_account_failures_keep_distinct_recovery_codes() {
        assert_eq!(
            status_error(reqwest::StatusCode::UNAUTHORIZED),
            "provider_cloud_code_unauthorized"
        );
        assert_eq!(
            status_error(reqwest::StatusCode::FORBIDDEN),
            "provider_account_ineligible"
        );
        assert_eq!(
            status_detail_code("provider_onboarding_required"),
            ProviderStatusCode::ProviderOnboardingRequired
        );
    }

    #[test]
    fn returned_scope_metadata_is_bounded_but_not_an_auth_gate() {
        assert!(parse_scopes("https://www.googleapis.com/auth/cloud-platform").is_ok());
        assert!(parse_scopes(&SCOPES.join(" ")).is_ok());
        assert!(parse_scopes(&"x".repeat(16 * 1024 + 1)).is_err());
    }

    #[test]
    fn callback_copy_reports_only_persisted_success_as_completed() {
        assert!(callback_body(CallbackOutcome::Completed).contains("completed"));
        assert!(callback_body(CallbackOutcome::Cancelled).contains("cancelled"));
        assert!(callback_body(CallbackOutcome::Failed).contains("failed"));
    }
}
