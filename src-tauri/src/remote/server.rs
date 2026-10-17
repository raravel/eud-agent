//! The loopback HTTP server phones reach through `tailscale serve`.
//!
//! Binds `127.0.0.1:<port>` only. Everything lives under `/remote`: the static
//! remote panel from the app's embedded frontend, and `/remote/api/*`, which
//! requires a paired device token except for `pair`.
//!
//! A paired client opens one event socket (`/remote/api/events`), which names
//! its connection, and sends commands to `/remote/api/invoke/<command>` with
//! that connection in `x-eud-remote-conn`; only the allowlist in
//! [`super::dispatch`] is accepted. A binary command (a rendered map) answers
//! `application/octet-stream` with the bytes the desktop invoke resolves to.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{broadcast, oneshot, watch};
use tokio::task::JoinHandle;

use super::auth::{self, DeviceView, RemoteAuth};
use super::caller::{
    RemoteCaller, RemoteConnections, PROJECT_CLOSED, PROJECT_NOT_OPEN, UNKNOWN_CONNECTION,
};
use super::dispatch::{self, RemoteCall, RemoteDispatcher};
use super::hub::RemoteHub;

/// How long a stopping server may drain before its task is aborted.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const REMOTE_PAGE: &str = "remote.html";

/// One file of the embedded frontend.
pub struct StaticAsset {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

/// Looks up a frontend file by its path below the dist root (`assets/x.js`).
pub type AssetLookup = Arc<dyn Fn(&str) -> Option<StaticAsset> + Send + Sync>;

/// Serve files from Tauri's embedded frontend (the built `panel/dist`).
pub fn tauri_assets<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> AssetLookup {
    Arc::new(move |path: &str| {
        app.asset_resolver()
            .get(format!("/{path}"))
            .map(|asset| StaticAsset {
                bytes: asset.bytes,
                mime_type: asset.mime_type,
            })
    })
}

/// Header naming the event socket a command comes from.
pub const CONNECTION_HEADER: &str = "x-eud-remote-conn";
/// Header naming one command invocation, so a resent request joins it.
pub const REQUEST_HEADER: &str = "x-eud-request-id";
/// How long after it FINISHED a command answers its resent request instead
/// of running again. An unfinished command is never forgotten.
const REQUEST_MEMORY: Duration = Duration::from_secs(10 * 60);
/// Commands one device may have in the ledger (running or remembered).
const DEVICE_REQUESTS: usize = 64;
/// Commands all devices together may have in the ledger.
const LEDGER_REQUESTS: usize = 512;
/// The remembered answers' total size.
const LEDGER_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const TOO_MANY_REQUESTS: &str =
    "요청이 너무 많습니다. 진행 중인 요청이 끝난 뒤 다시 시도해 주세요.";
/// Commands whose answer is remembered after they finished: they change
/// state or run long, so a resent request must not run them again. Any other
/// command only joins a resent request while it still runs.
pub(crate) const REMEMBERED_COMMANDS: &[&str] = &[
    "chat",
    "plan_feedback",
    "plan_approve",
    "ask_response",
    "cancel",
    "compact",
    "conversation_rewind",
    "session_create",
    "session_delete",
    "project_build_run",
    "remote_open_project",
    "remote_close_project",
    "attachment_stage",
    "map_agent_chat",
    "map_agent_cancel",
    "map_agent_session_create",
    "map_agent_candidate_apply",
    "map_agent_apply_undo",
    "map_agent_candidate_discard",
    "map_agent_candidate_revert",
    "map_agent_conversation_reset",
    "map_agent_conversation_rewind",
];
/// Attachments staged at once; more wait for none and are refused.
const CONCURRENT_UPLOADS: usize = 2;
/// The largest message a client may send on the event socket (attach,
/// detach, ping are a few bytes).
const SOCKET_MESSAGE_LIMIT: usize = 64 * 1024;
/// What the remote page may load: its own scripts, styles and images (blob
/// and data images are rendered maps and attachment previews; inline styles
/// are the UI library's), its own API and event socket. Nothing frames it.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; connect-src 'self' wss:; img-src 'self' blob: data:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; worker-src 'self'; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'";
const FUNNEL_REFUSED: &str = "공개 인터넷(Tailscale Funnel)으로 들어온 요청은 받지 않습니다. PC에서 이 포트의 Funnel을 끄고 테일넷 안에서 접속해 주세요.";
const IDENTITY_REFUSED: &str = "이 기기를 연결한 Tailscale 계정과 다른 계정에서 보낸 요청입니다. 이 기기의 Tailscale 계정으로 로그인하거나, PC의 설정 → 원격 접속에서 다시 연결해 주세요.";
const UPLOADS_BUSY: &str = "다른 첨부 파일을 올리는 중입니다. 올리기가 끝난 뒤 다시 시도해 주세요.";
/// A server heartbeat keeps proxies from closing an idle socket and notices
/// a revoked device or a project closed on the desktop.
const HEARTBEAT: Duration = Duration::from_secs(25);
/// Command arguments are JSON; a long conversation's panel log is the largest.
const INVOKE_BODY_LIMIT: usize = 32 * 1024 * 1024;
/// The largest attachment the desktop accepts, plus slack.
const ATTACHMENT_BODY_LIMIT: usize = crate::attachment::MAX_AUDIO_BYTES + 64 * 1024;

/// What the API needs from the app: connection bindings, the event hub, and
/// the command dispatcher. Absent until the app finished setup.
#[derive(Clone)]
pub struct RemoteBackend {
    pub connections: Arc<RemoteConnections>,
    pub hub: RemoteHub,
    pub dispatcher: Arc<dyn RemoteDispatcher>,
}

#[derive(Clone)]
struct AppState {
    auth: Arc<RemoteAuth>,
    assets: AssetLookup,
    backend: Option<RemoteBackend>,
    requests: Arc<RequestLedger>,
    /// Turns true when this listener stops: its event sockets close.
    stopping: watch::Receiver<bool>,
    /// `tailscale serve` exposes this port through Funnel: only the PC
    /// itself is served.
    funnel: Arc<AtomicBool>,
    uploads: Arc<tokio::sync::Semaphore>,
}

/// A command's answer, `None` until it finished.
type Answer = watch::Receiver<Option<Result<Value, String>>>;

struct LedgerEntry {
    key: String,
    device: String,
    answer: Answer,
    /// When the command finished; `None` while it runs.
    finished: Option<tokio::time::Instant>,
    /// The remembered answer's size.
    bytes: usize,
}

/// Commands by device, command and request id. A browser resends a POST
/// whose kept-alive connection closed before the answer (Chromium does, for
/// any method), and the phone itself resends a long turn whose answer it
/// lost; a chat turn held open for minutes is exactly such a request. The
/// resent request carries the same id and joins the running command, or
/// gets its answer once it finished, instead of starting a second turn.
///
/// A running command is never evicted. A finished [`REMEMBERED_COMMANDS`]
/// answer is kept for [`REQUEST_MEMORY`] after it finished, within the
/// per-device, overall and byte caps (oldest finished answers go first);
/// any other command is forgotten once it finished. A new command that
/// finds a cap full of running commands is refused, never run unremembered.
#[derive(Default)]
struct RequestLedger {
    entries: std::sync::Mutex<Vec<LedgerEntry>>,
}

impl RequestLedger {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<LedgerEntry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// The answer of `command` request `id` from `device`, started with
    /// `start` unless it already ran or runs.
    async fn join(
        self: &Arc<Self>,
        device: &str,
        command: &str,
        id: &str,
        start: impl FnOnce() -> Result<dispatch::DispatchFuture, String>,
    ) -> Result<Value, String> {
        let key = format!("{device}:{command}:{id}");
        let remembered = REMEMBERED_COMMANDS.contains(&command);
        let mut answer = {
            let mut entries = self.lock();
            let now = tokio::time::Instant::now();
            entries.retain(|entry| {
                entry
                    .finished
                    .map_or(true, |at| now.duration_since(at) < REQUEST_MEMORY)
            });
            match entries.iter().find(|entry| entry.key == key) {
                Some(entry) => entry.answer.clone(),
                None => {
                    if !make_room(
                        &mut entries,
                        |entry| entry.device == device,
                        DEVICE_REQUESTS,
                    ) || !make_room(&mut entries, |_| true, LEDGER_REQUESTS)
                    {
                        return Err(TOO_MANY_REQUESTS.to_string());
                    }
                    let command = start()?;
                    let (sender, answer) = watch::channel(None);
                    entries.push(LedgerEntry {
                        key: key.clone(),
                        device: device.to_string(),
                        answer: answer.clone(),
                        finished: None,
                        bytes: 0,
                    });
                    // Its own task, so it runs even if this request goes away.
                    let ledger = self.clone();
                    tokio::spawn(async move {
                        let result = run_detached(command).await;
                        ledger.finish(&key, remembered, &result);
                        sender.send_replace(Some(result));
                    });
                    answer
                }
            }
        };
        let answered = match answer.wait_for(Option::is_some).await {
            Ok(result) => result.clone(),
            Err(_) => None,
        };
        answered.unwrap_or_else(|| Err(LOST_ANSWER.to_string()))
    }

    /// Command `key` finished: remember its answer, or forget it.
    fn finish(&self, key: &str, remembered: bool, result: &Result<Value, String>) {
        let mut entries = self.lock();
        let Some(index) = entries.iter().position(|entry| entry.key == key) else {
            return;
        };
        if !remembered {
            entries.remove(index);
            return;
        }
        entries[index].finished = Some(tokio::time::Instant::now());
        entries[index].bytes = match result {
            Ok(value) => serde_json::to_vec(value).map_or(0, |bytes| bytes.len()),
            Err(error) => error.len(),
        };
        while entries.iter().map(|entry| entry.bytes).sum::<usize>() > LEDGER_BYTES {
            let Some(oldest) = entries.iter().position(|entry| entry.finished.is_some()) else {
                break;
            };
            entries.remove(oldest);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().len()
    }
}

/// Make room for one more entry among those `counted` selects, at most
/// `limit`: the oldest finished one goes. `false` when all are running.
fn make_room(
    entries: &mut Vec<LedgerEntry>,
    counted: impl Fn(&LedgerEntry) -> bool,
    limit: usize,
) -> bool {
    if entries.iter().filter(|entry| counted(entry)).count() < limit {
        return true;
    }
    match entries
        .iter()
        .position(|entry| counted(entry) && entry.finished.is_some())
    {
        Some(oldest) => {
            entries.remove(oldest);
            true
        }
        None => false,
    }
}

const LOST_ANSWER: &str = "요청을 처리하지 못했습니다. 대화를 다시 불러와 결과를 확인해 주세요.";

/// A request id: 1..=64 ASCII letters, digits and `-`.
fn request_id(headers: &HeaderMap) -> Option<&str> {
    let id = headers.get(REQUEST_HEADER)?.to_str().ok()?;
    (!id.is_empty()
        && id.len() <= 64
        && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-'))
    .then_some(id)
}

/// What one listener's handlers share with the server that runs it.
#[derive(Clone)]
pub struct ListenerControl {
    /// Turns true when the listener stops.
    pub stopping: watch::Receiver<bool>,
    /// `tailscale serve` exposes the port through Funnel.
    pub funnel: Arc<AtomicBool>,
}

pub fn router(
    auth: Arc<RemoteAuth>,
    assets: AssetLookup,
    backend: Option<RemoteBackend>,
    control: ListenerControl,
) -> Router {
    let state = AppState {
        auth,
        assets,
        backend,
        requests: Arc::default(),
        stopping: control.stopping,
        funnel: control.funnel,
        uploads: Arc::new(tokio::sync::Semaphore::new(CONCURRENT_UPLOADS)),
    };
    Router::new()
        .route("/remote", get(|| async { Redirect::temporary("/remote/") }))
        .route("/remote/", get(remote_page))
        .route("/remote/api/pair", post(pair))
        .route("/remote/api/me", get(me))
        .route("/remote/api/logout", post(logout))
        .route(
            "/remote/api/invoke/{command}",
            any(invoke).layer(DefaultBodyLimit::max(INVOKE_BODY_LIMIT)),
        )
        .route(
            "/remote/api/attachment_stage",
            any(attachment_stage).layer(DefaultBodyLimit::max(ATTACHMENT_BODY_LIMIT)),
        )
        .route("/remote/api/events", any(events))
        .route("/remote/api", any(unknown_api))
        .route("/remote/api/{*rest}", any(unknown_api))
        .route("/remote/{*path}", get(static_file))
        .layer(axum::middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

/// A 401 naming why (`unpaired`, `expired`, `identity`), so the phone can
/// tell a device that must pair again from one that never paired.
fn refused_device(code: &str, message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": message, "code": code })),
    )
        .into_response()
}

fn unauthorized() -> Response {
    refused_device("unpaired", auth::UNPAIRED_MESSAGE)
}

/// Host allowlist, Funnel and cross-origin refusal for every request, and
/// the security headers on every answer.
async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let api = request.uri().path().starts_with("/remote/api");
    let mut response = match refusal(&state, &request) {
        Some(refused) => refused,
        None => next.run(request).await,
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// Why `request` is refused before any handler runs, if it is.
fn refusal(state: &AppState, request: &Request) -> Option<Response> {
    let headers = request.headers();
    if !auth::host_allowed(headers) {
        return Some(error_response(
            StatusCode::MISDIRECTED_REQUEST,
            "허용되지 않은 주소입니다.",
        ));
    }
    // Funnel publishes the port to the whole internet: nothing but the PC
    // itself is served while it does.
    if auth::is_funnel_request(headers)
        || (state.funnel.load(Ordering::SeqCst) && !auth::is_local_request(headers))
    {
        return Some(error_response(StatusCode::FORBIDDEN, FUNNEL_REFUSED));
    }
    if !auth::origin_allowed(request.method(), headers) {
        return Some(error_response(
            StatusCode::FORBIDDEN,
            "다른 사이트에서 보낸 요청은 받을 수 없습니다.",
        ));
    }
    None
}

/// The paired device a request comes from, or the 401 refusing it: no or
/// an unknown token, one unused for 30 days, or (through `tailscale serve`)
/// a tailnet login other than the one that paired it.
// The refusal is the handler's own answer, returned as is.
#[allow(clippy::result_large_err)]
fn authenticated(state: &AppState, headers: &HeaderMap) -> Result<DeviceView, Response> {
    let token = auth::token_from_headers(headers).ok_or_else(unauthorized)?;
    let device = state.auth.check(&token).map_err(|refusal| match refusal {
        auth::AuthRefusal::Unknown => unauthorized(),
        auth::AuthRefusal::Expired => refused_device("expired", refusal.message()),
    })?;
    if !auth::identity_allowed(headers, &device) {
        return Err(refused_device("identity", IDENTITY_REFUSED));
    }
    Ok(device)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairRequest {
    code: String,
    #[serde(default)]
    device_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceResponse {
    device_id: String,
    name: String,
}

impl From<DeviceView> for DeviceResponse {
    fn from(device: DeviceView) -> Self {
        Self {
            device_id: device.id,
            name: device.name,
        }
    }
}

async fn pair(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PairRequest>,
) -> Response {
    // Through `tailscale serve` the device is bound to the tailnet login
    // that pairs it; serve names it for every tailnet user's request.
    let owner = auth::tailnet_login(&headers);
    if owner.is_none() && !auth::is_local_request(&headers) {
        return error_response(
            StatusCode::UNAUTHORIZED,
            &auth::PairError::MissingIdentity.message(),
        );
    }
    let auth = state.auth.clone();
    let result = tokio::task::spawn_blocking(move || {
        auth.pair_as(&request.code, &request.device_name, owner)
    })
    .await;
    match result {
        Ok(Ok(issued)) => {
            let cookie = HeaderValue::from_str(&auth::session_cookie(&issued.token))
                .expect("base64url token is a valid header value");
            (
                [(header::SET_COOKIE, cookie)],
                Json(DeviceResponse::from(issued.device)),
            )
                .into_response()
        }
        Ok(Err(error)) => error_response(StatusCode::UNAUTHORIZED, &error.message()),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("연결 처리에 실패했습니다: {error}. 다시 시도해 주세요."),
        ),
    }
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match authenticated(&state, &headers) {
        Ok(device) => Json(DeviceResponse::from(device)).into_response(),
        Err(refused) => refused,
    }
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let device = match authenticated(&state, &headers) {
        Ok(device) => device,
        Err(refused) => return refused,
    };
    let auth = state.auth.clone();
    match tokio::task::spawn_blocking(move || auth.revoke(&device.id)).await {
        Ok(Ok(_)) => (
            [(
                header::SET_COOKIE,
                HeaderValue::from_str(&auth::cleared_cookie()).expect("static cookie"),
            )],
            Json(serde_json::json!({ "ok": true })),
        )
            .into_response(),
        Ok(Err(error)) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("기기 연결을 해제하지 못했습니다: {error}. PC의 설정에서 해제해 주세요."),
        ),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("기기 연결을 해제하지 못했습니다: {error}. PC의 설정에서 해제해 주세요."),
        ),
    }
}

async fn unknown_api(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(refused) = authenticated(&state, &headers) {
        return refused;
    }
    error_response(StatusCode::NOT_FOUND, "지원하지 않는 요청입니다.")
}

fn invoke_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "ok": false, "code": code, "error": message })),
    )
        .into_response()
}

fn unavailable() -> Response {
    invoke_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "PC 앱이 아직 준비되지 않았습니다. 잠시 후 다시 시도해 주세요.",
    )
}

fn method_not_allowed() -> Response {
    error_response(StatusCode::METHOD_NOT_ALLOWED, "지원하지 않는 요청입니다.")
}

fn no_connection() -> Response {
    invoke_error(StatusCode::CONFLICT, "no_connection", UNKNOWN_CONNECTION)
}

/// The event socket a request names, when it is this device's.
fn request_caller(
    backend: &RemoteBackend,
    headers: &HeaderMap,
    device: &DeviceView,
) -> Option<RemoteCaller> {
    let conn = headers.get(CONNECTION_HEADER)?.to_str().ok()?;
    backend.connections.caller(conn, &device.id)
}

fn invoke_result(result: Result<Value, String>) -> Response {
    Json(match result {
        Ok(result) => json!({ "ok": true, "result": result }),
        Err(error) => json!({ "ok": false, "error": error }),
    })
    .into_response()
}

/// Run a dispatched command on its own task. hyper drops a handler's future
/// when the client goes away; a chat turn held open for minutes must outlive
/// a phone that locked its screen, exactly as a desktop invoke runs to the
/// end whether or not its window still waits for the answer.
async fn run_detached(command: dispatch::DispatchFuture) -> Result<Value, String> {
    tokio::spawn(command)
        .await
        .unwrap_or_else(|error| Err(format!("요청을 처리하지 못했습니다: {error}")))
}

/// The bytes of a binary command, or its error as JSON.
fn binary_result(result: Result<Vec<u8>, String>) -> Response {
    match result {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
        Err(error) => invoke_result(Err(error)),
    }
}

/// `POST /remote/api/invoke/<command>`: run one allowlisted command. A
/// command that fails answers `{ok: false, error}` with the text the desktop
/// invoke rejects with.
async fn invoke(
    State(state): State<AppState>,
    Path(command): Path<String>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let device = match authenticated(&state, &headers) {
        Ok(device) => device,
        Err(refused) => return refused,
    };
    if method != Method::POST {
        return method_not_allowed();
    }
    if !dispatch::is_allowed(&command) {
        return invoke_error(StatusCode::FORBIDDEN, "not_allowed", dispatch::NOT_ALLOWED);
    }
    let Some(backend) = state.backend.clone() else {
        return unavailable();
    };
    let args = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(args @ Value::Object(_)) => args,
            _ => {
                return invoke_error(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "요청 인자가 올바르지 않습니다.",
                )
            }
        }
    };
    match command.as_str() {
        "remote_projects" => return invoke_result(project_list(&backend).await),
        "remote_recent_list" => return invoke_result(recent_list(&backend).await),
        _ => {}
    }
    let Some(caller) = request_caller(&backend, &headers, &device) else {
        return no_connection();
    };
    if dispatch::is_binary(&command) {
        let attached = backend.connections.attached(&caller.conn_id);
        return binary_result(
            backend
                .dispatcher
                .invoke_binary(RemoteCall {
                    caller,
                    attached,
                    command,
                    args,
                })
                .await,
        );
    }
    let id = request_id(&headers).map(str::to_string);
    let result = match command.as_str() {
        // The dispatcher opens (or finds) the project; the connection then
        // attaches to it.
        "remote_open_project" => {
            let conn = caller.conn_id.clone();
            let call = remote_call(&backend, caller, command, args);
            match run_command(&state, &backend, &device.id, id.as_deref(), call).await {
                Ok(opened) => match opened.get("projectId").and_then(Value::as_str) {
                    Some(project) => backend
                        .connections
                        .attach(&conn, project)
                        .map(|project| json!(project)),
                    None => Err(PROJECT_NOT_OPEN.to_string()),
                },
                Err(error) => Err(error),
            }
        }
        // Closed or not, this connection no longer works on the project. A
        // refusal names its kind (`code`), so the phone offers to stop the
        // work only where stopping it would let the close through.
        "remote_close_project" => {
            let conn = caller.conn_id.clone();
            let project = args
                .get("projectId")
                .and_then(Value::as_str)
                .map(crate::project_id::ProjectId::from_stored);
            let call = remote_call(&backend, caller, command, args);
            let result = run_command(&state, &backend, &device.id, id.as_deref(), call).await;
            match (&result, &project) {
                (Ok(_), Some(project)) => backend.connections.detach_from(&conn, project),
                (Err(error), _) => {
                    return Json(json!({
                        "ok": false,
                        "code": crate::engine::close_refusal_code(error),
                        "error": error,
                    }))
                    .into_response()
                }
                _ => {}
            }
            result
        }
        "remote_attach" => match args.get("projectId").and_then(Value::as_str) {
            Some(project) => backend
                .connections
                .attach(&caller.conn_id, project)
                .map(|project| json!(project)),
            None => Err("요청 인자가 올바르지 않습니다: projectId".to_string()),
        },
        "remote_detach" => {
            backend.connections.detach(&caller.conn_id);
            Ok(Value::Null)
        }
        _ => {
            let call = remote_call(&backend, caller, command, args);
            run_command(&state, &backend, &device.id, id.as_deref(), call).await
        }
    };
    invoke_result(result)
}

/// `caller`'s invocation of `command`, with the project it is attached to.
fn remote_call(
    backend: &RemoteBackend,
    caller: RemoteCaller,
    command: String,
    args: Value,
) -> RemoteCall {
    RemoteCall {
        attached: backend.connections.attached(&caller.conn_id),
        caller,
        command,
        args,
    }
}

/// Dispatch `call` on its own task; a request that names an id goes through
/// the request ledger, so a resent request joins it instead of running twice.
async fn run_command(
    state: &AppState,
    backend: &RemoteBackend,
    device: &str,
    id: Option<&str>,
    call: RemoteCall,
) -> Result<Value, String> {
    let dispatcher = backend.dispatcher.clone();
    match id {
        Some(id) => {
            let command = call.command.clone();
            state
                .requests
                .join(device, &command, id, move || Ok(dispatcher.invoke(call)))
                .await
        }
        None => run_detached(dispatcher.invoke(call)).await,
    }
}

/// The open and recent projects for the phone's project screen.
async fn project_list(backend: &RemoteBackend) -> Result<Value, String> {
    let connections = backend.connections.clone();
    tokio::task::spawn_blocking(move || {
        let registry = connections.registry();
        let recents = crate::setup::recent_projects_payload(registry.dirs())?;
        Ok(json!(super::caller::project_entries(
            registry,
            &recents,
            |path| crate::native_runtime::project_id_at(std::path::Path::new(path)),
        )))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The recent projects, as the desktop launcher lists them.
async fn recent_list(backend: &RemoteBackend) -> Result<Value, String> {
    let dirs = backend.connections.registry().dirs().clone();
    tokio::task::spawn_blocking(move || {
        crate::setup::recent_projects_payload(&dirs).map(|r| json!(r))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// `POST /remote/api/attachment_stage`: the raw file body with the headers
/// the desktop's `attachment_stage` invoke sends.
async fn attachment_stage(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let device = match authenticated(&state, &headers) {
        Ok(device) => device,
        Err(refused) => return refused,
    };
    if method != Method::POST {
        return method_not_allowed();
    }
    let Some(backend) = state.backend.clone() else {
        return unavailable();
    };
    if request_caller(&backend, &headers, &device).is_none() {
        return no_connection();
    }
    let id = request_id(&headers).map(str::to_string);
    let dispatcher = backend.dispatcher.clone();
    let uploads = state.uploads.clone();
    // A few uploads at a time; more are refused rather than buffered.
    let start = move || {
        let permit = uploads
            .try_acquire_owned()
            .map_err(|_| UPLOADS_BUSY.to_string())?;
        let staging = dispatcher.stage_attachment(headers, body);
        Ok(Box::pin(async move {
            let staged = staging.await;
            drop(permit);
            staged
        }) as dispatch::DispatchFuture)
    };
    invoke_result(match id {
        Some(id) => {
            state
                .requests
                .join(&device.id, "attachment_stage", &id, start)
                .await
        }
        None => match start() {
            Ok(staging) => run_detached(staging).await,
            Err(error) => Err(error),
        },
    })
}

/// `GET /remote/api/events`: the event socket.
async fn events(State(state): State<AppState>, request: Request) -> Response {
    let device = match authenticated(&state, request.headers()) {
        Ok(device) => device,
        Err(refused) => return refused,
    };
    let Some(backend) = state.backend.clone() else {
        return unavailable();
    };
    let (mut parts, _) = request.into_parts();
    let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &state).await {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    let auth = state.auth.clone();
    let stopping = state.stopping.clone();
    upgrade
        .max_message_size(SOCKET_MESSAGE_LIMIT)
        .max_frame_size(SOCKET_MESSAGE_LIMIT)
        .on_upgrade(move |socket| run_socket(socket, backend, auth, stopping, device.id))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum SocketCommand {
    #[serde(rename_all = "camelCase")]
    Attach {
        project_id: String,
    },
    Detach,
    Ping,
}

async fn send_json(socket: &mut WebSocket, value: Value) -> bool {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .is_ok()
}

/// The reply to one client message on socket `conn`.
fn answer(backend: &RemoteBackend, conn: &str, text: &str) -> Value {
    match serde_json::from_str::<SocketCommand>(text) {
        Ok(SocketCommand::Attach { project_id }) => {
            match backend.connections.attach(conn, &project_id) {
                Ok(project) => json!({ "type": "attached", "project": project }),
                Err(error) => {
                    json!({ "type": "attach_failed", "projectId": project_id, "error": error })
                }
            }
        }
        Ok(SocketCommand::Detach) => {
            backend.connections.detach(conn);
            json!({ "type": "detached" })
        }
        Ok(SocketCommand::Ping) => json!({ "type": "pong" }),
        Err(_) => json!({ "type": "error", "error": "알 수 없는 메시지입니다." }),
    }
}

/// Close `socket` from the server side.
async fn close_socket(socket: &mut WebSocket) {
    let _ = socket.send(Message::Close(None)).await;
}

/// One event socket: announce its connection id, answer attach/detach/ping,
/// and forward the hub's events for its project until either side closes.
/// It closes at once when the listener stops (remote access turned off or
/// restarted) or its device is removed; a project closing on the desktop is
/// announced at once as `detached`. The binding goes with the socket.
async fn run_socket(
    mut socket: WebSocket,
    backend: RemoteBackend,
    auth: Arc<RemoteAuth>,
    mut stopping: watch::Receiver<bool>,
    device_id: String,
) {
    let mut events = backend.hub.subscribe();
    let mut auth_events = auth.subscribe();
    let mut closed = backend.connections.registry().subscribe_closed();
    let conn = backend.connections.open(&device_id).conn_id;
    let mut heartbeat =
        tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
    // Whether the socket was attached when it last looked, to tell the client
    // once when the desktop closes its project.
    let mut was_attached = false;
    // Stopped or revoked before this socket subscribed: it never opens.
    let mut open = !*stopping.borrow() && auth.is_active(&device_id);
    if open {
        open = send_json(&mut socket, json!({ "type": "hello", "connId": conn })).await;
    } else {
        close_socket(&mut socket).await;
    }
    while open {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let reply = answer(&backend, &conn, text.as_str());
                    was_attached = backend.connections.attached(&conn).is_some();
                    open = send_json(&mut socket, reply).await;
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => open = false,
                Some(Ok(_)) => {}
            },
            event = events.recv() => match event {
                Ok(event) => {
                    let attached = backend.connections.attached(&conn);
                    if was_attached && attached.is_none() {
                        was_attached = false;
                        open = send_json(
                            &mut socket,
                            json!({ "type": "detached", "error": PROJECT_CLOSED }),
                        )
                        .await;
                    }
                    if open && event.is_for(attached.as_ref()) {
                        open = send_json(
                            &mut socket,
                            json!({ "type": "event", "event": event.event, "payload": event.payload }),
                        )
                        .await;
                    }
                }
                // Events were dropped: the client reloads its session state.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    open = send_json(&mut socket, json!({ "type": "resync" })).await;
                }
                Err(broadcast::error::RecvError::Closed) => open = false,
            },
            // Remote access off, or the listener restarting.
            _ = stopping.changed() => {
                close_socket(&mut socket).await;
                open = false;
            }
            revoked = auth_events.recv() => {
                let gone = match revoked {
                    Ok(super::auth::AuthEvent::Revoked(id)) => id == device_id,
                    Ok(_) => false,
                    Err(broadcast::error::RecvError::Lagged(_)) => !auth.is_active(&device_id),
                    Err(broadcast::error::RecvError::Closed) => true,
                };
                if gone {
                    close_socket(&mut socket).await;
                    open = false;
                }
            }
            project = closed.recv() => {
                if matches!(project, Err(broadcast::error::RecvError::Closed)) {
                    open = false;
                } else if was_attached && backend.connections.attached(&conn).is_none() {
                    was_attached = false;
                    open = send_json(
                        &mut socket,
                        json!({ "type": "detached", "error": PROJECT_CLOSED }),
                    )
                    .await;
                }
            }
            _ = heartbeat.tick() => {
                if !auth.is_active(&device_id) {
                    // Revoked or expired: its socket goes too.
                    close_socket(&mut socket).await;
                    open = false;
                } else if was_attached && backend.connections.attached(&conn).is_none() {
                    was_attached = false;
                    open = send_json(
                        &mut socket,
                        json!({ "type": "detached", "error": PROJECT_CLOSED }),
                    )
                    .await;
                } else {
                    open = socket.send(Message::Ping(Bytes::new())).await.is_ok();
                }
            }
        }
    }
    backend.connections.close(&conn);
}

async fn remote_page(State(state): State<AppState>) -> Response {
    serve_asset(&state, REMOTE_PAGE)
}

async fn static_file(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    if !safe_asset_path(&path) || !remote_asset(&path) {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve_asset(&state, &path)
}

/// The files the remote page needs: the page, its PWA files and icons, and
/// the built bundle. The desktop's own pages (`index.html`, the Map and
/// import windows) and public files are never served.
fn remote_asset(path: &str) -> bool {
    matches!(path, REMOTE_PAGE | "remote.webmanifest" | "sw.js")
        || (path.starts_with("remote-") && path.ends_with(".png") && !path.contains('/'))
        || (path.starts_with("assets/") && !path.ends_with(".html"))
}

/// Relative `segment/segment.ext` paths only: no traversal, no hidden files,
/// no drive or separator tricks.
fn safe_asset_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && !segment.starts_with('.')
                && segment
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
        })
}

fn serve_asset(state: &AppState, path: &str) -> Response {
    let Some(asset) = (state.assets)(path) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "원격 화면 파일을 찾을 수 없습니다. 앱을 다시 설치하거나 패널을 다시 빌드해 주세요.",
        );
    };
    // Tauri's resolver knows no `.webmanifest` type and labels it text/html,
    // which is also how its index.html fallback looks; only a JSON body is
    // the manifest itself.
    let mime_type = if path.ends_with(".webmanifest") {
        if serde_json::from_slice::<Value>(&asset.bytes).is_err() {
            return StatusCode::NOT_FOUND.into_response();
        }
        "application/manifest+json".to_string()
    } else {
        asset.mime_type
    };
    // The frontend resolver falls back to index.html for unknown paths; a
    // script or style request must not receive the desktop page instead.
    if mime_type.starts_with("text/html") && !path.ends_with(".html") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let content_type = HeaderValue::from_str(&mime_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        asset.bytes,
    )
        .into_response()
}

struct Running {
    port: u16,
    shutdown: oneshot::Sender<()>,
    /// Tells this listener's event sockets to close.
    stopping: watch::Sender<bool>,
    task: JoinHandle<()>,
}

#[derive(Default)]
struct Slot {
    running: Option<Running>,
    error: Option<String>,
}

/// Listener state reported to the settings screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerState {
    pub listening: bool,
    pub error: Option<String>,
}

/// The process-wide remote server: at most one listener, restarted when
/// settings change. A failed bind is recorded, never fatal.
pub struct RemoteServer {
    dirs: crate::config::DataDirs,
    auth: Arc<RemoteAuth>,
    assets: AssetLookup,
    backend: OnceLock<RemoteBackend>,
    slot: tokio::sync::Mutex<Slot>,
    /// Whether remote access is on, as last applied: the process stays
    /// alive after its last window while it is.
    enabled: AtomicBool,
    /// `tailscale serve` exposes the port through Funnel, as last probed.
    funnel: Arc<AtomicBool>,
}

impl RemoteServer {
    pub fn new(dirs: crate::config::DataDirs, assets: AssetLookup) -> Arc<Self> {
        let auth = Arc::new(RemoteAuth::load(dirs.clone()));
        Arc::new(Self {
            dirs,
            auth,
            assets,
            backend: OnceLock::new(),
            slot: tokio::sync::Mutex::new(Slot::default()),
            enabled: AtomicBool::new(false),
            funnel: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn dirs(&self) -> &crate::config::DataDirs {
        &self.dirs
    }

    pub fn auth(&self) -> &Arc<RemoteAuth> {
        &self.auth
    }

    /// Connect the API to the app once its services exist. A listener
    /// started before answers commands and events with 503.
    pub fn set_backend(&self, backend: RemoteBackend) {
        if self.backend.set(backend).is_err() {
            eprintln!("eud-agent: remote backend was already set");
        }
    }

    /// Record whether `tailscale serve` exposes the port through Funnel.
    /// While it does, only requests from the PC itself are served.
    pub fn set_funnel(&self, funnel: bool) {
        self.funnel.store(funnel, Ordering::SeqCst);
    }

    /// Start or stop according to the saved settings.
    pub async fn start_from_config(&self) {
        match self.dirs.load_config() {
            Ok(config) => self.apply(config.remote.enabled, config.remote.port).await,
            Err(error) => eprintln!("eud-agent: remote server not started: {error}"),
        }
    }

    /// Stop any listener (closing its event sockets), then bind
    /// `127.0.0.1:<port>` when enabled. Port 0 binds any free port.
    pub async fn apply(&self, enabled: bool, port: u16) {
        let mut slot = self.slot.lock().await;
        self.enabled.store(enabled, Ordering::SeqCst);
        if let Some(running) = slot.running.take() {
            stop(running).await;
        }
        slot.error = None;
        if !enabled {
            return;
        }
        match bind_exclusive(port) {
            Ok(listener) => {
                let port = listener.local_addr().map_or(port, |addr| addr.port());
                let (stopping, control) = self.control();
                slot.running = Some(serve(
                    listener,
                    port,
                    router(
                        self.auth.clone(),
                        self.assets.clone(),
                        self.backend.get().cloned(),
                        control,
                    ),
                    stopping,
                ));
            }
            Err(error) => {
                slot.error = Some(format!(
                    "포트 {port}을(를) 열 수 없습니다({error}). 다른 프로그램이 쓰고 있다면 다른 포트를 지정해 주세요."
                ));
            }
        }
    }

    fn control(&self) -> (watch::Sender<bool>, ListenerControl) {
        let (stopping, receiver) = watch::channel(false);
        (
            stopping,
            ListenerControl {
                stopping: receiver,
                funnel: self.funnel.clone(),
            },
        )
    }

    /// Whether remote access is on (even when its port could not be bound).
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub async fn listener_state(&self) -> ListenerState {
        let slot = self.slot.lock().await;
        let listening = slot
            .running
            .as_ref()
            .is_some_and(|running| !running.task.is_finished());
        ListenerState {
            listening,
            error: slot.error.clone(),
        }
    }

    /// The port actually bound, when listening.
    pub async fn bound_port(&self) -> Option<u16> {
        self.slot
            .lock()
            .await
            .running
            .as_ref()
            .map(|running| running.port)
    }
}

/// Bind `127.0.0.1:<port>` so that no other process can bind it too. On
/// Windows a plain bind lets another process take over the port with
/// `SO_REUSEADDR`; `SO_EXCLUSIVEADDRUSE` refuses that.
fn bind_exclusive(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    let socket = tokio::net::TcpSocket::new_v4()?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{
            setsockopt, SOCKET, SOCKET_ERROR, SOL_SOCKET, SO_EXCLUSIVEADDRUSE,
        };
        let enable: i32 = 1;
        // SAFETY: the socket is open and owned by `socket`; the option value
        // is a live i32 whose size is passed.
        let result = unsafe {
            setsockopt(
                socket.as_raw_socket() as SOCKET,
                SOL_SOCKET,
                SO_EXCLUSIVEADDRUSE,
                (&enable as *const i32).cast(),
                std::mem::size_of::<i32>() as i32,
            )
        };
        if result == SOCKET_ERROR {
            return Err(std::io::Error::last_os_error());
        }
    }
    socket.bind(std::net::SocketAddr::from(([127, 0, 0, 1], port)))?;
    socket.listen(1024)
}

fn serve(
    listener: tokio::net::TcpListener,
    port: u16,
    app: Router,
    stopping: watch::Sender<bool>,
) -> Running {
    let (shutdown, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = stopped.await;
        });
        if let Err(error) = server.await {
            eprintln!("eud-agent: remote server stopped: {error}");
        }
    });
    Running {
        port,
        shutdown,
        stopping,
        task,
    }
}

async fn stop(running: Running) {
    // Upgraded sockets outlive axum's graceful shutdown; they close on this.
    running.stopping.send_replace(true);
    let _ = running.shutdown.send(());
    let mut task = running.task;
    if tokio::time::timeout(SHUTDOWN_GRACE, &mut task)
        .await
        .is_err()
    {
        task.abort();
        let _ = task.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DataDirs;

    fn temp_dirs(label: &str) -> DataDirs {
        let base = std::env::temp_dir().join(format!(
            "eud-remote-server-{label}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        DataDirs::from_bases(&base.join("roaming"), &base.join("local"))
    }

    fn fake_assets() -> AssetLookup {
        Arc::new(|path: &str| match path {
            "remote.html" => Some(StaticAsset {
                bytes: b"<!doctype html><title>remote</title>".to_vec(),
                mime_type: "text/html".to_string(),
            }),
            "assets/remote.js" => Some(StaticAsset {
                bytes: b"console.log(1)".to_vec(),
                mime_type: "text/javascript".to_string(),
            }),
            // Tauri's resolver labels a manifest text/html.
            "remote.webmanifest" => Some(StaticAsset {
                bytes: br#"{"name":"EUD Agent"}"#.to_vec(),
                mime_type: "text/html".to_string(),
            }),
            "sw.js" => Some(StaticAsset {
                bytes: b"self.addEventListener('fetch', () => {})".to_vec(),
                mime_type: "text/javascript".to_string(),
            }),
            // The real resolver's index.html fallback.
            _ => Some(StaticAsset {
                bytes: b"<!doctype html><title>desktop</title>".to_vec(),
                mime_type: "text/html".to_string(),
            }),
        })
    }

    fn serve_test(
        listener: tokio::net::TcpListener,
        auth: Arc<RemoteAuth>,
        backend: Option<RemoteBackend>,
    ) -> Running {
        let port = listener.local_addr().unwrap().port();
        let (stopping, receiver) = watch::channel(false);
        let control = ListenerControl {
            stopping: receiver,
            funnel: Arc::default(),
        };
        serve(
            listener,
            port,
            router(auth, fake_assets(), backend, control),
            stopping,
        )
    }

    async fn start(auth: Arc<RemoteAuth>) -> (String, Running) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let running = serve_test(listener, auth, None);
        (format!("http://127.0.0.1:{port}"), running)
    }

    fn cookie_token(response: &reqwest::Response) -> String {
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Path=/remote"));
        cookie
            .split(';')
            .next()
            .unwrap()
            .strip_prefix("eud_remote=")
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn pair_then_me_then_logout() {
        let auth = Arc::new(RemoteAuth::load(temp_dirs("flow")));
        let (base, running) = start(auth.clone()).await;
        let client = reqwest::Client::new();

        // Without a token every API route is 401.
        for path in [
            "/remote/api/me",
            "/remote/api/invoke/anything",
            "/remote/api",
        ] {
            let response = client.get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(response.status(), 401, "{path}");
        }
        let response = client
            .post(format!("{base}/remote/api/logout"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);

        // A wrong code is refused.
        let code = auth.begin_pairing().unwrap();
        let response = client
            .post(format!("{base}/remote/api/pair"))
            .json(&serde_json::json!({"code": "WRONGCOD", "deviceName": "폰"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);

        let response = client
            .post(format!("{base}/remote/api/pair"))
            .header("origin", base.clone())
            .json(&serde_json::json!({"code": code.code, "deviceName": "내 폰"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let token = cookie_token(&response);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["name"], "내 폰");
        let device_id = body["deviceId"].as_str().unwrap().to_string();

        // Cookie and bearer both authenticate.
        let response = client
            .get(format!("{base}/remote/api/me"))
            .header("cookie", format!("eud_remote={token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["deviceId"], device_id.as_str());
        let response = client
            .get(format!("{base}/remote/api/me"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);

        // Authenticated but unknown API: 404, not 401.
        let response = client
            .get(format!("{base}/remote/api/nothing"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);

        // A cross-origin state change is refused even with a valid token.
        let response = client
            .post(format!("{base}/remote/api/logout"))
            .bearer_auth(&token)
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);

        let response = client
            .post(format!("{base}/remote/api/logout"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let response = client
            .get(format!("{base}/remote/api/me"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
        assert!(auth.devices().is_empty());

        stop(running).await;
    }

    #[tokio::test]
    async fn static_panel_is_served_without_a_token() {
        let auth = Arc::new(RemoteAuth::load(temp_dirs("static")));
        let (base, running) = start(auth).await;
        let client = reqwest::Client::new();

        let response = client.get(format!("{base}/remote/")).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert!(response.text().await.unwrap().contains("remote"));
        let response = client
            .get(format!("{base}/remote/assets/remote.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/javascript");
        // The PWA files: a manifest typed as one, the worker beside the page
        // (so its default scope is /remote/), neither cached for a year.
        let response = client
            .get(format!("{base}/remote/remote.webmanifest"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/manifest+json"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        let response = client
            .get(format!("{base}/remote/sw.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/javascript");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        // A missing manifest gets the desktop page from the resolver: refused.
        let response = client
            .get(format!("{base}/remote/other.webmanifest"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        // The index.html fallback is never handed out as a script.
        let response = client
            .get(format!("{base}/remote/assets/missing.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        let response = client
            .get(format!("{base}/remote/..%2Fsecret.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        // Outside /remote nothing is served.
        let response = client.get(format!("{base}/")).send().await.unwrap();
        assert_eq!(response.status(), 404);
        // A foreign Host (DNS rebinding) is refused.
        let response = client
            .get(format!("{base}/remote/"))
            .header("host", "evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 421);

        stop(running).await;
    }

    #[tokio::test]
    async fn a_port_in_use_is_reported_not_fatal() {
        let blocker = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = blocker.local_addr().unwrap().port();
        let server = RemoteServer::new(temp_dirs("busy"), fake_assets());
        server.apply(true, port).await;
        let state = server.listener_state().await;
        assert!(!state.listening);
        assert!(state.error.unwrap().contains(&port.to_string()));

        drop(blocker);
        server.apply(true, port).await;
        let state = server.listener_state().await;
        assert!(state.listening);
        assert_eq!(state.error, None);
        assert_eq!(server.bound_port().await, Some(port));

        server.apply(false, port).await;
        assert!(!server.listener_state().await.listening);
        assert_eq!(server.bound_port().await, None);
    }

    #[test]
    fn only_the_remote_page_and_its_files_are_served() {
        for path in [
            "remote.html",
            "remote.webmanifest",
            "sw.js",
            "remote-icon-192.png",
            "remote-apple-touch-icon.png",
            "assets/remote-abc.js",
            "assets/index-abc.css",
        ] {
            assert!(remote_asset(path), "{path}");
        }
        for path in [
            "index.html",
            "map-agent.html",
            "map-import.html",
            "eud-agent.png",
            "assets/x.html",
            "remote-x/../a.png",
        ] {
            assert!(!remote_asset(path), "{path}");
        }
    }

    #[test]
    fn asset_paths_reject_traversal() {
        assert!(safe_asset_path("assets/remote-abc.js"));
        assert!(safe_asset_path("remote.html"));
        assert!(!safe_asset_path("../secret"));
        assert!(!safe_asset_path("assets/../x"));
        assert!(!safe_asset_path(".env"));
        assert!(!safe_asset_path("a//b"));
        assert!(!safe_asset_path("C:\\x"));
        assert!(!safe_asset_path(""));
    }

    /// Answers every command with what it was called with; `cancel` fails.
    struct EchoDispatcher;

    impl RemoteDispatcher for EchoDispatcher {
        fn invoke(&self, call: RemoteCall) -> dispatch::DispatchFuture {
            Box::pin(async move {
                if call.command == "remote_close_project" {
                    return Err(crate::engine::REMOTE_SWITCH_BUSY.to_string());
                }
                if call.command == "cancel" {
                    return Err("중단할 작업이 없습니다.".to_string());
                }
                Ok(json!({
                    "command": call.command,
                    "label": call.caller.label,
                    "attached": call.attached.map(|project| project.into_string()),
                    "args": call.args,
                }))
            })
        }

        fn invoke_binary(&self, call: RemoteCall) -> dispatch::BinaryFuture {
            Box::pin(async move {
                match call
                    .args
                    .get("command")
                    .and_then(|command| command.get("sessionId"))
                {
                    Some(Value::String(session)) if session == "missing" => {
                        Err("이 Map Agent 세션은 다른 프로젝트의 세션입니다.".to_string())
                    }
                    _ => Ok(b"\x89PNG-bytes".to_vec()),
                }
            })
        }

        fn stage_attachment(&self, headers: HeaderMap, body: Bytes) -> dispatch::DispatchFuture {
            let name = headers
                .get("x-eud-file-name-hex")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            Box::pin(async move { Ok(json!({ "size": body.len(), "nameHex": name })) })
        }
    }

    type Socket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn next_json(socket: &mut Socket) -> Value {
        use futures_util::StreamExt;
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .expect("socket message in time")
                .expect("socket open")
                .expect("socket message");
            if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
                return serde_json::from_str(text.as_str()).unwrap();
            }
        }
    }

    async fn send_socket(socket: &mut Socket, value: Value) {
        use futures_util::SinkExt;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                value.to_string().into(),
            ))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn remote_commands_and_events_flow_through_one_connection() {
        use crate::project_id::test_support::{create_project, temp_root};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let temp = temp_root("remote-flow");
        let dirs = DataDirs::from_bases(&temp.join("roaming"), &temp.join("local"));
        let registry = crate::project_registry::ProjectRegistry::new(dirs.clone());
        let mine = registry
            .register(&create_project(&temp.join("mine"), "Mine"))
            .unwrap();
        let other = registry
            .register(&create_project(&temp.join("other"), "Other"))
            .unwrap();
        let hub = RemoteHub::new();
        let backend = RemoteBackend {
            connections: RemoteConnections::new(registry.clone()),
            hub: hub.clone(),
            dispatcher: Arc::new(EchoDispatcher),
        };
        let auth = Arc::new(RemoteAuth::load(dirs));
        let code = auth.begin_pairing().unwrap();
        let token = auth.pair(&code.code, "폰").unwrap().token;

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let running = serve_test(listener, auth, Some(backend));
        let base = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::new();

        // No token: no socket.
        let response = client
            .get(format!("{base}/remote/api/events"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);

        let mut request = format!("ws://127.0.0.1:{port}/remote/api/events")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let hello = next_json(&mut socket).await;
        assert_eq!(hello["type"], "hello");
        let conn = hello["connId"].as_str().unwrap().to_string();

        let invoke = |command: &str, conn: Option<&str>, body: Value| {
            let mut request = client
                .post(format!("{base}/remote/api/invoke/{command}"))
                .bearer_auth(&token)
                .json(&body);
            if let Some(conn) = conn {
                request = request.header(CONNECTION_HEADER, conn);
            }
            request.send()
        };

        // The allowlist comes first, then the connection.
        let response = invoke("setup_pick_project_path", Some(&conn), json!({}))
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["code"], "not_allowed");
        let response = invoke("status", None, json!({})).await.unwrap();
        assert_eq!(response.status(), 409);
        let response = invoke("status", Some("someone-else"), json!({}))
            .await
            .unwrap();
        assert_eq!(response.status(), 409);

        let projects: Value = invoke("remote_projects", None, json!({}))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(projects["ok"], true);
        assert_eq!(projects["result"].as_array().unwrap().len(), 2);

        // Before attaching, the label resolves no project.
        let body: Value = invoke("status", Some(&conn), json!({}))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(body["ok"], true);
        let label = body["result"]["label"].as_str().unwrap().to_string();
        assert!(label.starts_with("remote-"));
        assert!(registry.for_window(&label).is_err());
        assert_eq!(body["result"]["attached"], Value::Null);

        send_socket(&mut socket, json!({"type": "attach", "projectId": "nope"})).await;
        let reply = next_json(&mut socket).await;
        assert_eq!(reply["type"], "attach_failed");
        assert_eq!(reply["error"], PROJECT_NOT_OPEN);

        send_socket(
            &mut socket,
            json!({"type": "attach", "projectId": mine.id().as_str()}),
        )
        .await;
        let reply = next_json(&mut socket).await;
        assert_eq!(reply["type"], "attached");
        assert_eq!(reply["project"]["name"], "Mine");
        assert_eq!(registry.for_window(&label).unwrap().id(), mine.id());

        // Arguments reach the dispatcher as sent; a failure is `ok: false`.
        let body: Value = invoke(
            "chat",
            Some(&conn),
            json!({"sessionId": "s1", "text": "안녕"}),
        )
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
        assert_eq!(body["result"]["command"], "chat");
        assert_eq!(body["result"]["attached"], mine.id().as_str());
        assert_eq!(body["result"]["args"]["text"], "안녕");
        let response = invoke("cancel", Some(&conn), json!({"sessionId": "s1"}))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(
            body,
            json!({"ok": false, "error": "중단할 작업이 없습니다."})
        );

        // A close refusal names its kind for the phone.
        let body: Value = invoke(
            "remote_close_project",
            Some(&conn),
            json!({"projectId": mine.id().as_str()}),
        )
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
        assert_eq!(body["ok"], false);
        assert_eq!(body["code"], "switching");
        assert_eq!(body["error"], crate::engine::REMOTE_SWITCH_BUSY);

        // A binary command answers raw bytes; its failure is JSON as usual.
        let response = invoke(
            "map_agent_render",
            Some(&conn),
            json!({"command": {"sessionId": "m1"}}),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()["content-type"],
            "application/octet-stream"
        );
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"\x89PNG-bytes");
        let body: Value = invoke(
            "map_agent_render",
            Some(&conn),
            json!({"command": {"sessionId": "missing"}}),
        )
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().contains("다른 프로젝트"));
        // A Map command goes through the dispatcher like any other.
        let body: Value = invoke(
            "map_agent_candidate_apply",
            Some(&conn),
            json!({"sessionId": "m1"}),
        )
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
        assert_eq!(body["result"]["command"], "map_agent_candidate_apply");
        assert_eq!(body["result"]["label"], label);
        // Desktop-only Map commands are refused before any connection check.
        let response = invoke("map_agent_properties_save", Some(&conn), json!({}))
            .await
            .unwrap();
        assert_eq!(response.status(), 403);

        // Raw attachment upload with the desktop's headers.
        let body: Value = client
            .post(format!("{base}/remote/api/attachment_stage"))
            .bearer_auth(&token)
            .header(CONNECTION_HEADER, &conn)
            .header("x-eud-file-name-hex", "612e706e67")
            .header("content-type", "image/png")
            .body(vec![7u8; 1000])
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            body["result"],
            json!({"size": 1000, "nameHex": "612e706e67"})
        );

        // Only the attached project's events and process-wide ones arrive.
        hub.publish_project(other.id(), "answer", &json!({"text": "other"}));
        hub.publish_project(mine.id(), "answer", &json!({"text": "mine"}));
        hub.publish_global("session_activity", &json!({"sessionId": "s1"}));
        let event = next_json(&mut socket).await;
        assert_eq!(event["type"], "event");
        assert_eq!(event["event"], "answer");
        assert_eq!(event["payload"]["text"], "mine");
        let event = next_json(&mut socket).await;
        assert_eq!(event["event"], "session_activity");

        // The desktop closes the project: the client is told at once, with
        // no event needed to notice it.
        registry.close(mine.id());
        let reply = next_json(&mut socket).await;
        assert_eq!(reply["type"], "detached");
        assert_eq!(reply["error"], PROJECT_CLOSED);
        hub.publish_global("session_activity", &json!({"sessionId": "s2"}));
        let event = next_json(&mut socket).await;
        assert_eq!(event["payload"]["sessionId"], "s2");

        send_socket(
            &mut socket,
            json!({"type": "attach", "projectId": other.id().as_str()}),
        )
        .await;
        assert_eq!(next_json(&mut socket).await["type"], "attached");
        assert_eq!(registry.window_labels(other.id()), [label.clone()]);

        // The binding goes with the socket.
        socket.close(None).await.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !registry.window_labels(other.id()).is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "binding outlived its socket"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let response = invoke("status", Some(&conn), json!({})).await.unwrap();
        assert_eq!(response.status(), 409);

        stop(running).await;
        std::fs::remove_dir_all(temp).ok();
    }

    /// `chat` takes 400 ms and counts its runs; every other command is instant.
    struct SlowDispatcher {
        finished: Arc<std::sync::atomic::AtomicBool>,
        runs: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl RemoteDispatcher for SlowDispatcher {
        fn invoke(&self, call: RemoteCall) -> dispatch::DispatchFuture {
            let finished = self.finished.clone();
            let runs = self.runs.clone();
            Box::pin(async move {
                if call.command == "chat" {
                    let run = runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    finished.store(true, std::sync::atomic::Ordering::SeqCst);
                    return Ok(json!({ "run": run }));
                }
                Ok(Value::Null)
            })
        }

        fn invoke_binary(&self, _call: RemoteCall) -> dispatch::BinaryFuture {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn stage_attachment(&self, _headers: HeaderMap, _body: Bytes) -> dispatch::DispatchFuture {
            Box::pin(async { Ok(Value::Null) })
        }
    }

    #[tokio::test]
    async fn a_command_runs_to_the_end_when_the_phone_goes_away() {
        use crate::project_id::test_support::temp_root;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let temp = temp_root("remote-detached");
        let dirs = DataDirs::from_bases(&temp.join("roaming"), &temp.join("local"));
        let registry = crate::project_registry::ProjectRegistry::new(dirs.clone());
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let backend = RemoteBackend {
            connections: RemoteConnections::new(registry),
            hub: RemoteHub::new(),
            dispatcher: Arc::new(SlowDispatcher {
                finished: finished.clone(),
                runs: runs.clone(),
            }),
        };
        let auth = Arc::new(RemoteAuth::load(dirs));
        let code = auth.begin_pairing().unwrap();
        let token = auth.pair(&code.code, "폰").unwrap().token;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let running = serve_test(listener, auth, Some(backend));

        let mut request = format!("ws://127.0.0.1:{port}/remote/api/events")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let conn = next_json(&mut socket).await["connId"]
            .as_str()
            .unwrap()
            .to_string();

        // The phone stops waiting (screen locked, network changed) long
        // before the turn ends.
        let gave_up = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/remote/api/invoke/chat"))
            .bearer_auth(&token)
            .header(CONNECTION_HEADER, &conn)
            .json(&json!({"sessionId": "s1"}))
            .timeout(Duration::from_millis(100))
            .send()
            .await;
        assert!(gave_up.is_err());
        assert!(!finished.load(std::sync::atomic::Ordering::SeqCst));
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(finished.load(std::sync::atomic::Ordering::SeqCst));

        // A resent request (same request id) joins the running command and,
        // after it finished, gets its answer: the turn runs once.
        let client = reqwest::Client::new();
        let send = |id: &'static str| {
            client
                .post(format!("http://127.0.0.1:{port}/remote/api/invoke/chat"))
                .bearer_auth(&token)
                .header(CONNECTION_HEADER, &conn)
                .header(REQUEST_HEADER, id)
                .json(&json!({"sessionId": "s1"}))
                .send()
        };
        let runs_before = runs.load(std::sync::atomic::Ordering::SeqCst);
        let (first, resent) = tokio::join!(send("turn-1"), send("turn-1"));
        let first: Value = first.unwrap().json().await.unwrap();
        let resent: Value = resent.unwrap().json().await.unwrap();
        assert_eq!(first, resent);
        let late: Value = send("turn-1").await.unwrap().json().await.unwrap();
        assert_eq!(late, first);
        assert_eq!(
            runs.load(std::sync::atomic::Ordering::SeqCst),
            runs_before + 1
        );
        // Another id is another command.
        let other: Value = send("turn-2").await.unwrap().json().await.unwrap();
        assert_ne!(other, first);
        assert_eq!(
            runs.load(std::sync::atomic::Ordering::SeqCst),
            runs_before + 2
        );
        assert!(request_id(&HeaderMap::new()).is_none());
        let mut bad = HeaderMap::new();
        bad.insert(REQUEST_HEADER, HeaderValue::from_static("a b"));
        assert!(request_id(&bad).is_none());

        drop(socket);
        stop(running).await;
        std::fs::remove_dir_all(temp).ok();
    }

    fn answer_after(
        delay: Duration,
        runs: &Arc<std::sync::atomic::AtomicUsize>,
    ) -> impl FnOnce() -> Result<dispatch::DispatchFuture, String> {
        let runs = runs.clone();
        move || {
            let run = runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            Ok(Box::pin(async move {
                tokio::time::sleep(delay).await;
                Ok(json!(run))
            }) as dispatch::DispatchFuture)
        }
    }

    fn never_answers() -> impl FnOnce() -> Result<dispatch::DispatchFuture, String> {
        || Ok(Box::pin(std::future::pending()) as dispatch::DispatchFuture)
    }

    fn run_count(runs: &Arc<std::sync::atomic::AtomicUsize>) -> usize {
        runs.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[tokio::test(start_paused = true)]
    async fn the_ledger_remembers_an_answer_ten_minutes_after_it_finished() {
        let ledger = Arc::new(RequestLedger::default());
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // A turn longer than the memory itself.
        let long = Duration::from_secs(20 * 60);
        let first = ledger
            .join("d1", "chat", "turn", answer_after(long, &runs))
            .await;
        assert_eq!(first, Ok(json!(1)));
        // Nine minutes after it finished: the resent request gets the answer.
        tokio::time::advance(Duration::from_secs(9 * 60)).await;
        let resent = ledger
            .join("d1", "chat", "turn", answer_after(Duration::ZERO, &runs))
            .await;
        assert_eq!(resent, Ok(json!(1)));
        assert_eq!(run_count(&runs), 1);
        // The id is scoped by device and command.
        let other_device = ledger
            .join("d2", "chat", "turn", answer_after(Duration::ZERO, &runs))
            .await;
        assert_eq!(other_device, Ok(json!(2)));
        let other_command = ledger
            .join(
                "d1",
                "plan_approve",
                "turn",
                answer_after(Duration::ZERO, &runs),
            )
            .await;
        assert_eq!(other_command, Ok(json!(3)));
        // Past ten minutes it is forgotten.
        tokio::time::advance(Duration::from_secs(2 * 60)).await;
        let late = ledger
            .join("d1", "chat", "turn", answer_after(Duration::ZERO, &runs))
            .await;
        assert_eq!(late, Ok(json!(4)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_read_only_command_joins_only_while_it_runs() {
        let ledger = Arc::new(RequestLedger::default());
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let slow = Duration::from_secs(5);
        let (first, joined) = tokio::join!(
            ledger.join("d1", "session_load", "r1", answer_after(slow, &runs)),
            ledger.join("d1", "session_load", "r1", answer_after(slow, &runs)),
        );
        assert_eq!(first, Ok(json!(1)));
        assert_eq!(joined, Ok(json!(1)));
        assert_eq!(ledger.len(), 0);
        let again = ledger
            .join(
                "d1",
                "session_load",
                "r1",
                answer_after(Duration::ZERO, &runs),
            )
            .await;
        assert_eq!(again, Ok(json!(2)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_ledger_refuses_rather_than_forgetting_running_commands() {
        let ledger = Arc::new(RequestLedger::default());
        for index in 0..DEVICE_REQUESTS {
            let ledger = ledger.clone();
            tokio::spawn(async move {
                let _ = ledger
                    .join("d1", "chat", &format!("t{index}"), never_answers())
                    .await;
            });
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(ledger.len(), DEVICE_REQUESTS);
        let refused = ledger.join("d1", "chat", "one-more", never_answers()).await;
        assert_eq!(refused, Err(TOO_MANY_REQUESTS.to_string()));
        assert_eq!(ledger.len(), DEVICE_REQUESTS);
        // Another device has its own room.
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let other = ledger
            .join("d2", "chat", "t0", answer_after(Duration::ZERO, &runs))
            .await;
        assert_eq!(other, Ok(json!(1)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_device_drops_its_oldest_finished_answer() {
        let ledger = Arc::new(RequestLedger::default());
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for index in 0..DEVICE_REQUESTS {
            ledger
                .join(
                    "d1",
                    "chat",
                    &format!("t{index}"),
                    answer_after(Duration::ZERO, &runs),
                )
                .await
                .unwrap();
        }
        ledger
            .join("d1", "chat", "new", answer_after(Duration::ZERO, &runs))
            .await
            .unwrap();
        assert_eq!(ledger.len(), DEVICE_REQUESTS);
        // The oldest answer went; the newest are still remembered.
        let oldest = ledger
            .join("d1", "chat", "t0", answer_after(Duration::ZERO, &runs))
            .await;
        assert_eq!(oldest, Ok(json!(DEVICE_REQUESTS + 2)));
        let kept = ledger
            .join("d1", "chat", "new", answer_after(Duration::ZERO, &runs))
            .await;
        assert_eq!(kept, Ok(json!(DEVICE_REQUESTS + 1)));
    }

    #[tokio::test]
    async fn remembered_answers_stay_within_the_byte_cap() {
        let ledger = Arc::new(RequestLedger::default());
        let big = |bytes: usize| {
            move || {
                Ok(Box::pin(async move { Ok(json!("x".repeat(bytes))) })
                    as dispatch::DispatchFuture)
            }
        };
        let five_mib = 5 * 1024 * 1024;
        ledger.join("d1", "chat", "a", big(five_mib)).await.unwrap();
        assert_eq!(ledger.len(), 1);
        // A second 5 MiB answer would pass 8 MiB: the older one goes.
        ledger.join("d1", "chat", "b", big(five_mib)).await.unwrap();
        assert_eq!(ledger.len(), 1);
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let again = ledger
            .join("d1", "chat", "b", answer_after(Duration::ZERO, &runs))
            .await
            .unwrap();
        assert_eq!(again.as_str().map(str::len), Some(five_mib));
        assert_eq!(run_count(&runs), 0);
        // An answer larger than the whole cap is not remembered at all.
        ledger
            .join("d1", "chat", "c", big(LEDGER_BYTES + 1))
            .await
            .unwrap();
        assert_eq!(ledger.len(), 0);
    }

    #[tokio::test]
    async fn a_socket_message_over_64_kib_closes_the_socket() {
        use crate::project_id::test_support::temp_root;
        use futures_util::SinkExt;

        let temp = temp_root("remote-socket-limit");
        let dirs = DataDirs::from_bases(&temp.join("roaming"), &temp.join("local"));
        let registry = crate::project_registry::ProjectRegistry::new(dirs.clone());
        let auth = Arc::new(RemoteAuth::load(dirs));
        let code = auth.begin_pairing().unwrap();
        let token = auth.pair(&code.code, "폰").unwrap().token;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let running = serve_test(
            listener,
            auth,
            Some(RemoteBackend {
                connections: RemoteConnections::new(registry),
                hub: RemoteHub::new(),
                dispatcher: Arc::new(EchoDispatcher),
            }),
        );
        let (mut socket, _) = connect(port, &token).await;
        // A message at the limit is answered.
        let padded = format!(
            r#"{{"type":"ping","pad":"{}"}}"#,
            "x".repeat(SOCKET_MESSAGE_LIMIT - 40)
        );
        assert!(padded.len() <= SOCKET_MESSAGE_LIMIT);
        send_socket(&mut socket, serde_json::from_str(&padded).unwrap()).await;
        assert_eq!(next_json(&mut socket).await["type"], "pong");
        // One past it ends the socket.
        let _ = socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                "x".repeat(SOCKET_MESSAGE_LIMIT + 1).into(),
            ))
            .await;
        expect_closed(&mut socket).await;
        stop(running).await;
        std::fs::remove_dir_all(temp).ok();
    }

    async fn connect(port: u16, token: &str) -> (Socket, String) {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = format!("ws://127.0.0.1:{port}/remote/api/events")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let hello = next_json(&mut socket).await;
        assert_eq!(hello["type"], "hello");
        let conn = hello["connId"].as_str().unwrap().to_string();
        (socket, conn)
    }

    /// The socket closes well before the 25-second heartbeat would notice.
    async fn expect_closed(socket: &mut Socket) {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match socket.next().await {
                    None | Some(Err(_)) | Some(Ok(WsMessage::Close(_))) => return,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await
        .expect("socket closed at once");
    }

    async fn wait_until(mut condition: impl FnMut() -> bool, what: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !condition() {
            assert!(tokio::time::Instant::now() < deadline, "{what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn sockets_close_when_their_device_is_removed_or_remote_access_stops() {
        use crate::project_id::test_support::{create_project, temp_root};

        let temp = temp_root("remote-socket-stop");
        let dirs = DataDirs::from_bases(&temp.join("roaming"), &temp.join("local"));
        let registry = crate::project_registry::ProjectRegistry::new(dirs.clone());
        let project = registry
            .register(&create_project(&temp.join("p"), "Sample"))
            .unwrap();
        let server = RemoteServer::new(dirs, fake_assets());
        server.set_backend(RemoteBackend {
            connections: RemoteConnections::new(registry.clone()),
            hub: RemoteHub::new(),
            dispatcher: Arc::new(EchoDispatcher),
        });
        server.apply(true, 0).await;
        let port = server.bound_port().await.unwrap();
        let pair = |name: &str| {
            let code = server.auth().begin_pairing().unwrap();
            server.auth().pair(&code.code, name).unwrap()
        };
        let phone = pair("폰");
        let tablet = pair("태블릿");
        let (mut phone_socket, _) = connect(port, &phone.token).await;
        let (mut tablet_socket, _) = connect(port, &tablet.token).await;
        for socket in [&mut phone_socket, &mut tablet_socket] {
            send_socket(
                socket,
                json!({"type": "attach", "projectId": project.id().as_str()}),
            )
            .await;
            assert_eq!(next_json(socket).await["type"], "attached");
        }
        assert_eq!(registry.window_labels(project.id()).len(), 2);

        // Removing the phone closes its socket at once and unbinds it; the
        // tablet's socket keeps working.
        server.auth().revoke(&phone.device.id).unwrap();
        expect_closed(&mut phone_socket).await;
        wait_until(
            || registry.window_labels(project.id()).len() == 1,
            "the removed device's binding stayed",
        )
        .await;
        send_socket(&mut tablet_socket, json!({"type": "ping"})).await;
        assert_eq!(next_json(&mut tablet_socket).await["type"], "pong");

        // Turning remote access off closes every socket and binding.
        server.apply(false, 0).await;
        expect_closed(&mut tablet_socket).await;
        wait_until(
            || registry.window_labels(project.id()).is_empty(),
            "a binding outlived remote access",
        )
        .await;
        assert!(!project.is_closed());
        std::fs::remove_dir_all(temp).ok();
    }

    /// Stages an attachment only once `release` is notified.
    struct GateDispatcher {
        release: Arc<tokio::sync::Notify>,
    }

    impl RemoteDispatcher for GateDispatcher {
        fn invoke(&self, _call: RemoteCall) -> dispatch::DispatchFuture {
            Box::pin(async { Ok(Value::Null) })
        }

        fn invoke_binary(&self, _call: RemoteCall) -> dispatch::BinaryFuture {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn stage_attachment(&self, _headers: HeaderMap, body: Bytes) -> dispatch::DispatchFuture {
            let release = self.release.clone();
            Box::pin(async move {
                release.notified().await;
                Ok(json!({ "size": body.len() }))
            })
        }
    }

    #[tokio::test]
    async fn only_two_attachments_upload_at_once() {
        use crate::project_id::test_support::temp_root;

        let temp = temp_root("remote-uploads");
        let dirs = DataDirs::from_bases(&temp.join("roaming"), &temp.join("local"));
        let registry = crate::project_registry::ProjectRegistry::new(dirs.clone());
        let release = Arc::new(tokio::sync::Notify::new());
        let auth = Arc::new(RemoteAuth::load(dirs));
        let code = auth.begin_pairing().unwrap();
        let token = auth.pair(&code.code, "폰").unwrap().token;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let running = serve_test(
            listener,
            auth,
            Some(RemoteBackend {
                connections: RemoteConnections::new(registry),
                hub: RemoteHub::new(),
                dispatcher: Arc::new(GateDispatcher {
                    release: release.clone(),
                }),
            }),
        );
        let (_socket, conn) = connect(port, &token).await;
        let client = reqwest::Client::new();
        let upload = |id: &str| {
            client
                .post(format!(
                    "http://127.0.0.1:{port}/remote/api/attachment_stage"
                ))
                .bearer_auth(&token)
                .header(CONNECTION_HEADER, &conn)
                .header(REQUEST_HEADER, id)
                .body(vec![1u8; 10])
                .send()
        };
        let first = tokio::spawn(upload("u1"));
        let second = tokio::spawn(upload("u2"));
        tokio::time::sleep(Duration::from_millis(200)).await;
        let third: Value = upload("u3").await.unwrap().json().await.unwrap();
        assert_eq!(third["ok"], false);
        assert_eq!(third["error"], UPLOADS_BUSY);
        release.notify_waiters();
        for upload in [first, second] {
            let body: Value = upload.await.unwrap().unwrap().json().await.unwrap();
            assert_eq!(body["result"]["size"], 10);
        }
        stop(running).await;
        std::fs::remove_dir_all(temp).ok();
    }

    #[tokio::test]
    async fn funnel_tailnet_identity_and_security_headers() {
        let auth = Arc::new(RemoteAuth::load(temp_dirs("identity")));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let funnel = Arc::new(AtomicBool::new(false));
        let (stopping, receiver) = watch::channel(false);
        let running = serve(
            listener,
            port,
            router(
                auth.clone(),
                fake_assets(),
                None,
                ListenerControl {
                    stopping: receiver,
                    funnel: funnel.clone(),
                },
            ),
            stopping,
        );
        let base = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::new();
        let tailnet = "pc.tail1234.ts.net";

        // The page carries its CSP and refuses framing; API answers are
        // never cached.
        let page = client.get(format!("{base}/remote/")).send().await.unwrap();
        let csp = page.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .to_string();
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
        assert!(csp.contains("connect-src 'self' wss:"), "{csp}");
        assert_eq!(page.headers()[header::X_FRAME_OPTIONS], "DENY");
        let me = client
            .get(format!("{base}/remote/api/me"))
            .send()
            .await
            .unwrap();
        assert_eq!(me.headers()[header::CACHE_CONTROL], "no-store");

        // Pairing through tailscale serve needs the tailnet login.
        let pair = |login: Option<&'static str>| {
            let code = auth.begin_pairing().unwrap().code;
            let mut request = client
                .post(format!("{base}/remote/api/pair"))
                .header("host", tailnet)
                .json(&json!({"code": code, "deviceName": "폰"}));
            if let Some(login) = login {
                request = request.header("tailscale-user-login", login);
            }
            request.send()
        };
        let response = pair(None).await.unwrap();
        assert_eq!(response.status(), 401);
        let body: Value = response.json().await.unwrap();
        assert!(body["error"].as_str().unwrap().contains("Tailscale"));
        // serve forwards the client's Host as is: a tailnet peer that claims
        // `Host: 127.0.0.1` is still a proxied request and needs the login.
        let code = auth.begin_pairing().unwrap().code;
        let spoofed_pair = client
            .post(format!("{base}/remote/api/pair"))
            .header("host", "127.0.0.1")
            .header("x-forwarded-host", "127.0.0.1")
            .header("x-forwarded-for", "100.64.0.7")
            .header("x-forwarded-proto", "https")
            .json(&json!({"code": code, "deviceName": "남의 폰"}))
            .send()
            .await
            .unwrap();
        assert_eq!(spoofed_pair.status(), 401);
        assert!(auth.devices().is_empty());
        let response = pair(Some("me@example.com")).await.unwrap();
        assert_eq!(response.status(), 200);
        let token = cookie_token(&response);
        assert_eq!(
            auth.devices()[0].owner_login.as_deref(),
            Some("me@example.com")
        );

        let me_as = |login: Option<&'static str>, host: &'static str| {
            let mut request = client
                .get(format!("{base}/remote/api/me"))
                .header("host", host)
                .bearer_auth(&token);
            if let Some(login) = login {
                request = request.header("tailscale-user-login", login);
            }
            request.send()
        };
        assert_eq!(
            me_as(Some("me@example.com"), tailnet)
                .await
                .unwrap()
                .status(),
            200
        );
        // The same token from another tailnet user, or with no login.
        assert_eq!(
            me_as(Some("other@example.com"), tailnet)
                .await
                .unwrap()
                .status(),
            401
        );
        let refused = me_as(None, tailnet).await.unwrap();
        assert_eq!(refused.status(), 401);
        let body: Value = refused.json().await.unwrap();
        assert_eq!(body["code"], "identity");
        // On the PC itself the token is enough.
        assert_eq!(me_as(None, "127.0.0.1").await.unwrap().status(), 200);
        let spoofed = || {
            client
                .get(format!("{base}/remote/api/me"))
                .header("host", "127.0.0.1")
                .header("x-forwarded-host", tailnet)
                .header("x-forwarded-for", "100.64.0.7")
                .bearer_auth(&token)
                .send()
        };
        let refused = spoofed().await.unwrap();
        assert_eq!(refused.status(), 401);
        let body: Value = refused.json().await.unwrap();
        assert_eq!(body["code"], "identity");

        // A request serve marked as Funnel traffic is refused outright.
        let response = client
            .get(format!("{base}/remote/"))
            .header("tailscale-funnel-request", "?1")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        // While Funnel exposes the port, only the PC itself is served.
        funnel.store(true, Ordering::SeqCst);
        assert_eq!(
            me_as(Some("me@example.com"), tailnet)
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(me_as(None, "127.0.0.1").await.unwrap().status(), 200);
        assert_eq!(spoofed().await.unwrap().status(), 403);

        stop(running).await;
    }
}
