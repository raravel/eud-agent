use super::*;

#[tokio::test]
async fn production_adapter_sends_coalesced_tool_result_continuation() {
    let stream = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"complete\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let (base_url, requests, server) = fixture(sse_response(stream));
    let call = |id: &str, path: &str| DirectToolCall {
        id: id.to_string(),
        name: "read_file".to_string(),
        arguments: json!({"path":path}),
    };
    let history = vec![
        ConversationItem::User {
            request_id: "request-a".to_string(),
            text: "inspect".to_string(),
            images: Vec::new(),
        },
        ConversationItem::Assistant(NormalizedBlock::Reasoning {
            response_id: "response-a".to_string(),
            text: "why".to_string(),
            continuation: None,
        }),
        ConversationItem::Assistant(NormalizedBlock::Text {
            response_id: "response-a".to_string(),
            text: "checking".to_string(),
        }),
        ConversationItem::Assistant(NormalizedBlock::ToolCall {
            response_id: "response-a".to_string(),
            batch_id: "batch-a".to_string(),
            call: call("call-1", "a.eps"),
            continuation: None,
        }),
        ConversationItem::Assistant(NormalizedBlock::ToolCall {
            response_id: "response-a".to_string(),
            batch_id: "batch-a".to_string(),
            call: call("call-2", "b.eps"),
            continuation: None,
        }),
    ];
    let results: Vec<DirectToolResult> = ["call-1", "call-2"]
        .into_iter()
        .map(|id| DirectToolResult {
            id: id.to_string(),
            name: "read_file".to_string(),
            result: json!({"ok":id}),
            is_error: false,
        })
        .collect();
    let mut history = history;
    history.extend(results.iter().cloned().map(|result| {
        ConversationItem::Assistant(NormalizedBlock::ToolResult {
            response_id: "response-a".to_string(),
            batch_id: "batch-a".to_string(),
            result,
        })
    }));
    let client = reqwest::Client::builder().build().unwrap();
    let mut adapter = ProductionOllamaAdapter::with_client(client, None);
    let (_cancel, request) = request(&base_url, history, results);
    let (events, _received) = tokio::sync::mpsc::channel(16);

    assert!(matches!(
        adapter.run_step(request, events).await.unwrap(),
        AdapterStepOutcome::Completed {
            output: AdapterOutput::Text(ref text),
            ..
        } if text == "complete"
    ));
    let request = requests.recv().unwrap();
    let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["messages"].as_array().unwrap().len(), 4);
    assert_eq!(body["messages"][1]["reasoning_content"], "why");
    assert_eq!(body["messages"][1]["content"], "checking");
    assert_eq!(
        body["messages"][1]["tool_calls"].as_array().unwrap().len(),
        2
    );
    assert_eq!(body["messages"][2]["tool_call_id"], "call-1");
    assert_eq!(body["messages"][3]["tool_call_id"], "call-2");
    server.join().unwrap();
}
