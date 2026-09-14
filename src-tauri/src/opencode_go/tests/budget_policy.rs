use super::*;

#[derive(Clone)]
struct WireServer {
    model: String,
    output_cap: u64,
    wire: OpenCodeGoWire,
    requests: tokio::sync::mpsc::UnboundedSender<Value>,
}

fn npm_for(wire: OpenCodeGoWire) -> &'static str {
    match wire {
        OpenCodeGoWire::Responses => "@ai-sdk/openai",
        OpenCodeGoWire::ChatCompletions => "@ai-sdk/openai-compatible",
        OpenCodeGoWire::AnthropicMessages => "@ai-sdk/anthropic",
    }
}

fn inference_path(wire: OpenCodeGoWire) -> &'static str {
    match wire {
        OpenCodeGoWire::Responses => "/v1/responses",
        OpenCodeGoWire::ChatCompletions => "/v1/chat/completions",
        OpenCodeGoWire::AnthropicMessages => "/v1/messages",
    }
}

async fn live_models(
    axum::extract::State(state): axum::extract::State<WireServer>,
) -> axum::Json<Value> {
    axum::Json(json!({"data":[{"id":state.model}]}))
}

async fn models_metadata(
    axum::extract::State(state): axum::extract::State<WireServer>,
) -> axum::Json<Value> {
    let mut models = serde_json::Map::new();
    models.insert(
        state.model.clone(),
        json!({
            "tool_call":true,
            "structured_output":true,
            "limit":{"output":state.output_cap}
        }),
    );
    axum::Json(json!({"opencode-go":{
        "npm":npm_for(state.wire),
        "models":models
    }}))
}

async fn capture_structured_request(
    axum::extract::State(state): axum::extract::State<WireServer>,
    axum::Json(body): axum::Json<Value>,
) -> String {
    state.requests.send(body).unwrap();
    match state.wire {
        OpenCodeGoWire::Responses => concat!(
            "event: response.output_item.added\n",
            "data: {\"item\":{\"type\":\"function_call\",\"call_id\":\"structured\",\"name\":\"submit_structured_result\",\"arguments\":\"{\\\"ok\\\":true}\"}}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
        )
        .to_string(),
        OpenCodeGoWire::ChatCompletions => concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"structured\",\"function\":{\"name\":\"submit_structured_result\",\"arguments\":\"{\\\"ok\\\":true}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string(),
        OpenCodeGoWire::AnthropicMessages => concat!(
            "event: content_block_start\n",
            "data: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"structured\",\"name\":\"submit_structured_result\",\"input\":{\"ok\":true}}}\n\n",
            "event: message_delta\n",
            "data: {\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n"
        )
        .to_string(),
    }
}

fn compiler_request(
    model: &str,
    output_cap: Option<u64>,
    cancellation: tokio::sync::watch::Receiver<u64>,
) -> AdapterStepRequest {
    let mut request = fixture_request(model, Vec::new(), cancellation);
    request.kind = AdapterRequestKind::Structured {
        kind: crate::provider_runtime::StructuredJobKind::TaskStateCompiler,
        prompt: "compile".to_string(),
        workspace_root: std::path::PathBuf::from("fixture"),
        output_schema: json!({
            "type":"object",
            "required":["ok"],
            "properties":{"ok":{"type":"boolean"}},
            "additionalProperties":false
        }),
    };
    request.policy.max_output_tokens = output_cap;
    request
}

#[tokio::test]
async fn production_adapter_translates_compiler_policy_against_each_wire_model_cap() {
    for wire in [
        OpenCodeGoWire::Responses,
        OpenCodeGoWire::ChatCompletions,
        OpenCodeGoWire::AnthropicMessages,
    ] {
        for (model_cap, caller_cap, expected) in [
            (4_096, Some(12_000), 4_096),
            (16_384, Some(4_096), 4_096),
            (16_384, Some(8_192), 8_192),
            (16_384, Some(12_000), 12_000),
            (16_384, None, 16_384),
        ] {
            let model = format!("fixture-{wire:?}-{model_cap}-{caller_cap:?}");
            let (requests, mut captured) = tokio::sync::mpsc::unbounded_channel();
            let state = WireServer {
                model: model.clone(),
                output_cap: model_cap,
                wire,
                requests,
            };
            let app = axum::Router::new()
                .route("/v1/models", axum::routing::get(live_models))
                .route("/models.dev", axum::routing::get(models_metadata))
                .route(
                    inference_path(wire),
                    axum::routing::post(capture_structured_request),
                )
                .with_state(state);
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let base = format!("http://{address}");
            let mut adapter = OpenCodeGoAdapter::new_for_test(
                reqwest::Client::new(),
                format!("{base}/v1"),
                format!("{base}/v1"),
                format!("{base}/models.dev"),
                "fixture-key".to_string(),
            );
            let (_cancel, cancellation) = tokio::sync::watch::channel(0);
            let (events, _received) = tokio::sync::mpsc::channel(16);
            let outcome = adapter
                .run_step(compiler_request(&model, caller_cap, cancellation), events)
                .await
                .unwrap();
            assert!(matches!(
                outcome,
                AdapterStepOutcome::Completed {
                    output: AdapterOutput::Structured(ref value),
                    ..
                } if value == &json!({"ok":true})
            ));
            let body = captured.recv().await.unwrap();
            let wire_limit = match wire {
                OpenCodeGoWire::Responses => body.get("max_output_tokens"),
                OpenCodeGoWire::ChatCompletions | OpenCodeGoWire::AnthropicMessages => {
                    body.get("max_tokens")
                }
            };
            assert_eq!(wire_limit.and_then(Value::as_u64), Some(expected));
            server.abort();
        }
    }
}
