use super::*;

#[test]
fn responses_stream_assembles_text_reasoning_and_tool_arguments() {
    let bytes = sse(&[
        "event: response.output_text.delta\ndata: {\"delta\":\"answer\"}",
        "event: response.reasoning_summary_text.delta\ndata: {\"delta\":\"think\"}",
        "event: response.output_item.added\ndata: {\"item\":{\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"read_file\"}}",
        "event: response.function_call_arguments.delta\ndata: {\"call_id\":\"c1\",\"delta\":\"{\\\"path\\\":\\\"a.eps\\\"}\"}",
    ]);
    let mut decoder = SseDecoder::default();
    let mut parser = WireParser::new(OpenCodeGoWire::Responses);
    for event in decoder.push(&bytes).unwrap() {
        let value = serde_json::from_str(&event.data).unwrap();
        parser.apply(event.event.as_deref(), value).unwrap();
    }
    let step = parser.finish().unwrap();
    assert_eq!(step.text, "answer");
    assert_eq!(step.reasoning, "think");
    assert_eq!(step.tool_calls[0].arguments, json!({"path":"a.eps"}));
}

#[test]
fn chat_and_anthropic_streams_preserve_tool_order() {
    let mut chat = WireParser::new(OpenCodeGoWire::ChatCompletions);
    chat.apply(
        None,
        json!({"choices":[{"delta":{"tool_calls":[
            {"index":0,"id":"a","function":{"name":"read_file","arguments":"{}"}},
            {"index":1,"id":"b","function":{"name":"search_docs","arguments":"{}"}}
        ]}}]}),
    )
    .unwrap();
    assert_eq!(
        chat.finish()
            .unwrap()
            .tool_calls
            .iter()
            .map(|call| call.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );

    let mut anthropic = WireParser::new(OpenCodeGoWire::AnthropicMessages);
    anthropic.apply(Some("content_block_start"), json!({"index":0,"content_block":{"type":"tool_use","id":"a","name":"read_file","input":{}}})).unwrap();
    anthropic.apply(Some("content_block_start"), json!({"index":1,"content_block":{"type":"tool_use","id":"b","name":"search_docs","input":{}}})).unwrap();
    assert_eq!(
        anthropic
            .finish()
            .unwrap()
            .tool_calls
            .iter()
            .map(|call| call.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[test]
fn responses_stream_rejects_duplicate_tool_call_id() {
    let mut parser = WireParser::new(OpenCodeGoWire::Responses);
    for name in ["read_file", "search_docs"] {
        let result = parser.apply(
            Some("response.output_item.added"),
            json!({"item":{
                "type":"function_call",
                "call_id":"duplicate",
                "name":name,
                "arguments":"{}"
            }}),
        );
        if name == "read_file" {
            assert!(result.is_ok());
        } else {
            assert_eq!(
                result,
                Err("provider returned duplicate tool call id".to_string())
            );
        }
    }
}

#[test]
fn incomplete_tool_arguments_are_never_returned_as_calls() {
    let mut parser = WireParser::new(OpenCodeGoWire::ChatCompletions);
    parser
        .apply(
            None,
            json!({"choices":[{"delta":{"tool_calls":[{
                "index":0,
                "id":"incomplete",
                "function":{"name":"read_file","arguments":"{"}
            }]}}]}),
        )
        .unwrap();
    assert_eq!(
        parser.finish(),
        Err("provider returned invalid tool arguments".to_string())
    );
}

#[test]
fn structured_response_rejects_duplicate_submissions() {
    let calls = ["first", "second"]
        .into_iter()
        .map(|id| DirectToolCall {
            id: id.to_string(),
            name: STRUCTURED_TOOL.to_string(),
            arguments: json!({"ok":true}),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        structured_value(&calls, &json!({"type":"object"})),
        Err(ProviderRuntimeError::StructuredOutputInvalid)
    );
}

#[tokio::test]
async fn chat_stream_error_is_not_empty_or_partial_success() {
    use axum::routing::get;

    let error = "data: {\"error\":{\"type\":\"server_error\",\"code\":\"server_error\",\"message\":\"Streaming response failed: upstream inference error\"}}\n\n";
    let partial =
        format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"unfinished\"}}}}]}}\n\n{error}");
    let app = axum::Router::new()
        .route("/error", get(move || async move { error }))
        .route(
            "/partial",
            get(move || {
                let partial = partial.clone();
                async move { partial }
            }),
        );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    let (_cancel, mut cancellation) = tokio::sync::watch::channel(0);
    let mut results = Vec::new();
    for path in ["error", "partial"] {
        let response = client
            .get(format!("http://{address}/{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        results.push(
            parse_stream(
                response,
                OpenCodeGoWire::ChatCompletions,
                &mut cancellation,
                0,
            )
            .await,
        );
    }
    server.abort();
    for result in results {
        assert_eq!(result.unwrap_err(), "provider_transport_closed");
    }
}

#[tokio::test]
async fn failed_inference_status_keeps_the_provider_error_message() {
    use axum::http::StatusCode;
    use axum::routing::get;

    let app = axum::Router::new()
        .route(
            "/json",
            get(|| async {
                (
                    StatusCode::FORBIDDEN,
                    r#"{"error":{"message":"You have hit the 5-hour\nlimit for glm-5.3","type":"forbidden"}}"#,
                )
            }),
        )
        .route(
            "/text",
            get(|| async { (StatusCode::PAYMENT_REQUIRED, "  Insufficient  balance \u{1} ") }),
        )
        .route("/empty", get(|| async { (StatusCode::FORBIDDEN, "") }));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    let mut results = Vec::new();
    for path in ["json", "text", "empty"] {
        let response = client
            .get(format!("http://{address}/{path}"))
            .send()
            .await
            .unwrap();
        results.push(inference_status_error(response).await);
    }
    server.abort();
    assert_eq!(
        results,
        [
            "provider_quota_exhausted (HTTP 403): You have hit the 5-hour limit for glm-5.3",
            "provider_quota_exhausted (HTTP 402): Insufficient balance",
            "provider_quota_exhausted (HTTP 403)",
        ]
    );
}

#[test]
fn incomplete_sse_fails_closed() {
    let mut incomplete = SseDecoder::default();
    incomplete.push(b"data: {\"x\":1}").unwrap();
    assert!(incomplete.finish().is_err());
}

#[test]
fn wire_deltas_drop_only_exactly_empty_text_and_reasoning() {
    let response_id = "response-1";
    let blocks = |wire, event, value: Value| {
        wire_delta_blocks(wire, event, &value, response_id).expect("decode wire delta")
    };

    assert!(blocks(
        OpenCodeGoWire::Responses,
        Some("response.output_text.delta"),
        json!({"delta":""}),
    )
    .is_empty());
    assert_eq!(
        blocks(
            OpenCodeGoWire::Responses,
            Some("response.output_text.delta"),
            json!({"delta":" "}),
        ),
        [NormalizedBlock::Text {
            response_id: response_id.to_string(),
            text: " ".to_string(),
        }]
    );
    assert!(blocks(
        OpenCodeGoWire::Responses,
        Some("response.reasoning_summary_text.delta"),
        json!({"delta":""}),
    )
    .is_empty());
    assert_eq!(
        blocks(
            OpenCodeGoWire::Responses,
            Some("response.reasoning_summary_text.delta"),
            json!({"delta":"\n"}),
        ),
        [NormalizedBlock::Reasoning {
            response_id: response_id.to_string(),
            text: "\n".to_string(),
            continuation: None,
        }]
    );

    assert!(blocks(
        OpenCodeGoWire::ChatCompletions,
        None,
        json!({"choices":[{"delta":{"content":"","reasoning_content":""}}]}),
    )
    .is_empty());
    assert_eq!(
        blocks(
            OpenCodeGoWire::ChatCompletions,
            None,
            json!({"choices":[{"delta":{"content":" ","reasoning_content":"\n"}}]}),
        ),
        [
            NormalizedBlock::Text {
                response_id: response_id.to_string(),
                text: " ".to_string(),
            },
            NormalizedBlock::Reasoning {
                response_id: response_id.to_string(),
                text: "\n".to_string(),
                continuation: None,
            },
        ]
    );

    assert!(blocks(
        OpenCodeGoWire::AnthropicMessages,
        None,
        json!({"delta":{"type":"text_delta","text":""}}),
    )
    .is_empty());
    assert_eq!(
        blocks(
            OpenCodeGoWire::AnthropicMessages,
            None,
            json!({"delta":{"type":"text_delta","text":" "}}),
        ),
        [NormalizedBlock::Text {
            response_id: response_id.to_string(),
            text: " ".to_string(),
        }]
    );
    assert!(blocks(
        OpenCodeGoWire::AnthropicMessages,
        None,
        json!({"delta":{"type":"thinking_delta","thinking":""}}),
    )
    .is_empty());
    assert_eq!(
        blocks(
            OpenCodeGoWire::AnthropicMessages,
            None,
            json!({"delta":{"type":"thinking_delta","thinking":"\n"}}),
        ),
        [NormalizedBlock::Reasoning {
            response_id: response_id.to_string(),
            text: "\n".to_string(),
            continuation: None,
        }]
    );
}
