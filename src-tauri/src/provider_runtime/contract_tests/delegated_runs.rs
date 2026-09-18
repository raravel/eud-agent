use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use serde_json::{json, Value};

use crate::{
    opencode_go::OpenCodeGoAdapter,
    provider::{ModelCapabilities, ProviderConversationState, ProviderId},
    provider_runtime::{
        BindingSnapshot, DelegatedRunExecutor, DelegatedRunKind, DelegatedRunOutcome,
        DelegatedRunRequest, ProviderRuntimeError, RunPolicy,
    },
    provider_tool_loop::{DelegatedToolProfile, SUBMIT_RESULT_TOOL},
};

use super::fixtures::{EventSummary, RuntimeFixture};

fn binding() -> BindingSnapshot {
    BindingSnapshot {
        provider: ProviderId::OpencodeGo,
        model: "fixture-responses".to_string(),
        reasoning: None,
        base_url: None,
        capabilities: Some(ModelCapabilities {
            tool_calls: true,
            ..ModelCapabilities::default()
        }),
        conversation: ProviderConversationState::empty(ProviderId::OpencodeGo),
    }
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "files": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["summary", "files"],
        "additionalProperties": false
    })
}

fn request(fixture: &RuntimeFixture, run: u64, rounds: usize) -> DelegatedRunRequest {
    DelegatedRunRequest {
        identity: fixture.identity(run),
        parent_run_id: None,
        binding: binding(),
        kind: DelegatedRunKind::Research,
        prompt: "inspect the project".to_string(),
        workspace_root: fixture.root.join("project"),
        workspace_temp: None,
        output_schema: schema(),
        profile: DelegatedToolProfile::new(["list_files", "read_file"], &schema()).unwrap(),
        allow_live_write_ticket: false,
        policy: RunPolicy {
            active_deadline: Some(Duration::from_secs(10)),
            shutdown_grace: Duration::from_secs(1),
            max_output_bytes: 64 * 1024,
            max_output_tokens: Some(4_096),
            max_tool_rounds: rounds,
            allow_resume: false,
        },
    }
}

fn accepted(events: String) -> ([(axum::http::HeaderName, &'static str); 1], String) {
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        events,
    )
}

fn function_call(call_id: &str, name: &str, arguments: &Value) -> String {
    let arguments = serde_json::to_string(arguments).unwrap();
    let item = json!({
        "item": {"type": "function_call", "call_id": call_id, "name": name, "arguments": arguments}
    });
    format!("event: response.output_item.added\ndata: {item}\n\n{COMPLETED}")
}

const COMPLETED: &str =
    "event: response.completed\ndata: {\"response\":{\"status\":\"completed\"}}\n\n";

fn text_completed(text: &str) -> String {
    format!(
        "event: response.output_text.delta\ndata: {}\n\n{COMPLETED}",
        json!({"delta": text})
    )
}

/// Serve a scripted Responses wire. `script(round, body)` returns the SSE body
/// for that round; every request body is retained for assertions.
async fn serve(
    script: impl Fn(usize, &Value) -> String + Send + Sync + 'static,
) -> (String, tokio::task::JoinHandle<()>, Arc<Mutex<Vec<Value>>>) {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    let rounds = Arc::new(AtomicUsize::new(0));
    let script = Arc::new(script);
    let app = axum::Router::new()
        .route(
            "/models.dev",
            axum::routing::get(|| async {
                axum::Json(json!({"opencode-go": {
                    "npm": "@ai-sdk/openai-compatible",
                    "models": {"fixture-responses": {
                        "provider": {"npm": "@ai-sdk/openai"},
                        "tool_call": true
                    }}
                }}))
            }),
        )
        .route(
            "/v1/models",
            axum::routing::get(|| async {
                axum::Json(json!({"data": [{"id": "fixture-responses"}]}))
            }),
        )
        .route(
            "/v1/responses",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let seen = Arc::clone(&seen);
                let rounds = Arc::clone(&rounds);
                let script = Arc::clone(&script);
                async move {
                    let round = rounds.fetch_add(1, Ordering::SeqCst);
                    let events = script(round, &body);
                    seen.lock().unwrap().push(body);
                    accepted(events)
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, server, bodies)
}

fn executor(fixture: &RuntimeFixture, base: &str) -> DelegatedRunExecutor {
    let adapter = OpenCodeGoAdapter::new_for_test(
        reqwest::Client::new(),
        format!("{base}/v1"),
        format!("{base}/v1"),
        format!("{base}/models.dev"),
        "fixture-key".to_string(),
    );
    DelegatedRunExecutor::new(
        Box::new(adapter),
        fixture.tools.clone(),
        fixture.events.clone(),
        fixture.cancellation.subscribe(),
    )
}

fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect()
}

fn function_outputs(body: &Value) -> Vec<Value> {
    body["input"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "function_call_output")
        .cloned()
        .collect()
}

#[tokio::test]
async fn delegated_run_reads_twice_then_submits_a_schema_result() {
    // Given: a scripted model that lists, reads, then submits.
    let fixture = RuntimeFixture::new("delegated-reads");
    let (base, server, bodies) = serve(|round, body| match round {
        0 => function_call("call-list", "list_files", &json!({})),
        1 => {
            let outputs = function_outputs(body);
            assert_eq!(outputs.len(), 1);
            assert!(outputs[0]["output"]
                .as_str()
                .unwrap()
                .contains("src/main.eps"));
            function_call("call-read", "read_file", &json!({"path": "src/main.eps"}))
        }
        2 => {
            let outputs = function_outputs(body);
            assert_eq!(outputs.len(), 2);
            assert!(outputs[1]["output"]
                .as_str()
                .unwrap()
                .contains("onPluginStart"));
            function_call(
                "call-submit",
                SUBMIT_RESULT_TOOL,
                &json!({"summary": "one entry module", "files": ["src/main.eps"]}),
            )
        }
        other => panic!("unexpected round {other}"),
    })
    .await;
    let mut executor = executor(&fixture, &base);

    // When: the delegated run executes.
    let outcome = executor.run(request(&fixture, 50_001, 8)).await;
    server.abort();

    // Then: the schema result is returned after the submission round, with no
    // fourth model step and the advertised tools limited to the profile.
    assert_eq!(
        outcome,
        DelegatedRunOutcome::Result {
            value: json!({"summary": "one entry module", "files": ["src/main.eps"]}),
            completions: 3,
            usage: None,
        }
    );
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert_eq!(
        tool_names(&bodies[0]),
        ["list_files", "read_file", SUBMIT_RESULT_TOOL]
    );
    assert!(!fixture.tools.owns_write_registration());
}

#[tokio::test]
async fn delegated_run_write_tool_is_fatal_and_registers_nothing() {
    let fixture = RuntimeFixture::new("delegated-write");
    // Satisfy the evidence gate so the only thing standing between the model
    // and the mutation is the delegated profile.
    fixture
        .tools
        .execute("search_docs", &json!({"query": "evidence"}))
        .unwrap();
    let (base, server, bodies) = serve(|round, _| match round {
        0 => function_call(
            "call-create",
            "file_create",
            &json!({"path": "src/x.eps", "ftype": "CUIEps", "code": "// x\n"}),
        ),
        other => panic!("unexpected round {other}"),
    })
    .await;
    let mut executor = executor(&fixture, &base);

    let outcome = executor.run(request(&fixture, 50_002, 8)).await;
    server.abort();

    assert!(
        matches!(
            &outcome,
            DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(message))
                if message.contains("unknown tool 'file_create'")
        ),
        "write tool must be a fatal admission error: {outcome:?}"
    );
    assert_eq!(bodies.lock().unwrap().len(), 1);
    assert!(!fixture.tools.owns_write_registration());
    assert!(!fixture.root.join("project/src/x.eps").exists());
    assert_eq!(fixture.tools.journal().entry_count(&fixture.request_id), 0);
}

#[tokio::test]
async fn delegated_run_refuses_a_session_with_a_live_write_ticket() {
    let fixture = RuntimeFixture::new("delegated-ticket");
    fixture
        .tools
        .register_write_request("foreground write in progress")
        .unwrap();
    let (base, server, bodies) =
        serve(|round, _| panic!("no model step expected, got {round}")).await;
    let mut executor = executor(&fixture, &base);

    let outcome = executor.run(request(&fixture, 50_007, 8)).await;
    server.abort();

    assert_eq!(
        outcome,
        DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
            "delegated run refused: the session holds a live write ticket".into()
        ))
    );
    assert!(bodies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn delegated_repeated_schema_violations_exhaust_without_a_result() {
    let fixture = RuntimeFixture::new("delegated-violations");
    let (base, server, bodies) = serve(|round, _| match round {
        0 | 1 => function_call(
            &format!("bad-{round}"),
            SUBMIT_RESULT_TOOL,
            &json!({"summary": round}),
        ),
        other => panic!("unexpected round {other}"),
    })
    .await;
    let mut executor = executor(&fixture, &base);

    let outcome = executor.run(request(&fixture, 50_008, 2)).await;
    server.abort();

    assert_eq!(
        outcome,
        DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
            "delegated run exhausted its tool rounds without submit_result".into()
        ))
    );
    assert_eq!(bodies.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn delegated_pause_request_during_a_run_cancels_it() {
    let fixture = RuntimeFixture::new("delegated-pause");
    let tools = fixture.tools.clone();
    let (base, server, bodies) = serve(move |round, _| match round {
        0 => {
            // The user pauses the session while the model is still working.
            tools.set_autonomous_pause_requested(true);
            function_call("call-list", "list_files", &json!({}))
        }
        other => panic!("unexpected round {other}"),
    })
    .await;
    let mut executor = executor(&fixture, &base);

    let outcome = executor.run(request(&fixture, 50_009, 8)).await;
    server.abort();

    assert_eq!(outcome, DelegatedRunOutcome::Cancelled);
    assert_eq!(bodies.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn delegated_run_prose_without_submission_fails_distinctly() {
    let fixture = RuntimeFixture::new("delegated-prose");
    let (base, server, _) = serve(|round, _| match round {
        0 => text_completed("here is my answer in prose"),
        other => panic!("unexpected round {other}"),
    })
    .await;
    let mut executor = executor(&fixture, &base);

    let outcome = executor.run(request(&fixture, 50_003, 8)).await;
    server.abort();

    assert_eq!(
        outcome,
        DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
            "delegated run ended without submit_result".into()
        ))
    );
    // The run's own prose never enters the session stream.
    assert!(!fixture
        .events
        .snapshot()
        .iter()
        .any(|event| matches!(event, EventSummary::Text(_) | EventSummary::Reasoning(_))));
}

#[tokio::test]
async fn delegated_submission_schema_violation_is_correctable() {
    let fixture = RuntimeFixture::new("delegated-schema");
    let (base, server, bodies) = serve(|round, body| match round {
        0 => function_call("call-bad", SUBMIT_RESULT_TOOL, &json!({"summary": 7})),
        1 => {
            let outputs = function_outputs(body);
            assert_eq!(outputs.len(), 1);
            let output = outputs[0]["output"].as_str().unwrap();
            assert!(
                output.contains("summary"),
                "usage error names the field: {output}"
            );
            function_call(
                "call-good",
                SUBMIT_RESULT_TOOL,
                &json!({"summary": "fixed", "files": []}),
            )
        }
        other => panic!("unexpected round {other}"),
    })
    .await;
    let mut executor = executor(&fixture, &base);

    let outcome = executor.run(request(&fixture, 50_004, 8)).await;
    server.abort();

    assert_eq!(
        outcome,
        DelegatedRunOutcome::Result {
            value: json!({"summary": "fixed", "files": []}),
            completions: 2,
            usage: None,
        }
    );
    assert_eq!(bodies.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn delegated_round_exhaustion_fails_without_a_partial_result() {
    let fixture = RuntimeFixture::new("delegated-rounds");
    let (base, server, bodies) = serve(|round, _| match round {
        0 | 1 => function_call(&format!("call-{round}"), "list_files", &json!({})),
        other => panic!("unexpected round {other}"),
    })
    .await;
    let mut executor = executor(&fixture, &base);

    let outcome = executor.run(request(&fixture, 50_005, 2)).await;
    server.abort();

    assert_eq!(
        outcome,
        DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
            "delegated run exhausted its tool rounds without submit_result".into()
        ))
    );
    assert_eq!(bodies.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn delegated_run_cancellation_returns_within_grace() {
    let fixture = RuntimeFixture::new("delegated-cancel");
    let cancellation = fixture.cancellation.clone();
    let (base, server, _) = serve(move |round, _| match round {
        0 => function_call("call-list", "list_files", &json!({})),
        1 => {
            cancellation.send_replace(1);
            function_call("call-read", "read_file", &json!({"path": "src/main.eps"}))
        }
        other => panic!("unexpected round {other}"),
    })
    .await;
    let mut executor = executor(&fixture, &base);

    let started = tokio::time::Instant::now();
    let outcome = executor.run(request(&fixture, 50_006, 8)).await;
    server.abort();

    assert_eq!(outcome, DelegatedRunOutcome::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!fixture.tools.owns_write_registration());
}

// ---- Ollama and Antigravity: the same executor over scripted direct wires ----

use super::fixtures::{request_body, sse, HttpFixture};
use crate::{
    antigravity_auth::{AntigravityAuthHandle, AntigravityCredential},
    antigravity_client::AntigravityAdapter,
    ollama::ProductionOllamaAdapter,
};

fn json_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn ollama_tool_call(id: &str, name: &str, arguments: &Value) -> String {
    let arguments = serde_json::to_string(&serde_json::to_string(arguments).unwrap()).unwrap();
    format!(
        concat!(
            "data: {{\"id\":\"response-{id}\",\"choices\":[{{\"delta\":{{\"tool_calls\":[",
            "{{\"index\":0,\"id\":\"{id}\",\"function\":{{\"name\":\"{name}\",\"arguments\":{arguments}}}}}]}},",
            "\"finish_reason\":\"tool_calls\"}}]}}\n\n",
            "data: [DONE]\n\n"
        ),
        id = id,
        name = name,
        arguments = arguments
    )
}

fn antigravity_tool_call(id: &str, name: &str, arguments: &Value) -> String {
    format!(
        concat!(
            "data: {{\"response\":{{\"candidates\":[{{\"content\":{{\"parts\":[",
            "{{\"functionCall\":{{\"id\":\"{id}\",\"name\":\"{name}\",\"args\":{args}}}}}]}},",
            "\"finishReason\":\"STOP\"}}]}}}}\n\n"
        ),
        id = id,
        name = name,
        args = arguments
    )
}

fn ollama_request(fixture: &RuntimeFixture, base_url: &str, run: u64) -> DelegatedRunRequest {
    let mut request = request(fixture, run, 8);
    request.binding = fixture.binding(base_url);
    request
}

#[tokio::test]
async fn ollama_delegated_run_reads_twice_then_submits() {
    let submit = json!({"summary": "one entry module", "files": ["src/main.eps"]});
    let http = HttpFixture::scripted([
        sse(&ollama_tool_call("call-list", "list_files", &json!({}))),
        sse(&ollama_tool_call(
            "call-read",
            "read_file",
            &json!({"path": "src/main.eps"}),
        )),
        sse(&ollama_tool_call(
            "call-submit",
            SUBMIT_RESULT_TOOL,
            &submit,
        )),
    ]);
    let fixture = RuntimeFixture::new("delegated-ollama");
    let mut executor = DelegatedRunExecutor::new(
        Box::new(ProductionOllamaAdapter::new(None).unwrap()),
        fixture.tools.clone(),
        fixture.events.clone(),
        fixture.cancellation.subscribe(),
    );

    let outcome = executor
        .run(ollama_request(&fixture, &http.base_url, 52_001))
        .await;

    assert_eq!(
        outcome,
        DelegatedRunOutcome::Result {
            value: submit,
            completions: 3,
            usage: None,
        }
    );
    let first = request_body(&http.requests.recv().unwrap());
    let names = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(names, ["list_files", "read_file", SUBMIT_RESULT_TOOL]);
    let third = request_body(
        &http
            .requests
            .recv()
            .and_then(|_| http.requests.recv())
            .unwrap(),
    );
    let tool_ids = third["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["tool_call_id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(tool_ids, ["call-list", "call-read"]);
    http.join();
    assert!(!fixture.tools.owns_write_registration());
}

#[tokio::test]
async fn antigravity_delegated_run_reads_twice_then_submits() {
    let submit = json!({"summary": "one entry module", "files": ["src/main.eps"]});
    let catalog = json_response(
        &json!({
            "models": {
                "fixture-model": {
                    "displayName": "Fixture",
                    "supportsImages": true,
                    "supportsThinking": true,
                    "thinkingBudget": 64,
                    "maxOutputTokens": 16_384
                }
            }
        })
        .to_string(),
    );
    let http = HttpFixture::scripted([
        catalog,
        sse(&antigravity_tool_call(
            "call-list",
            "list_files",
            &json!({}),
        )),
        sse(&antigravity_tool_call(
            "call-read",
            "read_file",
            &json!({"path": "src/main.eps"}),
        )),
        sse(&antigravity_tool_call(
            "call-submit",
            SUBMIT_RESULT_TOOL,
            &submit,
        )),
    ]);
    let fixture = RuntimeFixture::new("delegated-antigravity");
    let credential = AntigravityCredential {
        access_token: "fixture-token".into(),
        refresh_token: "fixture-refresh".into(),
        expires_at: u64::MAX,
        granted_scopes: Vec::new(),
        project_id: "fixture-project".into(),
    };
    let adapter = AntigravityAdapter::with_transport(
        "fixture-model".into(),
        AntigravityAuthHandle::fixed(credential),
        reqwest::Client::new(),
        http.base_url.clone(),
    )
    .unwrap();
    let mut executor = DelegatedRunExecutor::new(
        Box::new(adapter),
        fixture.tools.clone(),
        fixture.events.clone(),
        fixture.cancellation.subscribe(),
    );
    let mut request = request(&fixture, 52_002, 8);
    request.binding = fixture.binding(&http.base_url);
    request.binding.provider = ProviderId::Antigravity;
    request.binding.reasoning = None;
    request.binding.base_url = None;
    request.binding.conversation = ProviderConversationState::Antigravity {
        transcript_revision: 0,
    };

    let outcome = executor.run(request).await;

    assert_eq!(
        outcome,
        DelegatedRunOutcome::Result {
            value: submit,
            completions: 3,
            usage: None,
        }
    );
    let _catalog = http.requests.recv().unwrap();
    let first = request_body(&http.requests.recv().unwrap());
    let names = first["request"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|tool| {
            tool["functionDeclarations"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|declaration| declaration["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(names, ["list_files", "read_file", SUBMIT_RESULT_TOOL]);
    http.join();
    assert!(!fixture.tools.owns_write_registration());
}
