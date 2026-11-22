//! `mapvault_request` / `mapvault_sdk_install` against a local stand-in of
//! the MapVault site (loopback http, which the address check allows).

use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};

use super::{SessionToolRuntime, ToolServices};
use crate::native_project::{NativeProject, ProjectManifest, PROJECT_SCHEMA_VERSION};
use crate::tools;

const KEY: &str = "mv_abcdefghabcdefghabcdefghabcdefgh";
/// Key material as the generator emits it (`sdk/eps.ts`): a round key, a
/// key-derived self-test table and the two self-test probes.
const SECRET_LINE: &str = "var rke0 = 0x5EC2E7A1;\nvar rkm26 = 0x0BADF00D;\nconst T_KEY = EUDArray(py_eval(\"[17,4,29]\"));\n    var a, b = speck_e(0x01234567, 0x89ABCDEF);\n    if (macX != 0xC0FFEE01 || macY != 0xC0FFEE02) { ok = 0; }\n    if (a != 0xDEAD0001 || b != 0xDEAD0002 || c != 0xDEAD0003) { ok = 0; }";
/// Every secret value of [`SECRET_LINE`].
const SECRET_VALUES: [&str; 8] = [
    "5EC2E7A1", "0BADF00D", "17,4,29", "C0FFEE01", "C0FFEE02", "DEAD0001", "DEAD0002", "DEAD0003",
];

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    uri: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

type Answer = (u16, Vec<(&'static str, String)>, Vec<u8>);
type Responder = Arc<dyn Fn(&Seen) -> Answer + Send + Sync>;

/// A loopback HTTP server recording every request and answering through
/// `respond`.
struct Site {
    origin: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Site {
    async fn start(respond: impl Fn(&Seen) -> Answer + Send + Sync + 'static) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let respond: Responder = Arc::new(respond);
        let (log, answer) = (Arc::clone(&seen), Arc::clone(&respond));
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let (log, answer) = (Arc::clone(&log), Arc::clone(&answer));
            async move {
                let (parts, body) = request.into_parts();
                let body = axum::body::to_bytes(body, 64 * 1024 * 1024)
                    .await
                    .unwrap()
                    .to_vec();
                let seen = Seen {
                    method: parts.method.to_string(),
                    uri: parts.uri.to_string(),
                    headers: parts
                        .headers
                        .iter()
                        .map(|(name, value)| {
                            (
                                name.to_string(),
                                value.to_str().unwrap_or_default().to_string(),
                            )
                        })
                        .collect(),
                    body,
                };
                let (status, headers, body) = answer(&seen);
                log.lock().push(seen);
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
        Self { origin, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().clone()
    }
}

fn json_answer(status: u16, value: Value) -> Answer {
    (
        status,
        vec![(
            "content-type",
            "application/json; charset=utf-8".to_string(),
        )],
        value.to_string().into_bytes(),
    )
}

/// An open project whose MapVault address is `origin`, an EPS session in a
/// write turn, and its run identity.
fn session(
    name: &str,
    origin: &str,
    key: Option<&str>,
) -> (
    ToolServices,
    std::path::PathBuf,
    SessionToolRuntime,
    crate::provider_runtime::RunIdentity,
) {
    let mut services = ToolServices::for_tests();
    services.set_mapvault_key_for_tests(key);
    let mut config = services.dirs.load_config().unwrap();
    config.mapvault.address = origin.to_string();
    services.dirs.save_config(&config).unwrap();
    let root = services.dirs.app_data().join(name);
    std::fs::create_dir_all(root.join("maps")).unwrap();
    std::fs::write(root.join("maps/source.scx"), b"fixture map").unwrap();
    let project = NativeProject::create(
        &root,
        ProjectManifest {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: name.into(),
            source_map: "maps/source.scx".into(),
            output_map: "build/output.scx".into(),
            main_file: "src/main.eps".into(),
            settings: Default::default(),
            plugins: Vec::new(),
            python_entrypoints: Vec::new(),
            python_dependencies: Vec::new(),
            python_lock: None,
            editor_compatibility: None,
            e2_import: None,
        },
    )
    .unwrap();
    services.bind_for_tests(&project);
    let root = services.native().project_root().unwrap();
    // A turn's source baseline, as the engine captures it before the model runs.
    let manager = crate::workspace::WorkspaceManager::for_root(services.dirs.clone(), root.clone());
    let workspace = manager
        .prepare_session_current(&format!("{name}-session"))
        .unwrap();
    let baseline = manager.begin_turn(&workspace, "req-mv").unwrap();
    let runtime = services.session(format!("{name}-session"));
    runtime.begin_request("req-mv", name).unwrap();
    runtime.register_write_request("mapvault").unwrap();
    runtime
        .bind_source_baseline("req-mv", baseline.baseline_root.clone())
        .unwrap();
    let (cancel, receiver) = tokio::sync::watch::channel(0_u64);
    runtime.set_cancellation(receiver);
    std::mem::forget(cancel);
    let identity = crate::provider_runtime::RunIdentity {
        session_id: runtime.session_id().to_string(),
        run_id: crate::provider_runtime::RunId::new(1),
        request_id: "req-mv".to_string(),
        session_kind: crate::session::SessionKind::Eps,
        cancellation_generation: 0,
    };
    (services, root, runtime, identity)
}

fn cards(
    runtime: &SessionToolRuntime,
) -> tokio::sync::mpsc::UnboundedReceiver<crate::ipc::AskEvent> {
    let (events, emitted) = tokio::sync::mpsc::unbounded_channel();
    runtime.set_ask_emitter(move |event| {
        events
            .send(event)
            .map_err(|_| "ask event receiver closed".to_string())
    });
    emitted
}

fn answer(runtime: &SessionToolRuntime, card: &crate::ipc::AskEvent, label: &str) {
    runtime
        .answer_ask(
            &card.request_id,
            BTreeMap::from([(
                card.questions[0].id.clone(),
                crate::ipc::AskAnswer {
                    answers: vec![label.to_string()],
                },
            )]),
        )
        .unwrap();
}

/// The provider tool gate of the session's run, as every provider calls it.
fn gate(
    runtime: &SessionToolRuntime,
    identity: &crate::provider_runtime::RunIdentity,
    access: crate::provider_runtime::WorkspaceAccess,
) -> crate::provider_tool_loop::RunGate {
    // Each gate is its own run, as each provider run of a turn is.
    static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(100);
    let identity = crate::provider_runtime::RunIdentity {
        run_id: crate::provider_runtime::RunId::new(
            RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        ),
        ..identity.clone()
    };
    crate::provider_tool_loop::RunGate::new(identity, runtime.clone(), access, None)
}

async fn call(
    gate: &crate::provider_tool_loop::RunGate,
    tool: &str,
    arguments: Value,
) -> crate::provider_tool_loop::DirectToolResult {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let batch = gate
        .dispatch_batch(
            vec![crate::provider_tool_loop::DirectToolCall {
                id: format!("call-{id}"),
                name: tool.to_string(),
                arguments,
            }],
            false,
        )
        .await
        .unwrap();
    batch.results.into_iter().next().unwrap()
}

async fn request(
    runtime: &SessionToolRuntime,
    identity: &crate::provider_runtime::RunIdentity,
    args: Value,
) -> Result<Value, String> {
    runtime
        .mapvault_for_run(identity, tools::MAPVAULT_REQUEST_TOOL, &args)
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_key_both_tools_point_to_settings_and_send_nothing() {
    let site = Site::start(|_| json_answer(200, json!({}))).await;
    let (_services, root, runtime, identity) = session("mv-no-key", &site.origin, None);
    let error = request(&runtime, &identity, json!({"method": "GET", "path": "/me"}))
        .await
        .unwrap_err();
    assert!(error.contains("Settings → MapVault"), "{error}");
    assert!(error.contains("Never ask"), "{error}");
    let error = runtime
        .mapvault_for_run(
            &identity,
            tools::MAPVAULT_SDK_INSTALL_TOOL,
            &json!({"map": "m", "version": 1}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("Settings → MapVault"), "{error}");
    assert!(site.seen().is_empty());
    assert!(!root.join("src/mapvault.eps").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_runs_at_once_with_the_key_and_the_answer_never_carries_it() {
    // The stand-in echoes the Authorization header back, as a hostile or
    // broken server might.
    let site = Site::start(|seen| {
        json_answer(
            200,
            json!({"maps": [], "echo": seen.headers.get("authorization").cloned()}),
        )
    })
    .await;
    let (_services, _root, runtime, identity) = session("mv-read", &site.origin, Some(KEY));
    let value = request(
        &runtime,
        &identity,
        json!({"method": "GET", "path": "/maps/m/players?q=홍길동&page=2"}),
    )
    .await
    .unwrap();
    assert_eq!(value["status"], 200);
    assert_eq!(value["ok"], true);
    assert_eq!(value["json"]["maps"], json!([]));
    assert!(!value.to_string().contains(KEY), "{value}");
    let seen = site.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(
        seen[0].uri,
        "/api/v1/maps/m/players?q=%ED%99%8D%EA%B8%B8%EB%8F%99&page=2"
    );
    assert_eq!(
        seen[0].headers.get("authorization").map(String::as_str),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert!(runtime.pending_ask().is_none(), "a read asks nothing");

    // Errors are scrubbed too.
    let site = Site::start(|seen| {
        (
            500,
            vec![("content-type", "text/plain".to_string())],
            seen.headers["authorization"].clone().into_bytes(),
        )
    })
    .await;
    let (_services, _root, runtime, identity) = session("mv-read-error", &site.origin, Some(KEY));
    let value = request(&runtime, &identity, json!({"method": "GET", "path": "/me"}))
        .await
        .unwrap();
    assert_eq!(value["status"], 500);
    assert!(!value.to_string().contains(KEY), "{value}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostile_paths_sdk_downloads_and_other_sessions_are_refused_before_sending() {
    let site = Site::start(|_| json_answer(200, json!({}))).await;
    let (services, _root, runtime, identity) = session("mv-refuse", &site.origin, Some(KEY));
    for (args, expected) in [
        (
            json!({"method": "GET", "path": "/maps/m/versions/1/sdk/mapvault.eps"}),
            "mapvault_sdk_install",
        ),
        (
            json!({"method": "GET", "path": "/maps/m/versions/1/sdk/msqc.edd"}),
            "mapvault_sdk_install",
        ),
        (
            json!({"method": "GET", "path": "https://evil.example/api/v1/maps"}),
            "invalid MapVault path",
        ),
        (
            json!({"method": "GET", "path": "/../../evil"}),
            "invalid MapVault path",
        ),
        (
            json!({"method": "GET", "path": "/maps\\..\\x"}),
            "invalid MapVault path",
        ),
        (
            json!({"method": "GET", "path": "/me", "body": "{}"}),
            "takes no",
        ),
        (
            json!({"method": "POST", "path": "/maps", "body": "{not json"}),
            "JSON request body",
        ),
        (
            json!({"method": "POST", "path": "/maps", "file": "build/output.scx"}),
            "only for POST",
        ),
        (
            json!({"method": "POST", "path": "/maps/m/versions/1/file"}),
            "pass `file`",
        ),
        (json!({"method": "TRACE", "path": "/me"}), "TRACE"),
        (json!({"method": "GET", "path": "/me", "url": "x"}), "url"),
    ] {
        let error = request(&runtime, &identity, args.clone())
            .await
            .unwrap_err();
        assert!(error.contains(expected), "{args}: {error}");
    }
    assert!(site.seen().is_empty());

    // Workers and Map sessions are never offered the tools and refuse them.
    let worker = services.session("mv-worker");
    worker.begin_request("req-mv", "mv-refuse").unwrap();
    worker.set_worker_parent(Some("parent".to_string()));
    let (cancel, receiver) = tokio::sync::watch::channel(0_u64);
    worker.set_cancellation(receiver);
    std::mem::forget(cancel);
    let worker_identity = crate::provider_runtime::RunIdentity {
        session_id: worker.session_id().to_string(),
        ..identity.clone()
    };
    let error = request(
        &worker,
        &worker_identity,
        json!({"method": "GET", "path": "/me"}),
    )
    .await
    .unwrap_err();
    assert!(error.contains("worker_tool_unavailable"), "{error}");
    for profile in [
        crate::worker::WorkerProfile::Research,
        crate::worker::WorkerProfile::MapResearch,
        crate::worker::WorkerProfile::Implementation,
    ] {
        assert!(!profile.permits(tools::MAPVAULT_REQUEST_TOOL));
        assert!(!profile.permits(tools::MAPVAULT_SDK_INSTALL_TOOL));
    }
    assert!(tools::map_tool_registry()
        .iter()
        .all(|spec| !tools::is_mapvault_tool(spec.name)));
    let map = services.map_session("mv-map");
    map.begin_request("req-mv", "mv-refuse").unwrap();
    let map_identity = crate::provider_runtime::RunIdentity {
        session_id: map.session_id().to_string(),
        session_kind: crate::session::SessionKind::Map,
        ..identity.clone()
    };
    assert!(
        request(&map, &map_identity, json!({"method": "GET", "path": "/me"}))
            .await
            .is_err()
    );
    // A synchronous caller gets the reason, never a request.
    assert!(runtime
        .execute_for_run(
            &identity,
            tools::MAPVAULT_REQUEST_TOOL,
            &json!({"method": "GET", "path": "/me"})
        )
        .unwrap_err()
        .contains("provider tool gate"));
    assert!(site.seen().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_waits_for_the_card_and_a_refusal_sends_nothing() {
    let site = Site::start(|seen| {
        json_answer(200, json!({"id": 12, "status": "active", "uri": seen.uri}))
    })
    .await;
    let (_services, _root, runtime, identity) = session("mv-release", &site.origin, Some(KEY));
    let mut emitted = cards(&runtime);
    let args = json!({
        "method": "POST",
        "path": "/maps/my-map/versions/12/status",
        "body": "{\"status\":\"active\"}",
    });

    // Refused: nothing is sent and the model is told not to retry.
    let (asking, run, call) = (runtime.clone(), identity.clone(), args.clone());
    let refused = tokio::spawn(async move { request(&asking, &run, call).await });
    let card = emitted.recv().await.unwrap();
    assert_eq!(card.kind, crate::ipc::AskKind::MapvaultConfirm);
    let question = &card.questions[0];
    assert_eq!(question.header.as_deref(), Some("MapVault 확인"));
    assert!(question.question.contains("배포"), "{}", question.question);
    assert!(
        question.question.contains("되돌릴 수 없습니다"),
        "{}",
        question.question
    );
    assert!(
        question.question.contains("my-map"),
        "{}",
        question.question
    );
    assert!(
        question.question.contains("버전 id 12"),
        "{}",
        question.question
    );
    assert!(
        question.question.contains(&site.origin),
        "{}",
        question.question
    );
    let preview = question.preview.as_deref().unwrap();
    assert!(
        preview.contains("POST /api/v1/maps/my-map/versions/12/status"),
        "{preview}"
    );
    assert!(preview.contains("{\"status\":\"active\"}"), "{preview}");
    assert!(!format!("{card:?}").contains(KEY));
    answer(&runtime, &card, "거부");
    let error = refused.await.unwrap().unwrap_err();
    assert!(error.contains("did not allow"), "{error}");
    assert!(error.contains("Do not send it again"), "{error}");
    assert!(site.seen().is_empty());

    // Allowed: exactly that request is sent, once.
    let (asking, run, call) = (runtime.clone(), identity.clone(), args.clone());
    let allowed = tokio::spawn(async move { request(&asking, &run, call).await });
    let card = emitted.recv().await.unwrap();
    answer(&runtime, &card, "허용");
    let value = allowed.await.unwrap().unwrap();
    assert_eq!(value["status"], 200);
    let seen = site.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].uri, "/api/v1/maps/my-map/versions/12/status");
    assert_eq!(seen[0].body, b"{\"status\":\"active\"}");
    assert_eq!(
        seen[0].headers.get("content-type").map(String::as_str),
        Some("application/json; charset=utf-8")
    );

    // The approval was one-shot: the same request asks again.
    let (asking, run, call) = (runtime.clone(), identity.clone(), args.clone());
    let again = tokio::spawn(async move { request(&asking, &run, call).await });
    let card = emitted.recv().await.unwrap();
    assert_eq!(card.kind, crate::ipc::AskKind::MapvaultConfirm);
    answer(&runtime, &card, "거부");
    assert!(again.await.unwrap().is_err());
    assert_eq!(site.seen().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_approve_sends_card_requests_at_once_and_keeps_every_refusal() {
    // Given: Settings → MapVault's auto-approve switch is on.
    let site = Site::start(|_| json_answer(200, json!({"ok": true}))).await;
    let (services, _root, runtime, identity) = session("mv-auto", &site.origin, Some(KEY));
    let mut config = services.dirs.load_config().unwrap();
    config.mapvault.auto_approve = true;
    services.dirs.save_config(&config).unwrap();
    let mut emitted = cards(&runtime);

    // A release, a player reset and an unknown change go out without a card.
    for args in [
        json!({"method": "POST", "path": "/maps/my-map/versions/12/status", "body": "{\"status\":\"active\"}"}),
        json!({"method": "POST", "path": "/maps/m/players/7/reset", "body": "{}"}),
        json!({"method": "DELETE", "path": "/maps/m/everything"}),
    ] {
        let value = request(&runtime, &identity, args.clone()).await.unwrap();
        assert_eq!(value["status"], 200, "{args}");
        assert_eq!(value["autoApproved"], true, "{args}");
    }
    assert!(emitted.try_recv().is_err(), "no card was shown");
    assert!(runtime.pending_ask().is_none());
    assert_eq!(site.seen().len(), 3);

    // A request that never needed a card is not marked.
    let read = request(&runtime, &identity, json!({"method": "GET", "path": "/maps"}))
        .await
        .unwrap();
    assert!(read.get("autoApproved").is_none());

    // SDK downloads stay refused.
    let error = request(
        &runtime,
        &identity,
        json!({"method": "GET", "path": "/maps/m/versions/12/sdk/mapvault.eps"}),
    )
    .await
    .unwrap_err();
    assert!(error.contains("does not download SDK files"), "{error}");
    assert_eq!(site.seen().len(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn player_changes_and_unknown_changes_ask_while_creating_and_dry_runs_do_not() {
    let site = Site::start(|_| json_answer(201, json!({"ok": true}))).await;
    let (_services, _root, runtime, identity) = session("mv-player", &site.origin, Some(KEY));
    let mut emitted = cards(&runtime);
    for (args, irreversible, needle) in [
        (
            json!({"method": "POST", "path": "/maps/m/players/7/reset", "body": "{}"}),
            true,
            "플레이어 id 7",
        ),
        (
            json!({"method": "PATCH", "path": "/maps/m/players/7", "body": "{\"changes\":[{\"key\":\"gold\",\"op\":\"add\",\"amount\":5}]}"}),
            false,
            "\"gold\"",
        ),
        (
            json!({"method": "DELETE", "path": "/maps/m/everything"}),
            false,
            "DELETE /api/v1/maps/m/everything",
        ),
    ] {
        let (asking, run, call) = (runtime.clone(), identity.clone(), args.clone());
        let pending = tokio::spawn(async move { request(&asking, &run, call).await });
        let card = emitted.recv().await.unwrap();
        let shown = format!(
            "{}\n{}",
            card.questions[0].question,
            card.questions[0].preview.as_deref().unwrap_or_default()
        );
        assert_eq!(
            shown.contains("되돌릴 수 없습니다"),
            irreversible,
            "{shown}"
        );
        assert!(shown.contains(needle), "{shown}");
        answer(&runtime, &card, "거부");
        assert!(pending.await.unwrap().is_err());
    }
    assert!(site.seen().is_empty());

    for args in [
        json!({"method": "POST", "path": "/maps", "body": "{\"slug\":\"my-map\",\"name\":\"내 맵\"}"}),
        json!({"method": "POST", "path": "/maps/my-map/versions", "body": "{\"label\":\"v2\",\"copyFrom\":12}"}),
        json!({"method": "PATCH", "path": "/maps/my-map/versions/13", "body": "{\"screenLang\":\"en\"}"}),
        json!({"method": "POST", "path": "/maps/my-map/import", "body": "{\"text\":\"nickname,gold\",\"dryRun\":true}"}),
    ] {
        let value = request(&runtime, &identity, args.clone()).await.unwrap();
        assert_eq!(value["status"], 201, "{args}");
    }
    assert!(runtime.pending_ask().is_none());
    let seen = site.seen();
    assert_eq!(seen.len(), 4);
    assert_eq!(
        String::from_utf8(seen[0].body.clone()).unwrap(),
        "{\"slug\":\"my-map\",\"name\":\"내 맵\"}",
        "the body goes out as the UTF-8 text given"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redirect_is_answered_never_followed() {
    let elsewhere = Site::start(|_| json_answer(200, json!({}))).await;
    let target = format!("{}/steal", elsewhere.origin);
    let site = Site::start(move |_| (302, vec![("location", target.clone())], Vec::new())).await;
    let (_services, _root, runtime, identity) = session("mv-redirect", &site.origin, Some(KEY));
    let value = request(
        &runtime,
        &identity,
        json!({"method": "GET", "path": "/maps"}),
    )
    .await
    .unwrap();
    assert_eq!(value["status"], 302);
    assert_eq!(value["ok"], false);
    assert!(value["note"].as_str().unwrap().contains("Redirects"));
    assert_eq!(site.seen().len(), 1);
    assert!(elsewhere.seen().is_empty(), "the key never left the origin");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upload_needs_a_fresh_build_of_the_output_map_and_sends_it_whole_after_the_card() {
    let site = Site::start(|_| json_answer(201, json!({"id": 12}))).await;
    let (services, root, runtime, identity) = session("mv-upload", &site.origin, Some(KEY));
    let map = b"MPQ\x1a built map bytes".repeat(1000);
    std::fs::create_dir_all(root.join("build")).unwrap();
    std::fs::write(root.join("build/output.scx"), &map).unwrap();
    let upload = json!({
        "method": "POST",
        "path": "/maps/my-map/versions/12/file",
        "file": "build/output.scx",
    });
    let error = request(&runtime, &identity, upload.clone())
        .await
        .unwrap_err();
    assert!(error.contains("build_run"), "{error}");

    let revision = services.native().open().unwrap().revision().unwrap();
    *runtime.last_build_result.lock() = Some(json!({
        "buildProgress": {"inputRevision": revision, "success": true},
    }));
    std::fs::write(root.join("maps/other.scx"), b"x").unwrap();
    let error = request(
        &runtime,
        &identity,
        json!({"method": "POST", "path": "/maps/my-map/versions/12/file", "file": "maps/other.scx"}),
    )
    .await
    .unwrap_err();
    assert!(error.contains("output map"), "{error}");
    let error = request(
        &runtime,
        &identity,
        json!({"method": "POST", "path": "/maps/my-map/versions/12/file", "file": "../outside.scx"}),
    )
    .await
    .unwrap_err();
    assert!(!error.is_empty());
    assert!(site.seen().is_empty());

    let mut emitted = cards(&runtime);
    let (asking, run, call) = (runtime.clone(), identity.clone(), upload.clone());
    let pending = tokio::spawn(async move { request(&asking, &run, call).await });
    let card = emitted.recv().await.unwrap();
    let sha256 = {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(&map))
    };
    let shown = format!(
        "{}\n{}",
        card.questions[0].question,
        card.questions[0].preview.as_deref().unwrap()
    );
    assert!(shown.contains("scmscx.com"), "{shown}");
    assert!(shown.contains("되돌릴 수 없습니다"), "{shown}");
    assert!(shown.contains("build/output.scx"), "{shown}");
    assert!(shown.contains(&sha256), "{shown}");
    answer(&runtime, &card, "허용");
    let value = pending.await.unwrap().unwrap();
    assert_eq!(value["status"], 201);
    let seen = site.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].uri,
        format!("/api/v1/maps/my-map/versions/12/file?filename=output.scx&sha256={sha256}")
    );
    assert_eq!(seen[0].body, map);
    assert_eq!(
        seen[0].headers.get("content-length").map(String::as_str),
        Some(map.len().to_string().as_str())
    );
    assert!(!seen[0].headers.contains_key("transfer-encoding"));
}

fn sdk_site_answer(seen: &Seen) -> Answer {
    let sdk = format!(
        "// MapVault SDK — generated for \"M\" version v1 (versionId 12).\n// DO NOT EDIT. Regenerate from the MapVault site instead. Contains this version's secret key.\n//\n// Usage (main.eps):\n//   import mapvault;\n//   function onPluginStart()     {{ mapvault.init(); }}\n// Player data (index p = player 0..7):\n//   mapvault.d_gold[p]  골드 (0..1000, saved as 1000 if larger)\n\nconst VERSION_ID = 12;\n{SECRET_LINE}\n"
    );
    let text = |body: String| {
        (
            200,
            vec![("content-type", "text/plain; charset=utf-8".to_string())],
            body.into_bytes(),
        )
    };
    match seen.uri.as_str() {
        "/api/v1/maps/my-map/versions/12" => json_answer(
            200,
            json!({"id": 12, "label": "v1", "status": "draft", "screenLang": "ko", "stream": "normal", "cameraLocation": "", "stats": {"bits": 10}, "file": null}),
        ),
        "/api/v1/maps/my-map/versions/12/sdk/mapvault.eps" => text(sdk),
        "/api/v1/maps/my-map/versions/12/sdk/msqc.edd" => text(
            "[eudTurbo]\n\n[MSQC]\nQCDebug: false\nNotTyping ; KeyDown(ESC) : mvCtrl, 0x1\nmvS1 ; val, mvS1 : mvR1\n"
                .to_string(),
        ),
        _ => json_answer(404, json!({"error": "Version not found."})),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sdk_lands_in_src_through_the_source_writer_and_only_a_summary_returns() {
    let site = Site::start(sdk_site_answer).await;
    let (services, root, runtime, identity) = session("mv-sdk", &site.origin, Some(KEY));
    let install = |runtime: SessionToolRuntime,
                   identity: crate::provider_runtime::RunIdentity,
                   args: Value| async move {
        runtime
            .mapvault_for_run(&identity, tools::MAPVAULT_SDK_INSTALL_TOOL, &args)
            .await
    };
    let args = json!({"map": "my-map", "version": 12});
    let value = install(runtime.clone(), identity.clone(), args.clone())
        .await
        .unwrap();
    assert_eq!(value["path"], "src/mapvault.eps");
    assert_eq!(value["created"], true);
    assert_eq!(value["version"]["label"], "v1");
    assert_eq!(value["version"]["screenLang"], "ko");
    assert_eq!(
        value["fields"],
        json!(["mapvault.d_gold[p]  골드 (0..1000, saved as 1000 if larger)"])
    );
    assert_eq!(
        value["usage"],
        json!([
            "import mapvault;",
            "function onPluginStart()     { mapvault.init(); }"
        ])
    );
    assert!(value["plugin"]
        .as_str()
        .unwrap()
        .starts_with("[eudTurbo]\n\n[MSQC]"));
    let shown = value.to_string();
    assert!(
        SECRET_VALUES.iter().all(|value| !shown.contains(value)),
        "{shown}"
    );
    assert!(
        !shown.contains("VERSION_ID") && !shown.contains(KEY),
        "{shown}"
    );
    assert!(runtime.pending_ask().is_none(), "an install asks nothing");

    let written = std::fs::read_to_string(root.join("src/mapvault.eps")).unwrap();
    assert!(written.contains(SECRET_LINE));
    // Written as an agent edit: journaled and claimed for the turn commit.
    let entries = services
        .journal()
        .selected_entries("req-mv", &crate::journal::DecisionIds::All)
        .unwrap();
    assert!(
        entries.iter().any(|entry| matches!(
            &entry.target,
            crate::journal::JournalTarget::Path { path } if path == "src/mapvault.eps"
        )),
        "{entries:?}"
    );
    let scope = services.git_scope().unwrap();
    assert!(services
        .writes()
        .claimed_in_flight(&scope)
        .contains("src/mapvault.eps"));

    // Reading or searching it (or a copy under another name) through any
    // tool shows the API and never the key material.
    std::fs::write(root.join("notes-copy.txt"), &written).unwrap();
    let gate = gate(
        &runtime,
        &identity,
        crate::provider_runtime::WorkspaceAccess::Write,
    );
    for (tool, args) in [
        (tools::FS_READ_TOOL, json!({"path": "src/mapvault.eps"})),
        ("read_file", json!({"path": "src/mapvault.eps"})),
        (tools::FS_READ_TOOL, json!({"path": "notes-copy.txt"})),
        (
            tools::FS_GREP_TOOL,
            json!({"pattern": "rke0|T_KEY|macX|a != 0x"}),
        ),
    ] {
        let result = call(&gate, tool, args.clone()).await;
        let shown = result.result.to_string();
        assert!(!result.is_error, "{tool} {args}: {shown}");
        assert!(
            SECRET_VALUES.iter().all(|value| !shown.contains(value)),
            "{tool} {args}: {shown}"
        );
        assert!(shown.contains("[masked]"), "{tool} {args}: {shown}");
    }
    let read = call(&gate, "read_file", json!({"path": "src/mapvault.eps"})).await;
    let shown = read.result.to_string();
    assert!(
        shown.contains("mapvault.d_gold[p]") && shown.contains("var rke0 = 0x[masked];"),
        "{shown}"
    );

    // A reinstall replaces the whole file through file_write.
    std::fs::write(root.join("src/mapvault.eps"), "// stale\n").unwrap();
    let again = install(runtime.clone(), identity.clone(), args.clone())
        .await
        .unwrap();
    assert_eq!(again["created"], false);
    assert_eq!(
        std::fs::read_to_string(root.join("src/mapvault.eps")).unwrap(),
        written
    );

    // An unknown version writes nothing and says why.
    let error = install(
        runtime.clone(),
        identity.clone(),
        json!({"map": "my-map", "version": 99}),
    )
    .await
    .unwrap_err();
    assert!(
        error.contains("404") && error.contains("Version not found."),
        "{error}"
    );
    for bad in [
        json!({"map": "../x", "version": 12}),
        json!({"map": "my-map", "version": 0}),
    ] {
        assert!(install(runtime.clone(), identity.clone(), bad)
            .await
            .is_err());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_that_is_not_the_sdk_writes_nothing() {
    let site = Site::start(|seen| {
        if seen.uri.ends_with("/sdk/mapvault.eps") {
            (
                200,
                vec![("content-type", "text/plain".to_string())],
                b"<html>login</html>".to_vec(),
            )
        } else {
            sdk_site_answer(seen)
        }
    })
    .await;
    let (_services, root, runtime, identity) = session("mv-sdk-bad", &site.origin, Some(KEY));
    let error = runtime
        .mapvault_for_run(
            &identity,
            tools::MAPVAULT_SDK_INSTALL_TOOL,
            &json!({"map": "my-map", "version": 12}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("nothing was written"), "{error}");
    assert!(!root.join("src/mapvault.eps").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_run_sends_only_reads_and_turns_every_change_into_the_write_transition() {
    // Given: a read-only foreground run (plan feedback, an autonomous
    // triage), whose main EPS session is offered the MapVault tools.
    let site = Site::start(|_| json_answer(200, json!({"maps": []}))).await;
    let (_services, root, runtime, identity) = session("mv-read-turn", &site.origin, Some(KEY));
    runtime.release_write_registration().unwrap();
    assert!(runtime.write_ticket().is_none());
    assert!(runtime
        .tool_descriptors()
        .iter()
        .any(|tool| tool["name"] == tools::MAPVAULT_REQUEST_TOOL));
    let read_gate = gate(
        &runtime,
        &identity,
        crate::provider_runtime::WorkspaceAccess::Read,
    );

    // A GET runs in place.
    let read = call(
        &read_gate,
        tools::MAPVAULT_REQUEST_TOOL,
        json!({"method": "GET", "path": "/maps"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.result);
    assert_eq!(site.seen().len(), 1);
    assert!(!read_gate.has_requested_write_transition());

    // A change that needs no card, and an SDK install, send nothing and ask
    // for the write context like every other writer.
    for (tool, args) in [
        (
            tools::MAPVAULT_REQUEST_TOOL,
            json!({"method": "POST", "path": "/maps", "body": "{\"slug\":\"abc\",\"name\":\"A\"}"}),
        ),
        (
            tools::MAPVAULT_SDK_INSTALL_TOOL,
            json!({"map": "my-map", "version": 12}),
        ),
    ] {
        let gate = gate(
            &runtime,
            &identity,
            crate::provider_runtime::WorkspaceAccess::Read,
        );
        let result = call(&gate, tool, args.clone()).await;
        assert!(result.is_error, "{args}");
        assert!(
            result
                .result
                .as_str()
                .unwrap()
                .starts_with("WriteWorkspaceTransition"),
            "{}",
            result.result
        );
        assert!(gate.has_requested_write_transition());
        runtime.release_write_registration().unwrap();
    }
    assert_eq!(site.seen().len(), 1, "nothing but the GET reached the site");
    assert!(!root.join("src/mapvault.eps").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unanswered_card_refuses_and_sends_nothing() {
    let site = Site::start(|_| json_answer(200, json!({}))).await;
    let (_services, _root, runtime, identity) = session("mv-expire", &site.origin, Some(KEY));
    let mut emitted = cards(&runtime);
    runtime.set_ask_wait_timeout(std::time::Duration::from_millis(200));
    let (asking, run) = (runtime.clone(), identity.clone());
    let pending = tokio::spawn(async move {
        request(
            &asking,
            &run,
            json!({"method": "POST", "path": "/maps/m/players/7/reset", "body": "{}"}),
        )
        .await
    });
    let card = emitted.recv().await.unwrap();
    assert_eq!(card.kind, crate::ipc::AskKind::MapvaultConfirm);
    let error = pending.await.unwrap().unwrap_err();
    assert!(error.contains("did not allow"), "{error}");
    assert!(site.seen().is_empty());
    assert!(runtime.pending_ask().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_card_shows_the_query_duplicate_keys_follow_the_last_and_a_bom_is_refused() {
    let site = Site::start(|_| json_answer(200, json!({}))).await;
    let (_services, _root, runtime, identity) = session("mv-bodies", &site.origin, Some(KEY));
    let mut emitted = cards(&runtime);

    // The query goes on the card as given.
    let (asking, run) = (runtime.clone(), identity.clone());
    let pending = tokio::spawn(async move {
        request(
            &asking,
            &run,
            json!({"method": "POST", "path": "/maps/m/players/7/reset?note=홍길동", "body": "{}"}),
        )
        .await
    });
    let card = emitted.recv().await.unwrap();
    let preview = card.questions[0].preview.clone().unwrap();
    assert!(
        preview.contains("POST /api/v1/maps/m/players/7/reset?note=홍길동"),
        "{preview}"
    );
    answer(&runtime, &card, "거부");
    assert!(pending.await.unwrap().is_err());

    // JSON keeps the last of two equal keys, as the site's JSON.parse does:
    // `blocked` then `active` is a release, and the card says so.
    let body = "{\"status\":\"blocked\",\"status\":\"active\"}";
    let (asking, run) = (runtime.clone(), identity.clone());
    let pending = tokio::spawn(async move {
        request(
            &asking,
            &run,
            json!({"method": "POST", "path": "/maps/m/versions/1/status", "body": body}),
        )
        .await
    });
    let card = emitted.recv().await.unwrap();
    assert!(
        card.questions[0].question.contains("배포"),
        "{}",
        card.questions[0].question
    );
    assert!(card.questions[0].preview.as_deref().unwrap().contains(body));
    answer(&runtime, &card, "허용");
    pending.await.unwrap().unwrap();
    assert_eq!(
        site.seen()[0].body,
        body.as_bytes(),
        "sent exactly as shown"
    );

    // A byte-order mark is not JSON here: refused before anything is sent.
    let error = request(
        &runtime,
        &identity,
        json!({"method": "POST", "path": "/maps", "body": "\u{feff}{\"slug\":\"abc\",\"name\":\"A\"}"}),
    )
    .await
    .unwrap_err();
    assert!(error.contains("JSON request body"), "{error}");
    assert_eq!(site.seen().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn player_data_cards_summarize_long_bodies_with_their_hash() {
    let site = Site::start(|_| json_answer(200, json!({}))).await;
    let (_services, _root, runtime, identity) = session("mv-summary", &site.origin, Some(KEY));
    let mut emitted = cards(&runtime);
    let changes = (0..120)
        .map(|i| json!({"key": format!("item_{}", i % 3), "op": "add", "amount": i}))
        .collect::<Vec<_>>();
    let body = json!({"changes": changes, "note": "보상"}).to_string();
    let csv = std::iter::once("nickname,gold,items[0]".to_string())
        .chain((0..400).map(|i| format!("player{i},{i},1")))
        .collect::<Vec<_>>()
        .join("\n");
    let import = json!({"text": csv}).to_string();
    for (path, body, needles) in [
        (
            "/maps/m/players/7",
            body.clone(),
            vec!["변경 120건", "item_0, item_1, item_2"],
        ),
        (
            "/maps/m/import",
            import.clone(),
            vec!["데이터 400줄", "gold, items[0]"],
        ),
    ] {
        let method = if path.ends_with("import") {
            "POST"
        } else {
            "PATCH"
        };
        let (asking, run, sent) = (runtime.clone(), identity.clone(), body.clone());
        let pending = tokio::spawn(async move {
            request(
                &asking,
                &run,
                json!({"method": method, "path": path, "body": sent}),
            )
            .await
        });
        let card = emitted.recv().await.unwrap();
        let preview = card.questions[0].preview.clone().unwrap();
        let sha = {
            use sha2::Digest;
            format!("{:x}", sha2::Sha256::digest(body.as_bytes()))
        };
        assert!(preview.contains(&sha), "{preview}");
        for needle in needles {
            assert!(preview.contains(needle), "{needle}: {preview}");
        }
        assert!(preview.len() <= 2_000);
        answer(&runtime, &card, "거부");
        assert!(pending.await.unwrap().is_err());
    }
    assert!(site.seen().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upload_too_large_for_one_call_is_refused_before_hashing_or_asking() {
    let site = Site::start(|_| json_answer(201, json!({}))).await;
    let (services, root, runtime, identity) = session("mv-upload-big", &site.origin, Some(KEY));
    std::fs::create_dir_all(root.join("build")).unwrap();
    let file = std::fs::File::create(root.join("build/output.scx")).unwrap();
    file.set_len(250 * 1024 * 1024).unwrap();
    drop(file);
    let revision = services.native().open().unwrap().revision().unwrap();
    *runtime.last_build_result.lock() = Some(json!({
        "buildProgress": {"inputRevision": revision, "success": true},
    }));
    let started = std::time::Instant::now();
    let error = request(
        &runtime,
        &identity,
        json!({"method": "POST", "path": "/maps/m/versions/1/file", "file": "build/output.scx"}),
    )
    .await
    .unwrap_err();
    assert!(
        error.contains("too large to upload within one tool call"),
        "{error}"
    );
    assert!(error.contains("about 185 MB"), "{error}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "not hashed"
    );
    assert!(runtime.pending_ask().is_none(), "no card was shown");
    assert!(site.seen().is_empty());
    assert_eq!(
        crate::mapvault::upload_send_time(10 * 1024 * 1024),
        std::time::Duration::from_secs(30)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_dropped_mid_send_is_named_to_the_next_turn() {
    // The stand-in never answers, so the request is still in flight when
    // the call is dropped, as when the user stops the turn.
    let site = Site::start(|_| {
        std::thread::sleep(std::time::Duration::from_secs(3));
        json_answer(201, json!({}))
    })
    .await;
    let (_services, _root, runtime, identity) = session("mv-dropped", &site.origin, Some(KEY));
    let (sending, run) = (runtime.clone(), identity.clone());
    let pending = tokio::spawn(async move {
        request(
            &sending,
            &run,
            json!({"method": "POST", "path": "/maps", "body": "{\"slug\":\"abc\",\"name\":\"A\"}"}),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    let note = runtime.take_mapvault_note().unwrap();
    assert!(
        note.starts_with("[mapvault requests interrupted]"),
        "{note}"
    );
    assert!(note.contains("`POST /api/v1/maps`"), "{note}");
    assert!(note.contains("GET"), "{note}");
    assert!(runtime.take_mapvault_note().is_none(), "told once");
}

#[test]
fn build_output_never_shows_the_stored_key() {
    let mut services = ToolServices::for_tests();
    services.set_mapvault_key_for_tests(Some(KEY));
    let runtime = services.session("mv-build-redact");
    let mut outcome = Ok(json!({"outputExcerpt": format!("print({KEY})"), "errors": [KEY]}));
    runtime.redact_mapvault_key(&mut outcome);
    assert!(!outcome.unwrap().to_string().contains(KEY));
    let mut failed: Result<Value, String> = Err(format!("euddraft said {KEY}"));
    runtime.redact_mapvault_key(&mut failed);
    assert_eq!(failed.unwrap_err(), "euddraft said [redacted]");
}

#[test]
fn import_cards_count_rows_as_the_site_does_and_edit_cards_count_ops() {
    let rows = |text: &str| {
        let body = json!({"text": text}).to_string();
        super::mapvault_body_summary(
            crate::mapvault::Action::DataImport { overwrite: true },
            &body,
        )
        .unwrap()
    };
    // Lone CR lines (old spreadsheets), a BOM, CRLF and blank lines.
    let shown = rows("nickname,gold\rAlice,5\rBob,7");
    assert!(
        shown.contains("데이터 2줄") && shown.contains("열: gold"),
        "{shown}"
    );
    let shown = rows("\u{feff}nickname,gold,items[0]\r\n\r\nAlice,5,1\r\n  \r\nBob,7,2\n");
    assert!(
        shown.contains("데이터 2줄") && shown.contains("gold, items[0]"),
        "{shown}"
    );
    // Tab-separated when the header holds a tab; quoted cells as the site reads them.
    let shown = rows("nickname\tgold\t\"lv, x\"\nAlice\t5\t1");
    assert!(
        shown.contains("데이터 1줄") && shown.contains("gold, lv, x"),
        "{shown}"
    );
    assert_eq!(
        super::mapvault_csv_cells(" \"a\"\"b\" , c ,,", ','),
        ["a\"b", "c"]
    );

    let edit = json!({"changes": [
        {"key": "gold", "op": "add", "amount": -5},
        {"key": "gold", "op": "add", "amount": 3},
        {"key": "level", "op": "set", "amount": 10},
    ]})
    .to_string();
    let shown = super::mapvault_body_summary(crate::mapvault::Action::PlayerEdit, &edit).unwrap();
    assert!(shown.contains("변경 3건(add 2, set 1)"), "{shown}");
    assert!(shown.contains("필드: gold, level"), "{shown}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn masked_sdk_lines_cannot_be_written_back() {
    let site = Site::start(|_| json_answer(200, json!({}))).await;
    let (_services, root, runtime, identity) = session("mv-masked-write", &site.origin, Some(KEY));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/mapvault.eps"), "var rke0 = 0x5EC2E7A1;\n").unwrap();
    for (tool, args) in [
        (
            tools::FS_WRITE_TOOL,
            json!({"path": "src/mapvault.eps", "content": "var rke0 = 0x[masked];\n"}),
        ),
        (
            tools::FS_EDIT_TOOL,
            json!({"path": "src/mapvault.eps", "edits": [{"old_text": "var rke0 = 0x[masked];", "new_text": "x"}]}),
        ),
        (
            "file_write",
            json!({"path": "src/mapvault.eps", "code": "const T_KEY = EUDArray(py_eval(\"[masked]\"));"}),
        ),
    ] {
        let error = runtime.execute_for_run(&identity, tool, &args).unwrap_err();
        assert!(error.contains("mapvault_sdk_install"), "{tool}: {error}");
    }
    assert_eq!(
        std::fs::read_to_string(root.join("src/mapvault.eps")).unwrap(),
        "var rke0 = 0x5EC2E7A1;\n"
    );
    // An ordinary write is unaffected.
    runtime
        .execute_for_run(
            &identity,
            tools::FS_WRITE_TOOL,
            &json!({"path": "notes/a.md", "content": "masked words [masked] are fine"}),
        )
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_call_in_a_read_turn_is_a_usage_error_not_a_write_transition() {
    let site = Site::start(|_| json_answer(200, json!({}))).await;
    let (_services, _root, runtime, identity) = session("mv-read-usage", &site.origin, Some(KEY));
    runtime.release_write_registration().unwrap();
    let read_gate = gate(
        &runtime,
        &identity,
        crate::provider_runtime::WorkspaceAccess::Read,
    );
    let result = call(
        &read_gate,
        tools::MAPVAULT_REQUEST_TOOL,
        json!({"path": "/maps", "body": "{}"}),
    )
    .await;
    assert!(result.is_error);
    let shown = result.result.to_string();
    assert!(
        shown.contains("method") && !shown.contains("WriteWorkspaceTransition"),
        "{shown}"
    );
    assert!(!read_gate.has_requested_write_transition());
    assert!(site.seen().is_empty());
}
