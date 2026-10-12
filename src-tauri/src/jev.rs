//! Opt-in TypeSafe Jev rerank for `search_docs`.
//!
//! Each candidate chunk is scored by one Jev `noul` relevance question; all
//! candidate calls run concurrently on a small dedicated runtime so the
//! synchronous tool dispatcher (already on a blocking thread) can wait for
//! them without touching the app's async runtime. Every failure is fail-open:
//! the caller keeps its ordinary hybrid ranking.
//!
//! The API key lives only in the OS credential store
//! (`eud-agent/integrations/jev/api-key`); it is never written to config,
//! logs, or IPC responses.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::config::DataDirs;

pub const JEV_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
pub const JEV_MODEL: &str = "jev-latest";
/// Lexical and dense pool size gathered for reranking (beyond `MAX_TOP_K`).
pub const POOL_SIZE: usize = 15;
/// Minimum `noul` for a reranked hit to be returned.
pub const RELEVANT_THRESHOLD: f64 = 0.7;
/// Hits returned when no candidate reaches [`RELEVANT_THRESHOLD`].
pub const FALLBACK_TOP: usize = 3;
pub const PASSAGE_MAX_CHARS: usize = 3000;
pub const MAX_CONCURRENCY: usize = 32;
/// Whole-rerank budget; past it `search_docs` returns the hybrid result.
pub const RERANK_TIMEOUT: Duration = Duration::from_secs(4);
const RETRY_BACKOFF: Duration = Duration::from_millis(300);
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

const SECRET_INTEGRATION: &str = "jev";
const SECRET_NAME: &str = "api-key";

const REL_INSTRUCTIONS: &str = "An agent building a StarCraft EUD map (epScript/euddraft/TrigEditPlus) searched a Korean reference corpus with the query. Does the candidate passage contain information that helps answer the query?";
const REL_TRUE: &str =
    "The passage explains, defines, or shows code/steps for what the query is looking for.";
const REL_FALSE: &str =
    "The passage merely shares keywords or a nearby topic but does not help answer the query.";

pub const ERR_KEY_INVALID: &str = "jev_key_invalid";
pub const ERR_UNREACHABLE: &str = "jev_unreachable";
pub const ERR_UNAVAILABLE: &str = "jev_unavailable";
pub const ERR_KEY_MISSING: &str = "jev_key_missing";
pub const ERR_KEY_STORE: &str = "jev_key_store";

/// One raw HTTP outcome from the Jev endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

pub type TransportFuture =
    Pin<Box<dyn Future<Output = Result<TransportResponse, String>> + Send + 'static>>;

/// The POST seam: production uses reqwest, tests inject a fake.
/// `Err` means the request never produced an HTTP response (network failure).
pub trait JevTransport: Send + Sync {
    fn post(&self, api_key: &str, body: Vec<u8>) -> TransportFuture;
}

/// Why one Jev call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JevError {
    Unauthorized,
    Unreachable,
    Timeout,
    Status(u16),
    Malformed,
}

impl JevError {
    /// Stable error code surfaced to the panel.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized => ERR_KEY_INVALID,
            Self::Unreachable | Self::Timeout => ERR_UNREACHABLE,
            Self::Status(_) | Self::Malformed => ERR_UNAVAILABLE,
        }
    }

    fn reason(&self) -> String {
        match self {
            Self::Unauthorized => "unauthorized (401)".to_string(),
            Self::Unreachable => "network failure".to_string(),
            Self::Timeout => "timeout".to_string(),
            Self::Status(status) => format!("http status {status}"),
            Self::Malformed => "malformed response".to_string(),
        }
    }
}

/// reqwest transport on the dedicated Jev runtime.
pub struct HttpTransport {
    client: reqwest::Client,
    endpoint: String,
}

impl HttpTransport {
    fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| "jev http client cannot be built".to_string())?;
        Ok(Self {
            client,
            endpoint: JEV_ENDPOINT.to_string(),
        })
    }
}

impl JevTransport for HttpTransport {
    fn post(&self, api_key: &str, body: Vec<u8>) -> TransportFuture {
        let authorization = Zeroizing::new(format!("Bearer {api_key}"));
        let header = reqwest::header::HeaderValue::from_str(&authorization).map(|mut value| {
            value.set_sensitive(true);
            value
        });
        let request = header.map(|header| {
            self.client
                .post(&self.endpoint)
                .header(reqwest::header::AUTHORIZATION, header)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
        });
        Box::pin(async move {
            // An unencodable key cannot authenticate; treat it like a 401.
            let Ok(request) = request else {
                return Ok(TransportResponse {
                    status: 401,
                    body: Vec::new(),
                });
            };
            let response = request
                .send()
                .await
                .map_err(|_| "send failed".to_string())?;
            let status = response.status().as_u16();
            let body = response
                .bytes()
                .await
                .map_err(|_| "body read failed".to_string())?
                .to_vec();
            Ok(TransportResponse { status, body })
        })
    }
}

/// The process-wide Jev runtime and HTTP transport, created on first use.
/// A dedicated runtime keeps the pooled client bound to one live runtime and
/// lets synchronous callers wait without `block_on` inside the app runtime.
type Shared = (tokio::runtime::Runtime, Arc<dyn JevTransport>);

fn shared() -> Result<&'static Shared, String> {
    static SHARED: OnceLock<Result<Shared, String>> = OnceLock::new();
    SHARED
        .get_or_init(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("eud-agent-jev")
                .enable_all()
                .build()
                .map_err(|_| "jev runtime cannot start".to_string())?;
            let transport: Arc<dyn JevTransport> = Arc::new(HttpTransport::new()?);
            Ok((runtime, transport))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// The exact relevance question every rerank and validation call asks.
fn relevance_question() -> Value {
    json!({
        "rel": {
            "type": "noul",
            "instructions": REL_INSTRUCTIONS,
            "criteria": {"true": REL_TRUE, "false": REL_FALSE},
        }
    })
}

fn truncate_chars(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((index, _)) => &text[..index],
        None => text,
    }
}

/// Request body for one candidate.
pub fn request_body(query: &str, passage: &str) -> Value {
    json!({
        "state": {
            "query": query,
            "candidate_passage": truncate_chars(passage, PASSAGE_MAX_CHARS),
        },
        "model": JEV_MODEL,
        "questions": relevance_question(),
    })
}

fn parse_noul(body: &[u8]) -> Result<f64, JevError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| JevError::Malformed)?;
    value
        .pointer("/answers/rel/noul")
        .and_then(Value::as_f64)
        .filter(|score| score.is_finite() && (0.0..=1.0).contains(score))
        .ok_or(JevError::Malformed)
}

/// Ask one question; retry once on 429/529 after a short backoff (the caller's
/// timeout bounds the whole exchange).
async fn ask(transport: &dyn JevTransport, api_key: &str, body: &[u8]) -> Result<f64, JevError> {
    let mut retried = false;
    loop {
        let response = transport
            .post(api_key, body.to_vec())
            .await
            .map_err(|_| JevError::Unreachable)?;
        match response.status {
            200..=299 => return parse_noul(&response.body),
            401 => return Err(JevError::Unauthorized),
            429 | 529 if !retried => {
                retried = true;
                tokio::time::sleep(RETRY_BACKOFF).await;
            }
            status => return Err(JevError::Status(status)),
        }
    }
}

/// Score every passage concurrently (at most [`MAX_CONCURRENCY`] in flight).
/// Any single failure or the overall `timeout` fails the whole batch.
pub async fn score_all(
    transport: Arc<dyn JevTransport>,
    api_key: Arc<Zeroizing<String>>,
    query: &str,
    passages: &[String],
    timeout: Duration,
) -> Result<Vec<f64>, JevError> {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENCY));
    let mut tasks = tokio::task::JoinSet::new();
    for (index, passage) in passages.iter().enumerate() {
        let body =
            serde_json::to_vec(&request_body(query, passage)).map_err(|_| JevError::Malformed)?;
        let (transport, api_key, semaphore) = (
            Arc::clone(&transport),
            Arc::clone(&api_key),
            Arc::clone(&semaphore),
        );
        tasks.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| JevError::Unreachable)?;
            ask(transport.as_ref(), &api_key, &body)
                .await
                .map(|score| (index, score))
        });
    }
    let collect = async {
        let mut scores = vec![0.0; passages.len()];
        while let Some(joined) = tasks.join_next().await {
            let (index, score) = joined.map_err(|_| JevError::Unreachable)??;
            scores[index] = score;
        }
        Ok(scores)
    };
    // Dropping the JoinSet on timeout or error aborts the remaining calls.
    tokio::time::timeout(timeout, collect)
        .await
        .map_err(|_| JevError::Timeout)?
}

/// Indices (into the scored candidates) to return, best first, with scores:
/// every candidate at or above [`RELEVANT_THRESHOLD`] capped at `k`, or the
/// top [`FALLBACK_TOP`] (capped at `k`) when none qualifies. Ties keep the
/// candidate order.
pub fn select(scores: &[f64], k: usize) -> Vec<(usize, f64)> {
    let mut ranked: Vec<(usize, f64)> = scores.iter().copied().enumerate().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let relevant = ranked
        .iter()
        .take_while(|(_, score)| *score >= RELEVANT_THRESHOLD)
        .count();
    let keep = if relevant > 0 {
        relevant.min(k)
    } else {
        FALLBACK_TOP.min(k)
    };
    ranked.truncate(keep);
    ranked
}

/// Why the rerank was skipped; the caller keeps the hybrid ranking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    NoCandidates,
    NoKey,
    KeyStore,
    Failed(JevError),
}

/// Grace past the rerank budget for the scoring task to report back.
const RECV_MARGIN: Duration = Duration::from_millis(250);

type KeySource = Arc<dyn Fn() -> Result<Option<Zeroizing<String>>, String> + Send + Sync>;

#[derive(Clone)]
enum Backend {
    /// The process-wide Jev runtime and reqwest transport.
    Shared,
    Custom {
        runtime: tokio::runtime::Handle,
        transport: Arc<dyn JevTransport>,
    },
}

/// The `search_docs` reranker: where calls run, where the key comes from,
/// and the whole-rerank wall-time budget.
#[derive(Clone)]
pub struct Reranker {
    backend: Backend,
    key: KeySource,
    timeout: Duration,
}

impl Reranker {
    /// Shared runtime/transport and the key from the OS credential store.
    pub fn production() -> Self {
        Self {
            backend: Backend::Shared,
            key: Arc::new(read_api_key),
            timeout: RERANK_TIMEOUT,
        }
    }

    /// A reranker on `runtime` with `transport` and a fixed key (no OS store).
    pub fn custom(
        runtime: tokio::runtime::Handle,
        transport: Arc<dyn JevTransport>,
        key: Option<&str>,
        timeout: Duration,
    ) -> Self {
        let key = key.map(str::to_string);
        Self {
            backend: Backend::Custom { runtime, transport },
            key: Arc::new(move || Ok(key.clone().map(Zeroizing::new))),
            timeout,
        }
    }

    /// Score `passages` and select hits, waiting synchronously at most the
    /// budget plus [`RECV_MARGIN`]; runtime start-up and the key read count
    /// against the budget. Logs the skip reason (never the key or query);
    /// every `Err` is fail-open.
    pub fn rerank(
        &self,
        query: &str,
        passages: Vec<String>,
        k: usize,
    ) -> Result<Vec<(usize, f64)>, Skip> {
        let outcome = self.rerank_within(std::time::Instant::now(), query, passages, k);
        match &outcome {
            Err(Skip::NoCandidates) | Ok(_) => {}
            Err(Skip::NoKey) => eprintln!("eud-agent: jev rerank skipped: no api key stored"),
            Err(Skip::KeyStore) => {
                eprintln!("eud-agent: jev rerank skipped: credential store read failed")
            }
            Err(Skip::Failed(error)) => {
                eprintln!("eud-agent: jev rerank failed open: {}", error.reason())
            }
        }
        outcome
    }

    fn rerank_within(
        &self,
        started: std::time::Instant,
        query: &str,
        passages: Vec<String>,
        k: usize,
    ) -> Result<Vec<(usize, f64)>, Skip> {
        if passages.is_empty() {
            return Err(Skip::NoCandidates);
        }
        let (runtime, transport) = match &self.backend {
            Backend::Shared => {
                let (runtime, transport) =
                    shared().map_err(|_| Skip::Failed(JevError::Unreachable))?;
                (runtime.handle().clone(), Arc::clone(transport))
            }
            Backend::Custom { runtime, transport } => (runtime.clone(), Arc::clone(transport)),
        };
        let api_key = match (self.key)() {
            Ok(Some(key)) if !key.is_empty() => Arc::new(key),
            Ok(_) => return Err(Skip::NoKey),
            Err(_) => return Err(Skip::KeyStore),
        };
        let remaining = self
            .timeout
            .checked_sub(started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(Skip::Failed(JevError::Timeout))?;
        let query = query.to_string();
        let (sender, receiver) = std::sync::mpsc::channel();
        runtime.spawn(async move {
            let result = score_all(transport, api_key, &query, &passages, remaining).await;
            let _ = sender.send(result);
        });
        let scores = match receiver.recv_timeout(remaining + RECV_MARGIN) {
            Ok(result) => result.map_err(Skip::Failed)?,
            // A starved runtime still cannot hold the search past the budget.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                return Err(Skip::Failed(JevError::Timeout))
            }
            // A dropped sender (panicked task) is a failure, not a hang.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Skip::Failed(JevError::Unreachable))
            }
        };
        Ok(select(&scores, k))
    }
}

/// One tiny scored request proving `api_key` authenticates.
pub async fn validate_key(transport: &dyn JevTransport, api_key: &str) -> Result<(), JevError> {
    let body = serde_json::to_vec(&request_body(
        "epScript 변수 선언",
        "epScript에서 var 키워드로 변수를 선언합니다.",
    ))
    .map_err(|_| JevError::Malformed)?;
    tokio::time::timeout(VALIDATE_TIMEOUT, ask(transport, api_key, &body))
        .await
        .map_err(|_| JevError::Timeout)?
        .map(|_| ())
}

pub fn read_api_key() -> Result<Option<Zeroizing<String>>, String> {
    crate::provider_secrets::read_integration_secret(SECRET_INTEGRATION, SECRET_NAME)
}

/// Credential operations behind the settings commands (tests use memory).
pub trait JevKeyStore: Send + Sync {
    fn has_key(&self) -> Result<bool, String>;
    fn save(&self, key: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

struct OsKeyStore;

impl JevKeyStore for OsKeyStore {
    fn has_key(&self) -> Result<bool, String> {
        read_api_key().map(|key| key.is_some_and(|key| !key.is_empty()))
    }
    fn save(&self, key: &str) -> Result<(), String> {
        crate::provider_secrets::save_integration_secret(SECRET_INTEGRATION, SECRET_NAME, key)
    }
    fn delete(&self) -> Result<(), String> {
        crate::provider_secrets::delete_integration_secret(SECRET_INTEGRATION, SECRET_NAME)
    }
}

/// Secret-free Jev settings view for the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JevSettingsView {
    pub enabled: bool,
    pub has_key: bool,
}

pub fn settings_payload(
    dirs: &DataDirs,
    store: &dyn JevKeyStore,
) -> Result<JevSettingsView, String> {
    let config = dirs.load_config().map_err(|error| error.to_string())?;
    Ok(JevSettingsView {
        enabled: config.jev.enabled,
        has_key: store.has_key().map_err(|_| ERR_KEY_STORE.to_string())?,
    })
}

/// A provided key, trimmed; blank means no key.
pub fn provided_key(api_key: Option<Zeroizing<String>>) -> Option<Zeroizing<String>> {
    api_key
        .map(|key| Zeroizing::new(key.trim().to_string()))
        .filter(|key| !key.is_empty())
}

/// One Jev request checking `api_key`, mapped to a stable error code.
/// Performs no store or config I/O, so it can run on the Jev runtime.
pub async fn validate_provided(transport: &dyn JevTransport, api_key: &str) -> Result<(), String> {
    validate_key(transport, api_key)
        .await
        .map_err(|error| error.code().to_string())
}

/// Store an already-validated key (if any), then persist `enabled`.
/// Enabling without any key fails with [`ERR_KEY_MISSING`]. Blocking I/O.
pub fn persist_payload(
    dirs: &DataDirs,
    store: &dyn JevKeyStore,
    enabled: bool,
    validated_key: Option<&str>,
) -> Result<JevSettingsView, String> {
    if let Some(key) = validated_key {
        store.save(key).map_err(|_| ERR_KEY_STORE.to_string())?;
    }
    let has_key = store.has_key().map_err(|_| ERR_KEY_STORE.to_string())?;
    if enabled && !has_key {
        return Err(ERR_KEY_MISSING.to_string());
    }
    let mut config = dirs.load_config().map_err(|error| error.to_string())?;
    config.jev.enabled = enabled;
    dirs.save_config(&config)
        .map_err(|error| error.to_string())?;
    Ok(JevSettingsView { enabled, has_key })
}

/// Delete the stored key; rerank is turned off because it cannot run keyless.
pub fn clear_key_payload(
    dirs: &DataDirs,
    store: &dyn JevKeyStore,
) -> Result<JevSettingsView, String> {
    store.delete().map_err(|_| ERR_KEY_STORE.to_string())?;
    let mut config = dirs.load_config().map_err(|error| error.to_string())?;
    if config.jev.enabled {
        config.jev.enabled = false;
        dirs.save_config(&config)
            .map_err(|error| error.to_string())?;
    }
    Ok(JevSettingsView {
        enabled: false,
        has_key: false,
    })
}

/// Read Jev rerank settings (`hasKey` only; the key never leaves the backend).
#[tauri::command]
pub async fn jev_settings(
    state: tauri::State<'_, crate::ipc::AppManaged>,
) -> Result<JevSettingsView, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || settings_payload(&dirs, &OsKeyStore))
        .await
        .map_err(|error| error.to_string())?
}

/// Save the Jev toggle and optionally a new key (validated before storing).
#[tauri::command]
pub async fn jev_settings_save(
    state: tauri::State<'_, crate::ipc::AppManaged>,
    enabled: bool,
    api_key: Option<String>,
) -> Result<JevSettingsView, String> {
    let api_key = provided_key(api_key.map(Zeroizing::new));
    // Only a new key needs the Jev runtime; disabling or re-enabling with the
    // stored key works even if it cannot start.
    if let Some(key) = &api_key {
        let (runtime, transport) = shared().map_err(|_| ERR_UNREACHABLE.to_string())?;
        let (transport, key) = (Arc::clone(transport), key.clone());
        runtime
            .spawn(async move { validate_provided(transport.as_ref(), &key).await })
            .await
            .map_err(|error| error.to_string())??;
    }
    // Credential-store and config I/O stay off the Jev worker.
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || {
        persist_payload(
            &dirs,
            &OsKeyStore,
            enabled,
            api_key.as_deref().map(String::as_str),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Delete the stored Jev key and turn rerank off.
#[tauri::command]
pub async fn jev_key_clear(
    state: tauri::State<'_, crate::ipc::AppManaged>,
) -> Result<JevSettingsView, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || clear_key_payload(&dirs, &OsKeyStore))
        .await
        .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Scripted transport: a response per call (or a default), recording bodies.
    struct FakeTransport {
        script: parking_lot::Mutex<VecDeque<Result<TransportResponse, String>>>,
        default: Result<TransportResponse, String>,
        /// Score each call from a `score=<f64>` marker in its passage.
        by_passage: bool,
        delay: Duration,
        calls: AtomicUsize,
        bodies: parking_lot::Mutex<Vec<Value>>,
        keys: parking_lot::Mutex<Vec<String>>,
    }

    impl FakeTransport {
        fn new(default: Result<TransportResponse, String>) -> Self {
            Self {
                script: parking_lot::Mutex::new(VecDeque::new()),
                default,
                by_passage: false,
                delay: Duration::ZERO,
                calls: AtomicUsize::new(0),
                bodies: parking_lot::Mutex::new(Vec::new()),
                keys: parking_lot::Mutex::new(Vec::new()),
            }
        }
        fn scripted(responses: Vec<Result<TransportResponse, String>>) -> Self {
            let fake = Self::new(Err("script exhausted".to_string()));
            *fake.script.lock() = responses.into();
            fake
        }
        fn by_passage() -> Self {
            let mut fake = Self::new(Err("unused".to_string()));
            fake.by_passage = true;
            fake
        }
    }

    impl JevTransport for FakeTransport {
        fn post(&self, api_key: &str, body: Vec<u8>) -> TransportFuture {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let value: Value = serde_json::from_slice(&body).unwrap();
            self.keys.lock().push(api_key.to_string());
            let passage = value["state"]["candidate_passage"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            self.bodies.lock().push(value);
            let response = self.script.lock().pop_front().unwrap_or_else(|| {
                if self.by_passage {
                    let score: f64 = passage
                        .split("score=")
                        .nth(1)
                        .and_then(|rest| rest.split_whitespace().next())
                        .and_then(|text| text.parse().ok())
                        .unwrap_or(0.0);
                    Ok(ok(score))
                } else {
                    self.default.clone()
                }
            });
            let delay = self.delay;
            Box::pin(async move {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                response
            })
        }
    }

    fn ok(score: f64) -> TransportResponse {
        TransportResponse {
            status: 200,
            body: serde_json::to_vec(&json!({
                "model": "jev-latest",
                "answers": {"rel": {"type": "noul", "noul": score}},
                "usage": {}
            }))
            .unwrap(),
        }
    }

    fn status(code: u16) -> TransportResponse {
        TransportResponse {
            status: code,
            body: b"{}".to_vec(),
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    fn key() -> Result<Option<Zeroizing<String>>, String> {
        Ok(Some(Zeroizing::new("test-key".to_string())))
    }

    fn reranker(
        runtime: &tokio::runtime::Runtime,
        transport: Arc<FakeTransport>,
        api_key: Result<Option<Zeroizing<String>>, String>,
        timeout: Duration,
    ) -> Reranker {
        Reranker {
            backend: Backend::Custom {
                runtime: runtime.handle().clone(),
                transport,
            },
            key: Arc::new(move || api_key.clone()),
            timeout,
        }
    }

    fn rerank(
        transport: Arc<FakeTransport>,
        api_key: Result<Option<Zeroizing<String>>, String>,
        passages: &[&str],
        k: usize,
        timeout: Duration,
    ) -> Result<Vec<(usize, f64)>, Skip> {
        let runtime = runtime();
        reranker(&runtime, transport, api_key, timeout).rerank(
            "chatEvent 사용법",
            passages.iter().map(|text| text.to_string()).collect(),
            k,
        )
    }

    #[test]
    fn request_body_carries_query_truncated_passage_and_the_exact_question() {
        let long = "가".repeat(PASSAGE_MAX_CHARS + 50);
        let body = request_body("q", &long);
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"]["query"], "q");
        assert_eq!(
            body["state"]["candidate_passage"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            PASSAGE_MAX_CHARS
        );
        assert_eq!(
            body["questions"],
            json!({"rel": {"type": "noul",
                "instructions": "An agent building a StarCraft EUD map (epScript/euddraft/TrigEditPlus) searched a Korean reference corpus with the query. Does the candidate passage contain information that helps answer the query?",
                "criteria": {"true": "The passage explains, defines, or shows code/steps for what the query is looking for.",
                             "false": "The passage merely shares keywords or a nearby topic but does not help answer the query."}}})
        );
    }

    #[test]
    fn select_keeps_relevant_hits_sorted_and_capped_at_k() {
        let scores = [0.2, 0.9, 0.7, 0.95, 0.69, 0.8];
        assert_eq!(
            select(&scores, 10),
            vec![(3, 0.95), (1, 0.9), (5, 0.8), (2, 0.7)]
        );
        assert_eq!(select(&scores, 2), vec![(3, 0.95), (1, 0.9)]);
    }

    #[test]
    fn select_falls_back_to_top_three_capped_at_k_when_none_is_relevant() {
        let scores = [0.1, 0.5, 0.3, 0.6, 0.2];
        assert_eq!(select(&scores, 5), vec![(3, 0.6), (1, 0.5), (2, 0.3)]);
        assert_eq!(select(&scores, 2), vec![(3, 0.6), (1, 0.5)]);
        // Ties keep candidate order.
        assert_eq!(select(&[0.4, 0.4], 5), vec![(0, 0.4), (1, 0.4)]);
    }

    #[test]
    fn rerank_scores_every_candidate_and_selects() {
        let transport = Arc::new(FakeTransport::by_passage());
        let selected = rerank(
            Arc::clone(&transport),
            key(),
            &["a score=0.2", "b score=0.9", "c score=0.75"],
            5,
            RERANK_TIMEOUT,
        )
        .unwrap();
        assert_eq!(selected, vec![(1, 0.9), (2, 0.75)]);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 3);
        assert!(transport.keys.lock().iter().all(|key| key == "test-key"));
    }

    #[test]
    fn rerank_is_skipped_without_a_key_on_store_failure_or_without_candidates() {
        let transport = Arc::new(FakeTransport::by_passage());
        let none = rerank(Arc::clone(&transport), key(), &[], 5, RERANK_TIMEOUT);
        assert_eq!(none, Err(Skip::NoCandidates));
        let no_key = rerank(Arc::clone(&transport), Ok(None), &["x"], 5, RERANK_TIMEOUT);
        assert_eq!(no_key, Err(Skip::NoKey));
        let empty_key = rerank(
            Arc::clone(&transport),
            Ok(Some(Zeroizing::new(String::new()))),
            &["x"],
            5,
            RERANK_TIMEOUT,
        );
        assert_eq!(empty_key, Err(Skip::NoKey));
        let store = rerank(
            Arc::clone(&transport),
            Err("store".to_string()),
            &["x"],
            5,
            RERANK_TIMEOUT,
        );
        assert_eq!(store, Err(Skip::KeyStore));
        assert_eq!(
            transport.calls.load(Ordering::SeqCst),
            0,
            "no request is sent"
        );
    }

    #[test]
    fn any_request_error_fails_the_whole_rerank_open() {
        for (response, expected) in [
            (Err("down".to_string()), JevError::Unreachable),
            (Ok(status(500)), JevError::Status(500)),
            (Ok(status(422)), JevError::Status(422)),
            (Ok(status(401)), JevError::Unauthorized),
            (
                Ok(TransportResponse {
                    status: 200,
                    body: b"{\"answers\":{}}".to_vec(),
                }),
                JevError::Malformed,
            ),
        ] {
            let transport = Arc::new(FakeTransport::scripted(vec![
                Ok(ok(0.9)),
                response,
                Ok(ok(0.9)),
            ]));
            let outcome = rerank(transport, key(), &["a", "b", "c"], 5, RERANK_TIMEOUT);
            assert_eq!(outcome, Err(Skip::Failed(expected)));
        }
    }

    #[test]
    fn overall_timeout_fails_open_within_the_budget() {
        let mut fake = FakeTransport::new(Ok(ok(0.9)));
        fake.delay = Duration::from_secs(5);
        let started = std::time::Instant::now();
        let outcome = rerank(
            Arc::new(fake),
            key(),
            &["a", "b"],
            5,
            Duration::from_millis(50),
        );
        assert_eq!(outcome, Err(Skip::Failed(JevError::Timeout)));
        assert!(
            started.elapsed() < Duration::from_millis(50) + RECV_MARGIN,
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_starved_runtime_cannot_hold_the_caller_past_budget_and_margin() {
        // A current-thread runtime nobody drives never polls the scoring task,
        // like a Jev worker stuck on blocking work.
        let starved = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let transport = Arc::new(FakeTransport::new(Ok(ok(0.9))));
        let started = std::time::Instant::now();
        let outcome = reranker(
            &starved,
            Arc::clone(&transport),
            key(),
            Duration::from_millis(50),
        )
        .rerank("q", vec!["a".to_string()], 5);
        let elapsed = started.elapsed();
        assert_eq!(outcome, Err(Skip::Failed(JevError::Timeout)));
        assert!(elapsed >= Duration::from_millis(50) + RECV_MARGIN);
        assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn rate_limits_retry_once_then_fail() {
        let transport = Arc::new(FakeTransport::scripted(vec![Ok(status(429)), Ok(ok(0.8))]));
        let outcome = rerank(Arc::clone(&transport), key(), &["a"], 5, RERANK_TIMEOUT);
        assert_eq!(outcome, Ok(vec![(0, 0.8)]));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 2);

        let transport = Arc::new(FakeTransport::scripted(vec![
            Ok(status(529)),
            Ok(status(529)),
        ]));
        let outcome = rerank(Arc::clone(&transport), key(), &["a"], 5, RERANK_TIMEOUT);
        assert_eq!(outcome, Err(Skip::Failed(JevError::Status(529))));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
    }

    #[derive(Default)]
    struct MemoryStore {
        key: parking_lot::Mutex<Option<String>>,
    }

    impl JevKeyStore for MemoryStore {
        fn has_key(&self) -> Result<bool, String> {
            Ok(self.key.lock().is_some())
        }
        fn save(&self, key: &str) -> Result<(), String> {
            *self.key.lock() = Some(key.to_string());
            Ok(())
        }
        fn delete(&self) -> Result<(), String> {
            *self.key.lock() = None;
            Ok(())
        }
    }

    fn temp_dirs() -> (std::path::PathBuf, DataDirs) {
        let base = std::env::temp_dir().join(format!("eud-agent-jev-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        (base, dirs)
    }

    fn save(
        dirs: &DataDirs,
        store: &MemoryStore,
        transport: &FakeTransport,
        enabled: bool,
        api_key: Option<&str>,
    ) -> Result<JevSettingsView, String> {
        let api_key = provided_key(api_key.map(|key| Zeroizing::new(key.to_string())));
        if let Some(key) = &api_key {
            runtime().block_on(validate_provided(transport, key))?;
        }
        persist_payload(dirs, store, enabled, api_key.as_deref().map(String::as_str))
    }

    #[test]
    fn key_validation_maps_errors_and_persists_only_valid_keys() {
        let (base, dirs) = temp_dirs();
        let store = MemoryStore::default();

        let invalid = FakeTransport::new(Ok(status(401)));
        assert_eq!(
            save(&dirs, &store, &invalid, true, Some("bad")),
            Err(ERR_KEY_INVALID.to_string())
        );
        let unreachable = FakeTransport::new(Err("dns".to_string()));
        assert_eq!(
            save(&dirs, &store, &unreachable, true, Some("bad")),
            Err(ERR_UNREACHABLE.to_string())
        );
        let unavailable = FakeTransport::new(Ok(status(503)));
        assert_eq!(
            save(&dirs, &store, &unavailable, true, Some("bad")),
            Err(ERR_UNAVAILABLE.to_string())
        );
        assert_eq!(*store.key.lock(), None, "rejected keys are never stored");
        assert!(!dirs.load_config().unwrap().jev.enabled);

        let valid = FakeTransport::new(Ok(ok(0.9)));
        assert_eq!(
            save(&dirs, &store, &valid, true, Some("  good-key  ")),
            Ok(JevSettingsView {
                enabled: true,
                has_key: true
            })
        );
        assert_eq!(store.key.lock().as_deref(), Some("good-key"));
        assert_eq!(
            valid.calls.load(Ordering::SeqCst),
            1,
            "one validation request"
        );
        assert!(dirs.load_config().unwrap().jev.enabled);
        let saved = std::fs::read_to_string(dirs.config_path()).unwrap();
        assert!(
            !saved.contains("good-key"),
            "the key never reaches config.json"
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn enabling_needs_a_key_and_disabling_keeps_it_until_cleared() {
        let (base, dirs) = temp_dirs();
        let store = MemoryStore::default();
        let transport = FakeTransport::new(Ok(ok(0.9)));

        assert_eq!(
            save(&dirs, &store, &transport, true, None),
            Err(ERR_KEY_MISSING.to_string())
        );
        assert_eq!(
            save(&dirs, &store, &transport, true, Some("   ")),
            Err(ERR_KEY_MISSING.to_string()),
            "a blank key is no key"
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);

        save(&dirs, &store, &transport, true, Some("k")).unwrap();
        // Re-enabling with the stored key needs no new key or request.
        save(&dirs, &store, &transport, true, None).unwrap();
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);

        assert_eq!(
            save(&dirs, &store, &transport, false, None),
            Ok(JevSettingsView {
                enabled: false,
                has_key: true
            })
        );
        assert_eq!(
            settings_payload(&dirs, &store),
            Ok(JevSettingsView {
                enabled: false,
                has_key: true
            })
        );

        save(&dirs, &store, &transport, true, None).unwrap();
        assert_eq!(
            clear_key_payload(&dirs, &store),
            Ok(JevSettingsView {
                enabled: false,
                has_key: false
            })
        );
        assert_eq!(*store.key.lock(), None);
        assert!(!dirs.load_config().unwrap().jev.enabled);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn settings_view_serializes_without_secret_fields() {
        let value = serde_json::to_value(JevSettingsView {
            enabled: true,
            has_key: true,
        })
        .unwrap();
        assert_eq!(value, json!({"enabled": true, "hasKey": true}));
    }
}
