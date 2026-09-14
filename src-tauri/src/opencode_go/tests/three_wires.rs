use super::{successful_replies::*, *};

#[tokio::test]
async fn production_adapter_decodes_all_three_wire_contracts() {
    use axum::routing::{get, post};

    let metadata = json!({"opencode-go":{
        "npm":"@ai-sdk/openai-compatible",
        "models":{
            "fixture-responses":{"provider":{"npm":"@ai-sdk/openai"},"tool_call":true,"structured_output":true},
            "fixture-chat":{"provider":{"npm":"@ai-sdk/openai-compatible"},"tool_call":true,"structured_output":true},
            "fixture-messages":{"provider":{"npm":"@ai-sdk/anthropic"},"tool_call":true,"structured_output":true,"limit":{"output":1024}}
        }
    }});

    let app = axum::Router::new()
        .route(
            "/models.dev",
            get(move || {
                let metadata = metadata.clone();
                async move { axum::Json(metadata) }
            }),
        )
        .route(
            "/v1/models",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer fixture-key")
                );
                axum::Json(json!({"data":[
                    {"id":"fixture-responses"},
                    {"id":"fixture-chat"},
                    {"id":"fixture-messages"}
                ]}))
            }),
        )
        .route("/v1/responses", post(responses))
        .route("/v1/chat/completions", post(chat))
        .route("/v1/messages", post(messages));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{address}");

    for model in ["fixture-responses", "fixture-chat", "fixture-messages"] {
        let client = reqwest::Client::new();
        let mut adapter = OpenCodeGoAdapter::new_for_test(
            client,
            format!("{base}/v1"),
            format!("{base}/v1"),
            format!("{base}/models.dev"),
            "fixture-key".to_string(),
        );
        let (_cancel, cancellation) = tokio::sync::watch::channel(0);
        let user = ConversationItem::User {
            request_id: "fixture-request".to_string(),
            text: "inspect".to_string(),
            images: Vec::new(),
        };
        let (events, mut received) = tokio::sync::mpsc::channel(32);
        let first = adapter
            .run_step(
                fixture_request(model, vec![user.clone()], cancellation.clone()),
                events,
            )
            .await
            .unwrap();
        let AdapterStepOutcome::NeedsTools {
            calls,
            continuation,
        } = first
        else {
            panic!("fixture must request tools");
        };
        assert_eq!(
            calls
                .iter()
                .map(|call| call.id.as_str())
                .collect::<Vec<_>>(),
            ["z", "a"]
        );
        let mut history = vec![user];
        while let Ok(event) = received.try_recv() {
            if let AdapterEventKind::Block(block) = event.kind {
                history.push(ConversationItem::Assistant(block));
            }
        }
        for call in &calls {
            history.push(ConversationItem::Assistant(NormalizedBlock::ToolResult {
                response_id: "7-1".to_string(),
                batch_id: "7-1-tools".to_string(),
                result: DirectToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    result: json!({"ok":true}),
                    is_error: false,
                },
            }));
        }
        let (events, _received) = tokio::sync::mpsc::channel(32);
        let mut second = fixture_request(model, history, cancellation.clone());
        second.continuation = continuation;
        let completed = adapter.run_step(second, events).await.unwrap();
        assert!(matches!(
            completed,
            AdapterStepOutcome::Completed {
                output: AdapterOutput::Text(ref text),
                ..
            } if text == "done"
        ));

        let (events, _received) = tokio::sync::mpsc::channel(16);
        let mut structured = fixture_request(model, Vec::new(), cancellation.clone());
        structured.kind = AdapterRequestKind::Structured {
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
        structured.policy.max_output_tokens = Some(8_192);
        let output = adapter.run_step(structured, events).await.unwrap();
        assert!(matches!(
            output,
            AdapterStepOutcome::Completed {
                output: AdapterOutput::Structured(ref value),
                ..
            } if value == &json!({"ok":true})
        ));
    }

    server.abort();
}
