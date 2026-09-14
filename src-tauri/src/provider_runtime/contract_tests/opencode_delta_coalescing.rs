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
    provider_transcript::{
        CheckpointBoundary, ProviderTranscriptStore, TranscriptBlock, TranscriptBranch,
    },
};

use super::fixtures::{EventSummary, RuntimeFixture};

const DELTA_COUNT: usize = 5_000;
const RUN_ID: u64 = 42_001;
const FIRST_RESPONSE_ID: &str = "42001-1";
const FINAL_RESPONSE_ID: &str = "42001-4";
const TOOL_BATCH_ID: &str = "42001-1-tools";
const TOOL_CALL_ID: &str = "delta-read";

fn binding() -> BindingSnapshot {
    BindingSnapshot {
        provider: ProviderId::OpencodeGo,
        model: "fixture-chat".to_string(),
        reasoning: None,
        base_url: None,
        capabilities: Some(ModelCapabilities {
            tool_calls: true,
            ..ModelCapabilities::default()
        }),
        conversation: ProviderConversationState::empty(ProviderId::OpencodeGo),
    }
}

fn reasoning_deltas() -> Vec<String> {
    (0..DELTA_COUNT)
        .map(|index| format!("r{index:04};"))
        .collect()
}

fn reasoning_tool_stream() -> String {
    let mut events = String::new();
    for (index, delta) in reasoning_deltas().into_iter().enumerate() {
        if index == DELTA_COUNT / 2 {
            events.push_str("data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"\"}}]}\n\n");
        }
        events.push_str("data: ");
        events.push_str(&json!({"choices":[{"delta":{"reasoning_content":delta}}]}).to_string());
        events.push_str("\n\n");
    }
    events.push_str(concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[",
        "{\"index\":0,\"id\":\"delta-read\",\"function\":{\"name\":\"read_file\",",
        "\"arguments\":\"{\\\"path\\\":\\\"src/main.eps\\\"}\"}}]},",
        "\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    ));
    events
}

fn accepted(events: String) -> ([(axum::http::HeaderName, &'static str); 1], String) {
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        events,
    )
}

#[tokio::test]
async fn opencode_chat_streams_transport_deltas_but_checkpoints_semantic_blocks() {
    // Given: the production Chat Completions codec receives 5,000 fragmented reasoning deltas,
    // an exactly empty delta, and a real read_file tool call before the final response.
    let fixture = RuntimeFixture::new("opencode-delta-coalescing");
    let posts = Arc::new(AtomicUsize::new(0));
    let captured_followup = Arc::new(Mutex::new(None::<Value>));
    let response_posts = Arc::clone(&posts);
    let response_followup = Arc::clone(&captured_followup);
    let app = axum::Router::new()
        .route(
            "/models.dev",
            axum::routing::get(|| async {
                axum::Json(json!({"opencode-go": {
                    "npm": "@ai-sdk/openai-compatible",
                    "models": {"fixture-chat": {"tool_call": true}}
                }}))
            }),
        )
        .route(
            "/v1/models",
            axum::routing::get(|| async { axum::Json(json!({"data": [{"id": "fixture-chat"}]})) }),
        )
        .route(
            "/v1/chat/completions",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let posts = Arc::clone(&response_posts);
                let captured_followup = Arc::clone(&response_followup);
                async move {
                    match posts.fetch_add(1, Ordering::SeqCst) {
                        0 => accepted(reasoning_tool_stream()),
                        1 => {
                            *captured_followup.lock() = Some(body);
                            accepted(
                                concat!(
                                    "data: {\"choices\":[{\"delta\":{\"content\":\"complete\"},",
                                    "\"finish_reason\":\"stop\"}]}\n\n",
                                    "data: [DONE]\n\n"
                                )
                                .to_string(),
                            )
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
    let mut request = fixture.foreground(binding(), RUN_ID);
    request.policy.max_output_bytes = 2 * 1024 * 1024;

    // When: the common runtime drives model -> real direct tool gate -> model and checkpoints it.
    let outcome = runtime.run_foreground(request).await;
    server.abort();

    // Then: transport fragmentation remains observable but does not consume transcript entries.
    assert!(
        matches!(outcome, RunOutcome::Completed { ref text, .. } if text == "complete"),
        "fragmented OpenCode run failed: {outcome:?}"
    );
    assert_eq!(posts.load(Ordering::SeqCst), 2);
    let expected_deltas = reasoning_deltas();
    let events = fixture.events.snapshot();
    let observed_deltas = events
        .iter()
        .filter_map(|event| match event {
            EventSummary::Reasoning(text) => Some(text.clone()),
            EventSummary::Started(_)
            | EventSummary::Text(_)
            | EventSummary::Usage(_)
            | EventSummary::Finished(_, _) => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(observed_deltas, expected_deltas);
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                EventSummary::Started(response_id) => Some(response_id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [FIRST_RESPONSE_ID, FINAL_RESPONSE_ID]
    );
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                EventSummary::Finished(response_id, complete) => {
                    Some((response_id.as_str(), *complete))
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        [(FIRST_RESPONSE_ID, true), (FINAL_RESPONSE_ID, true)]
    );
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                EventSummary::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        ["complete"]
    );

    let expected_reasoning = expected_deltas.concat();
    let followup = captured_followup
        .lock()
        .clone()
        .expect("capture follow-up Chat Completions request");
    let messages = followup["messages"].as_array().expect("chat messages");
    let assistants = messages
        .iter()
        .filter(|message| message["role"] == "assistant")
        .collect::<Vec<_>>();
    assert_eq!(assistants.len(), 1);
    assert_eq!(assistants[0]["reasoning_content"], expected_reasoning);
    assert_eq!(assistants[0]["tool_calls"][0]["id"], TOOL_CALL_ID);
    assert_eq!(
        assistants[0]["tool_calls"][0]["function"]["name"],
        "read_file"
    );
    let tool_message = messages
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("actual direct tool result in follow-up history");
    assert_eq!(tool_message["tool_call_id"], TOOL_CALL_ID);
    assert!(
        tool_message["content"]
            .as_str()
            .is_some_and(|content| content.contains("onPluginStart")),
        "follow-up must contain the actual direct read_file result"
    );

    let store = ProviderTranscriptStore::new(&fixture.dirs);
    let revision = match runtime.conversation_state() {
        ProviderConversationState::OpencodeGo {
            transcript_revision,
        } => transcript_revision,
        other => panic!("unexpected direct conversation: {other:?}"),
    };
    assert_eq!(
        store
            .current_revision(ProviderId::OpencodeGo, &fixture.session_id)
            .expect("validate current pointer and generation hash"),
        revision
    );
    let current = store
        .load_current(ProviderId::OpencodeGo, &fixture.session_id)
        .expect("load hash-verified current generation");
    assert_eq!(current.revision, revision);
    assert_eq!(
        current.checkpoint.boundary,
        CheckpointBoundary::ResponseCompleted {
            response_id: FINAL_RESPONSE_ID.to_string()
        }
    );
    let restored = store
        .restore(
            ProviderId::OpencodeGo,
            &fixture.session_id,
            revision,
            &TranscriptBranch {
                instruction_epoch: 3,
                task_leaf_id: None,
            },
        )
        .expect("restore validated OpenCode transcript")
        .expect("current transcript generation");
    assert_eq!(restored.generation.revision, revision);
    assert!(!restored.metadata_was_stale);
    assert_eq!(restored.generation.checkpoint.blocks.len(), 5);
    assert!(matches!(
        &restored.generation.checkpoint.blocks[1],
        TranscriptBlock::AssistantReasoning {
            response_id,
            text,
            continuation: None,
        } if response_id == FIRST_RESPONSE_ID && text.as_bytes() == expected_reasoning.as_bytes()
    ));
    assert!(matches!(
        &restored.generation.checkpoint.blocks[2],
        TranscriptBlock::ToolCall {
            response_id,
            batch_id,
            id,
            name,
            audit_only: false,
            ..
        } if response_id == FIRST_RESPONSE_ID
            && batch_id == TOOL_BATCH_ID
            && id == TOOL_CALL_ID
            && name == "read_file"
    ));
    assert!(matches!(
        &restored.generation.checkpoint.blocks[3],
        TranscriptBlock::ToolResult {
            response_id,
            batch_id,
            id,
            name,
            is_error: false,
            ..
        } if response_id == FIRST_RESPONSE_ID
            && batch_id == TOOL_BATCH_ID
            && id == TOOL_CALL_ID
            && name == "read_file"
    ));
    assert!(matches!(
        &restored.generation.checkpoint.blocks[4],
        TranscriptBlock::AssistantText { response_id, text }
            if response_id == FINAL_RESPONSE_ID && text == "complete"
    ));
}
