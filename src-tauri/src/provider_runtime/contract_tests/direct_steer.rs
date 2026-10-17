use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::{
    opencode_go::OpenCodeGoAdapter,
    provider::{ModelCapabilities, ProviderConversationState, ProviderId},
    provider_runtime::{BindingSnapshot, ProviderRuntime, RunOutcome, RuntimeExecutor},
    provider_transcript::{ProviderTranscriptStore, TranscriptBlock, TranscriptBranch},
    steer::{SteerEventKind, SteerMessage, SteerPush},
};

use super::fixtures::RuntimeFixture;

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

fn accepted(events: String) -> ([(axum::http::HeaderName, &'static str); 1], String) {
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        events,
    )
}

fn steer(id: &str, text: &str) -> SteerMessage {
    SteerMessage {
        steer_id: id.to_string(),
        text: text.to_string(),
        attachments: Vec::new(),
        mentions: Value::Null,
        model_text: crate::steer::frame_for_model(text),
        image_paths: Vec::new(),
    }
}

/// A message the user sent while a direct provider's tool batch ran reaches the
/// same request at the next step, after the batch's results, and the
/// checkpoint a later run restores keeps it in that place.
#[tokio::test]
async fn a_steer_joins_the_running_request_after_its_tool_results() {
    let fixture = RuntimeFixture::new("direct-steer");
    let inbox = fixture.tools.steer_inbox();
    let announced = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&announced);
    inbox.set_emitter(move |event| sink.lock().push(event));
    inbox.open();
    let posts = Arc::new(AtomicUsize::new(0));
    let response_posts = Arc::clone(&posts);
    let second_input = Arc::new(Mutex::new(None::<Vec<Value>>));
    let captured = Arc::clone(&second_input);
    let steering_inbox = inbox.clone();
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
                let posts = Arc::clone(&response_posts);
                let captured = Arc::clone(&captured);
                let steering_inbox = steering_inbox.clone();
                async move {
                    match posts.fetch_add(1, Ordering::SeqCst) {
                        0 => {
                            // The user types while the model asks for a tool.
                            assert_eq!(
                                steering_inbox.push(steer("steer-1", "STEER: also add a timer")),
                                SteerPush::Accepted
                            );
                            accepted(concat!(
                                "event: response.output_item.added\n",
                                "data: {\"item\":{\"type\":\"function_call\",\"call_id\":\"look\",\"name\":\"list_files\",\"arguments\":\"{}\"}}\n\n",
                                "event: response.completed\n",
                                "data: {\"response\":{\"status\":\"completed\"}}\n\n"
                            ).to_string())
                        }
                        1 => {
                            *captured.lock() = body["input"].as_array().cloned();
                            accepted(concat!(
                                "event: response.output_text.delta\n",
                                "data: {\"delta\":\"timer added too\"}\n\n",
                                "event: response.completed\n",
                                "data: {\"response\":{\"status\":\"completed\"}}\n\n"
                            ).to_string())
                        }
                        other => panic!("unexpected OpenCode response round {other}"),
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let adapter = OpenCodeGoAdapter::new_for_test(
        reqwest::Client::new(),
        format!("{base}/v1"),
        format!("{base}/v1"),
        format!("{base}/models.dev"),
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
    let outcome = runtime
        .run_foreground(fixture.foreground(binding(), 40_301))
        .await;
    server.abort();

    assert!(
        matches!(outcome, RunOutcome::Completed { ref text, .. } if text == "timer added too"),
        "{outcome:?}"
    );
    let input = second_input
        .lock()
        .clone()
        .expect("the second step was sent");
    let result_at = input
        .iter()
        .position(|item| item["type"] == "function_call_output")
        .expect("the tool result is in the second step");
    let steer_at = input
        .iter()
        .position(|item| item.to_string().contains("STEER: also add a timer"))
        .expect("the steer is in the second step");
    assert!(
        result_at < steer_at,
        "the steer must follow the tool result: {input:?}"
    );
    assert!(input[steer_at]
        .to_string()
        .contains("[user message during this turn]"));
    {
        let announced = announced.lock();
        assert_eq!(announced.len(), 1);
        assert_eq!(announced[0].kind, SteerEventKind::Applied);
        assert_eq!(announced[0].steers[0].steer_id, "steer-1");
    }
    assert!(inbox.drain().is_empty());

    let revision = match runtime.conversation_state() {
        ProviderConversationState::OpencodeGo {
            transcript_revision,
        } => transcript_revision,
        other => panic!("unexpected direct conversation: {other:?}"),
    };
    let restored = ProviderTranscriptStore::new(&fixture.dirs)
        .restore(
            ProviderId::OpencodeGo,
            &fixture.session_id,
            revision,
            &TranscriptBranch {
                instruction_epoch: 3,
                task_leaf_id: None,
            },
        )
        .unwrap()
        .unwrap();
    let shape = restored
        .generation
        .checkpoint
        .blocks
        .iter()
        .map(|block| match block {
            TranscriptBlock::User { text, .. } if text.contains("STEER") => "steer",
            TranscriptBlock::User { .. } => "user",
            TranscriptBlock::ToolCall { .. } => "call",
            TranscriptBlock::ToolResult { .. } => "result",
            TranscriptBlock::AssistantText { .. } => "text",
            TranscriptBlock::AssistantReasoning { .. } => "reasoning",
            TranscriptBlock::Compaction { .. } => "compaction",
        })
        .collect::<Vec<_>>();
    assert_eq!(shape, vec!["user", "call", "result", "steer", "text"]);
}
