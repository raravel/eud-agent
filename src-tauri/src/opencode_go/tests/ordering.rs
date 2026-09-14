use super::{fixture::*, ordering_frames::interleaved, *};

#[tokio::test]
async fn production_http_responses_followup_preserves_interleaved_blocks() {
    captured_followup(OpenCodeGoWire::Responses).await;
}

#[tokio::test]
async fn production_http_anthropic_followup_preserves_interleaved_blocks() {
    captured_followup(OpenCodeGoWire::AnthropicMessages).await;
}

#[tokio::test]
async fn production_http_chat_followup_preserves_fragmented_indexed_batch() {
    captured_followup(OpenCodeGoWire::ChatCompletions).await;
}

async fn captured_followup(wire: OpenCodeGoWire) {
    use axum::routing::{get, post};
    let captured = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let requests = captured.clone();
    let npm = match wire {
        OpenCodeGoWire::Responses => "@ai-sdk/openai",
        OpenCodeGoWire::ChatCompletions => "@ai-sdk/openai-compatible",
        OpenCodeGoWire::AnthropicMessages => "@ai-sdk/anthropic",
    };
    let metadata = json!({"opencode-go":{"npm":npm,"models":{"fixture-wire":{
        "tool_call":true,"structured_output":true,"limit":{"output":1024}
    }}}});
    let app = axum::Router::new()
        .route(
            "/v1/models",
            get(|| async { axum::Json(json!({"data":[{"id":"fixture-wire"}]})) }),
        )
        .route(
            "/models.dev",
            get(move || {
                let metadata = metadata.clone();
                async move { axum::Json(metadata) }
            }),
        )
        .route(
            &format!("/v1/{}", wire_path(wire)),
            post(move |axum::Json(body): axum::Json<Value>| {
                let first = {
                    let mut requests = requests.lock().unwrap();
                    requests.push(body);
                    requests.len() == 1
                };
                async move {
                    (
                        [("content-type", "text/event-stream")],
                        if first {
                            interleaved(wire)
                        } else {
                            text_frame(wire, "done") + &terminal(wire, false, false)
                        },
                    )
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut adapter = OpenCodeGoAdapter::new_for_test(
        reqwest::Client::new(),
        format!("{base}/v1"),
        format!("{base}/v1"),
        format!("{base}/models.dev"),
        "fixture-key".into(),
    );
    let (_cancel, cancellation) = tokio::sync::watch::channel(0);
    let mut history = vec![ConversationItem::User {
        request_id: "fixture-request".into(),
        text: "inspect".into(),
        images: Vec::new(),
    }];
    let (events, mut received) = tokio::sync::mpsc::channel(32);
    let first = adapter
        .run_step(
            fixture_request("fixture-wire", history.clone(), cancellation.clone()),
            events,
        )
        .await
        .unwrap();
    let AdapterStepOutcome::NeedsTools {
        calls,
        continuation,
    } = first
    else {
        panic!("expected complete batch");
    };
    assert_eq!(
        calls
            .iter()
            .map(|call| call.id.as_str())
            .collect::<Vec<_>>(),
        ["z", "a"]
    );
    assert_eq!(calls[0].arguments, json!({"path":"a"}));
    assert_eq!(calls[1].arguments, json!({"path":"b"}));
    let mut results = Vec::new();
    while let Ok(event) = received.try_recv() {
        if let AdapterEventKind::Block(block) = event.kind {
            if let NormalizedBlock::ToolCall {
                response_id,
                batch_id,
                call,
                ..
            } = &block
            {
                results.push(ConversationItem::Assistant(NormalizedBlock::ToolResult {
                    response_id: response_id.clone(),
                    batch_id: batch_id.clone(),
                    result: DirectToolResult {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        result: json!({"ok":true}),
                        is_error: false,
                    },
                }));
            }
            history.push(ConversationItem::Assistant(block));
        }
    }
    assert_eq!(results.len(), 2, "exactly one complete event per tool");
    history.extend(results);
    let restored = serde_json::from_slice(&serde_json::to_vec(&history).unwrap()).unwrap();
    let mut second = fixture_request("fixture-wire", restored, cancellation);
    second.continuation = continuation;
    let (events, _received) = tokio::sync::mpsc::channel(8);
    assert!(matches!(
        adapter.run_step(second, events).await.unwrap(),
        AdapterStepOutcome::Completed { .. }
    ));
    server.abort();
    let requests = captured.lock().unwrap();
    let followup = &requests[1];
    if wire == OpenCodeGoWire::ChatCompletions {
        let messages = followup["messages"].as_array().unwrap();
        let assistant = messages
            .iter()
            .find(|message| message["role"] == "assistant")
            .unwrap();
        assert_eq!(assistant["content"], "ABC");
        assert_eq!(assistant["reasoning_content"], "R");
        assert_eq!(
            assistant["tool_calls"]
                .as_array()
                .unwrap()
                .iter()
                .map(|call| call["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["z", "a"]
        );
        assert_eq!(
            messages
                .iter()
                .filter(|message| message["role"] == "tool")
                .map(|message| message["tool_call_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["z", "a"]
        );
    } else {
        let ordered: Vec<String> = if wire == OpenCodeGoWire::Responses {
            followup["input"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|item| match item["type"].as_str() {
                    Some("function_call") => vec![item["call_id"].as_str().unwrap().to_string()],
                    Some("reasoning") => {
                        assert_eq!(item["encrypted_content"], "encrypted-R");
                        vec!["R".into()]
                    }
                    _ if item["role"] == "assistant" => item["content"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|part| part["text"].as_str().unwrap().to_string())
                        .collect(),
                    _ => Vec::new(),
                })
                .collect()
        } else {
            let assistant = followup["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["role"] == "assistant")
                .unwrap();
            assistant["content"]
                .as_array()
                .unwrap()
                .iter()
                .map(|part| match part["type"].as_str().unwrap() {
                    "tool_use" => part["id"].as_str().unwrap().to_string(),
                    "thinking" => {
                        assert_eq!(part["signature"], "signed-R");
                        part["thinking"].as_str().unwrap().to_string()
                    }
                    "text" => part["text"].as_str().unwrap().to_string(),
                    kind => panic!("unexpected block {kind}"),
                })
                .collect()
        };
        assert_eq!(
            ordered,
            ["A", "z", "R", "B", "a", "C"],
            "captured followup for {wire:?}"
        );
    }
}
