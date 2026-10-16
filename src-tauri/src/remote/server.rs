//! The loopback HTTP server phones reach through `tailscale serve`.
//!
//! Binds `127.0.0.1:<port>` only. Everything lives under `/remote`: the static
//! remote panel from the app's embedded frontend, and `/remote/api/*`, which
//! requires a paired device token except for `pair`.
//!
//! A paired client opens one event socket (`/remote/api/events`), which names
//! its connection, and sends commands to `/remote/api/invoke/<command>` with
//! that connection in `x-eud-remote-conn`; only the allowlist in
//! [`super::dispatch`] is accepted.

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
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

use super::auth::{self, DeviceView, RemoteAuth};
use super::caller::{RemoteCaller, RemoteConnections, PROJECT_NOT_OPEN, UNKNOWN_CONNECTION};
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
}

pub fn router(
    auth: Arc<RemoteAuth>,
    assets: AssetLookup,
    backend: Option<RemoteBackend>,
) -> Router {
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
        .layer(axum::middleware::from_fn(guard))
        .with_state(AppState {
            auth,
            assets,
            backend,
        })
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

fn unauthorized() -> Response {
    error_response(
        StatusCode::UNAUTHORIZED,
        "연결된 기기가 아닙니다. PC의 설정 → 원격 접속에서 기기를 연결해 주세요.",
    )
}

/// Host allowlist and cross-origin refusal for every request.
async fn guard(request: Request, next: Next) -> Response {
    if !auth::host_allowed(request.headers()) {
        return error_response(StatusCode::MISDIRECTED_REQUEST, "허용되지 않은 주소입니다.");
    }
    if !auth::origin_allowed(request.method(), request.headers()) {
        return error_response(
            StatusCode::FORBIDDEN,
            "다른 사이트에서 보낸 요청은 받을 수 없습니다.",
        );
    }
    next.run(request).await
}

fn authenticated(state: &AppState, headers: &HeaderMap) -> Option<DeviceView> {
    let token = auth::token_from_headers(headers)?;
    state.auth.authenticate(&token)
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

async fn pair(State(state): State<AppState>, Json(request): Json<PairRequest>) -> Response {
    let auth = state.auth.clone();
    let result =
        tokio::task::spawn_blocking(move || auth.pair(&request.code, &request.device_name)).await;
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
        Some(device) => Json(DeviceResponse::from(device)).into_response(),
        None => unauthorized(),
    }
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(device) = authenticated(&state, &headers) else {
        return unauthorized();
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
    if authenticated(&state, &headers).is_none() {
        return unauthorized();
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
    let Some(device) = authenticated(&state, &headers) else {
        return unauthorized();
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
    if command == "remote_projects" {
        return invoke_result(Ok(json!(backend.connections.open_projects())));
    }
    let Some(caller) = request_caller(&backend, &headers, &device) else {
        return no_connection();
    };
    let result = match command.as_str() {
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
            let attached = backend.connections.attached(&caller.conn_id);
            backend
                .dispatcher
                .invoke(RemoteCall {
                    caller,
                    attached,
                    command,
                    args,
                })
                .await
        }
    };
    invoke_result(result)
}

/// `POST /remote/api/attachment_stage`: the raw file body with the headers
/// the desktop's `attachment_stage` invoke sends.
async fn attachment_stage(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(device) = authenticated(&state, &headers) else {
        return unauthorized();
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
    invoke_result(backend.dispatcher.stage_attachment(headers, body).await)
}

/// `GET /remote/api/events`: the event socket.
async fn events(State(state): State<AppState>, request: Request) -> Response {
    let Some(device) = authenticated(&state, request.headers()) else {
        return unauthorized();
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
    upgrade.on_upgrade(move |socket| run_socket(socket, backend, auth, device.id))
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

/// One event socket: announce its connection id, answer attach/detach/ping,
/// and forward the hub's events for its project until either side closes.
/// The binding goes with the socket.
async fn run_socket(
    mut socket: WebSocket,
    backend: RemoteBackend,
    auth: Arc<RemoteAuth>,
    device_id: String,
) {
    let mut events = backend.hub.subscribe();
    let conn = backend.connections.open(&device_id).conn_id;
    let mut heartbeat =
        tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
    // Whether the socket was attached when it last looked, to tell the client
    // once when the desktop closes its project.
    let mut was_attached = false;
    let mut open = send_json(&mut socket, json!({ "type": "hello", "connId": conn })).await;
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
                            json!({ "type": "detached", "error": PROJECT_NOT_OPEN }),
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
            _ = heartbeat.tick() => {
                if !auth.devices().iter().any(|device| device.id == device_id) {
                    // Revoked: its socket goes too.
                    let _ = socket.send(Message::Close(None)).await;
                    open = false;
                } else if was_attached && backend.connections.attached(&conn).is_none() {
                    was_attached = false;
                    open = send_json(
                        &mut socket,
                        json!({ "type": "detached", "error": PROJECT_NOT_OPEN }),
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
    if !safe_asset_path(&path) {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve_asset(&state, &path)
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
    // The frontend resolver falls back to index.html for unknown paths; a
    // script or style request must not receive the desktop page instead.
    if asset.mime_type.starts_with("text/html") && !path.ends_with(".html") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let content_type = HeaderValue::from_str(&asset.mime_type)
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

    /// Start or stop according to the saved settings.
    pub async fn start_from_config(&self) {
        match self.dirs.load_config() {
            Ok(config) => self.apply(config.remote.enabled, config.remote.port).await,
            Err(error) => eprintln!("eud-agent: remote server not started: {error}"),
        }
    }

    /// Stop any listener, then bind `127.0.0.1:<port>` when enabled.
    pub async fn apply(&self, enabled: bool, port: u16) {
        let mut slot = self.slot.lock().await;
        if let Some(running) = slot.running.take() {
            stop(running).await;
        }
        slot.error = None;
        if !enabled {
            return;
        }
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => {
                slot.running = Some(serve(
                    listener,
                    port,
                    router(
                        self.auth.clone(),
                        self.assets.clone(),
                        self.backend.get().cloned(),
                    ),
                ));
            }
            Err(error) => {
                slot.error = Some(format!(
                    "포트 {port}을(를) 열 수 없습니다({error}). 다른 프로그램이 쓰고 있다면 다른 포트를 지정해 주세요."
                ));
            }
        }
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

fn serve(listener: tokio::net::TcpListener, port: u16, app: Router) -> Running {
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
        task,
    }
}

async fn stop(running: Running) {
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
            // The real resolver's index.html fallback.
            _ => Some(StaticAsset {
                bytes: b"<!doctype html><title>desktop</title>".to_vec(),
                mime_type: "text/html".to_string(),
            }),
        })
    }

    async fn start(auth: Arc<RemoteAuth>) -> (String, Running) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let running = serve(listener, port, router(auth, fake_assets(), None));
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
        let running = serve(listener, port, router(auth, fake_assets(), Some(backend)));
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

        // The desktop closes the project: the client is told on the next event.
        registry.close(mine.id());
        hub.publish_global("session_activity", &json!({"sessionId": "s2"}));
        let reply = next_json(&mut socket).await;
        assert_eq!(reply["type"], "detached");
        assert_eq!(reply["error"], PROJECT_NOT_OPEN);
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
}
