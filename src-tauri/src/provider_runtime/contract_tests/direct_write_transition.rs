use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use serde_json::{json, Value};

use crate::{
    opencode_go::OpenCodeGoAdapter,
    provider::{ModelCapabilities, ProviderConversationState, ProviderId},
    provider_runtime::{
        BindingSnapshot, ProviderRuntime, RunOutcome, RuntimeExecutor, WorkspaceAccess,
    },
    provider_transcript::{ProviderTranscriptStore, TranscriptBlock, TranscriptBranch},
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

#[tokio::test]
async fn writable_opencode_run_keeps_redundant_intent_and_real_mutation_in_one_run() {
    // Given: a production OpenCode Responses adapter is already executing with write access.
    let fixture = RuntimeFixture::new("direct-redundant-write");
    fixture
        .tools
        .execute("search_docs", &json!({"query": "file creation"}))
        .unwrap();
    fixture
        .tools
        .request_write_workspace("prepare writable fixture")
        .unwrap();
    let posts = Arc::new(AtomicUsize::new(0));
    let response_posts = Arc::clone(&posts);
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
                async move {
                    let round = posts.fetch_add(1, Ordering::SeqCst);
                    let outputs = body["input"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|item| item["type"] == "function_call_output")
                        .collect::<Vec<_>>();
                    match round {
                        0 => accepted(concat!(
                            "event: response.output_item.added\n",
                            "data: {\"item\":{\"type\":\"function_call\",\"call_id\":\"write-again\",\"name\":\"request_write_workspace\",\"arguments\":\"{\\\"reason\\\":\\\"retry the same edit\\\"}\"}}\n\n",
                            "event: response.completed\n",
                            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
                        ).to_string()),
                        1 => {
                            assert_eq!(outputs.len(), 1);
                            let write_result: Value = serde_json::from_str(
                                outputs[0]["output"].as_str().expect("write result output"),
                            )
                            .expect("write result JSON");
                            assert_eq!(write_result["status"], "already_granted");
                            assert!(write_result["note"]
                                .as_str()
                                .is_some_and(|note| note.contains("Continue this turn")));
                            accepted(concat!(
                                "event: response.output_item.added\n",
                                "data: {\"item\":{\"type\":\"function_call\",\"call_id\":\"create-after-intent\",\"name\":\"file_create\",\"arguments\":\"{\\\"path\\\":\\\"src/direct-after-intent.eps\\\",\\\"ftype\\\":\\\"CUIEps\\\",\\\"code\\\":\\\"const direct_after_intent = 1;\\\\n\\\"}\"}}\n\n",
                                "event: response.completed\n",
                                "data: {\"response\":{\"status\":\"completed\"}}\n\n"
                            ).to_string())
                        }
                        2 => {
                            assert_eq!(outputs.len(), 2);
                            accepted(concat!(
                                "event: response.output_text.delta\n",
                                "data: {\"delta\":\"mutation completed\"}\n\n",
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
    let mut request = fixture.foreground(binding(), 40_201);
    request.turn.workspace_access = WorkspaceAccess::Write;

    // When: the model repeats write intent, then performs a real mutation and answers.
    let outcome = runtime.run_foreground(request).await;
    server.abort();

    // Then: both completions remain ordered and the redundant intent causes no transition.
    assert!(
        matches!(outcome, RunOutcome::Completed { ref text, .. } if text == "mutation completed"),
        "redundant write intent terminated the writable run: {outcome:?}"
    );
    assert_eq!(posts.load(Ordering::SeqCst), 3);
    assert_eq!(
        std::fs::read(fixture.root.join("project/src/direct-after-intent.eps")).unwrap(),
        b"const direct_after_intent = 1;\n"
    );
    let revision = match runtime.conversation_state() {
        ProviderConversationState::OpencodeGo {
            transcript_revision,
        } => transcript_revision,
        other => panic!("unexpected direct conversation: {other:?}"),
    };
    let transcript = ProviderTranscriptStore::new(&fixture.dirs)
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
    let results = transcript
        .generation
        .checkpoint
        .blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::ToolResult { id, result, .. } => Some((id.as_str(), result)),
            TranscriptBlock::User { .. }
            | TranscriptBlock::AssistantText { .. }
            | TranscriptBlock::AssistantReasoning { .. }
            | TranscriptBlock::ToolCall { .. }
            | TranscriptBlock::Compaction { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, "write-again");
    assert_eq!(results[0].1["status"], "already_granted");
    assert_eq!(results[1].0, "create-after-intent");
    assert_eq!(
        results[1].1,
        &json!({"ok": true, "path": "src/direct-after-intent.eps"})
    );
}
