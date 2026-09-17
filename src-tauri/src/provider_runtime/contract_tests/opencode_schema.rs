use super::*;
use crate::{
    opencode_go::OpenCodeGoAdapter,
    provider::{ModelCapabilities, ProviderConversationState, ProviderId},
    provider_runtime::BindingSnapshot,
};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

struct SchemaServer {
    base: String,
    posts: Arc<AtomicUsize>,
    saw_actual_result: Arc<AtomicBool>,
    saw_usage_error: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for SchemaServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn schema_server(invalid_arguments: bool) -> SchemaServer {
    use axum::routing::{get, post};

    let posts = Arc::new(AtomicUsize::new(0));
    let saw_actual_result = Arc::new(AtomicBool::new(false));
    let saw_usage_error = Arc::new(AtomicBool::new(false));
    let response_posts = Arc::clone(&posts);
    let response_saw_result = Arc::clone(&saw_actual_result);
    let response_saw_usage = Arc::clone(&saw_usage_error);
    let app = axum::Router::new()
        .route(
            "/models.dev",
            get(|| async {
                axum::Json(json!({"opencode-go":{
                    "npm":"@ai-sdk/openai-compatible",
                    "models":{"fixture-responses":{
                        "provider":{"npm":"@ai-sdk/openai"},
                        "tool_call":true
                    }}
                }}))
            }),
        )
        .route(
            "/v1/models",
            get(|| async { axum::Json(json!({"data":[{"id":"fixture-responses"}]})) }),
        )
        .route(
            "/v1/responses",
            post(move |axum::Json(body): axum::Json<Value>| {
                let posts = Arc::clone(&response_posts);
                let saw_result = Arc::clone(&response_saw_result);
                let saw_usage = Arc::clone(&response_saw_usage);
                async move {
                    let read_file = body["tools"]
                        .as_array()
                        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "read_file"));
                    let valid_schema = read_file.is_some_and(|tool| {
                        tool.get("strict") == Some(&Value::Bool(false))
                            && tool["parameters"]["required"] == json!(["path"])
                            && tool["parameters"]["additionalProperties"] == false
                            && tool["parameters"]["properties"]["startLine"]["type"] == "integer"
                            && tool["parameters"]["properties"]["endLine"]["type"] == "integer"
                    });
                    if !valid_schema {
                        return (
                            axum::http::StatusCode::BAD_REQUEST,
                            [("content-type", "application/json")],
                            json!({"error":{
                                "type":"invalid_request_error",
                                "param":"tools[2].parameters"
                            }})
                            .to_string(),
                        );
                    }
                    posts.fetch_add(1, Ordering::SeqCst);
                    let output = body["input"].as_array().and_then(|input| {
                        input
                            .iter()
                            .find(|item| item["type"] == "function_call_output")
                            .and_then(|item| item["output"].as_str())
                    });
                    let stream = if let Some(output) = output {
                        saw_result.store(output.contains("onPluginStart"), Ordering::SeqCst);
                        saw_usage.store(
                            output.contains("Usage: read_file(path)")
                                && output
                                    .contains("arguments do not match the documented input schema"),
                            Ordering::SeqCst,
                        );
                        concat!(
                            "event: response.output_text.delta\n",
                            "data: {\"delta\":\"done\"}\n\n",
                            "event: response.completed\n",
                            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
                        )
                        .to_string()
                    } else {
                        let arguments = if invalid_arguments {
                            r#"{"startLine":1}"#
                        } else {
                            r#"{"path":"src/main.eps"}"#
                        };
                        format!(
                            concat!(
                                "event: response.output_item.added\n",
                                "data: {{\"item\":{{\"type\":\"function_call\",",
                                "\"call_id\":\"read-1\",\"name\":\"read_file\",",
                                "\"arguments\":{arguments:?}}}}}\n\n",
                                "event: response.completed\n",
                                "data: {{\"response\":{{\"status\":\"completed\"}}}}\n\n"
                            ),
                            arguments = arguments
                        )
                    };
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "text/event-stream")],
                        stream,
                    )
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    SchemaServer {
        base,
        posts,
        saw_actual_result,
        saw_usage_error,
        task,
    }
}

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

async fn runtime_outcome(fixture: &RuntimeFixture, server: &SchemaServer, run: u64) -> RunOutcome {
    let adapter = OpenCodeGoAdapter::new_for_test(
        reqwest::Client::new(),
        format!("{}/v1", server.base),
        format!("{}/v1", server.base),
        format!("{}/models.dev", server.base),
        "fixture-key".to_string(),
    );
    let mut runtime = ProviderRuntime::new(
        Box::new(adapter),
        binding(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        fixture.events.clone(),
    )
    .unwrap();
    runtime
        .run_foreground(fixture.foreground(binding(), run))
        .await
}

#[tokio::test]
async fn opencode_optional_schema_roundtrips_through_runtime_admission() {
    let valid_fixture = RuntimeFixture::new("opencode-optional-schema-valid");
    let valid_server = schema_server(false).await;
    let valid = runtime_outcome(&valid_fixture, &valid_server, 40_101).await;
    assert!(
        matches!(&valid, RunOutcome::Completed { text, .. } if text == "done"),
        "{valid:?}"
    );
    assert_eq!(valid_server.posts.load(Ordering::SeqCst), 2);
    assert!(valid_server.saw_actual_result.load(Ordering::SeqCst));

    // A schema violation is a recoverable usage completion: the exact guidance
    // reaches the model, the run continues, and no fatal protocol failure ends it.
    let invalid_fixture = RuntimeFixture::new("opencode-optional-schema-invalid");
    let invalid_server = schema_server(true).await;
    let invalid = runtime_outcome(&invalid_fixture, &invalid_server, 40_102).await;
    assert!(
        matches!(&invalid, RunOutcome::Completed { text, .. } if text == "done"),
        "{invalid:?}"
    );
    assert_eq!(invalid_server.posts.load(Ordering::SeqCst), 2);
    assert!(invalid_server.saw_usage_error.load(Ordering::SeqCst));
    assert!(!invalid_server.saw_actual_result.load(Ordering::SeqCst));
}
