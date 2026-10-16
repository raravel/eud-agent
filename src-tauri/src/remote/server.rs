//! The loopback HTTP server phones reach through `tailscale serve`.
//!
//! Binds `127.0.0.1:<port>` only. Everything lives under `/remote`: the static
//! remote panel from the app's embedded frontend, and `/remote/api/*`, which
//! requires a paired device token except for `pair`.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::auth::{self, DeviceView, RemoteAuth};

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

#[derive(Clone)]
struct AppState {
    auth: Arc<RemoteAuth>,
    assets: AssetLookup,
}

pub fn router(auth: Arc<RemoteAuth>, assets: AssetLookup) -> Router {
    Router::new()
        .route("/remote", get(|| async { Redirect::temporary("/remote/") }))
        .route("/remote/", get(remote_page))
        .route("/remote/api/pair", post(pair))
        .route("/remote/api/me", get(me))
        .route("/remote/api/logout", post(logout))
        .route("/remote/api", any(unknown_api))
        .route("/remote/api/{*rest}", any(unknown_api))
        .route("/remote/{*path}", get(static_file))
        .layer(axum::middleware::from_fn(guard))
        .with_state(AppState { auth, assets })
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
    slot: tokio::sync::Mutex<Slot>,
}

impl RemoteServer {
    pub fn new(dirs: crate::config::DataDirs, assets: AssetLookup) -> Arc<Self> {
        let auth = Arc::new(RemoteAuth::load(dirs.clone()));
        Arc::new(Self {
            dirs,
            auth,
            assets,
            slot: tokio::sync::Mutex::new(Slot::default()),
        })
    }

    pub fn dirs(&self) -> &crate::config::DataDirs {
        &self.dirs
    }

    pub fn auth(&self) -> &Arc<RemoteAuth> {
        &self.auth
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
                    router(self.auth.clone(), self.assets.clone()),
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
        let running = serve(listener, port, router(auth, fake_assets()));
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
}
