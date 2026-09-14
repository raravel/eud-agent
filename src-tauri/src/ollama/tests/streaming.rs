use super::*;

#[tokio::test]
async fn production_adapter_streams_reasoning_answer_usage_and_uses_binding_snapshot() {
    let stream = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"why\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}],",
        "\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":3,\"total_tokens\":7,\"prompt_tokens_details\":{\"cached_tokens\":2},\"completion_tokens_details\":{\"reasoning_tokens\":1}}}\n\n",
        "data: [DONE]\n\n"
    );
    let (base_url, requests, server) = fixture(sse_response(stream));
    let client = reqwest::Client::builder().build().unwrap();
    let mut adapter = ProductionOllamaAdapter::with_client(client, Some("proxy-key"));
    let (_cancel, mut request) = request(&base_url, Vec::new(), Vec::new());
    request.binding.capabilities = Some(ModelCapabilities {
        context_window: Some(128_000),
        ..Default::default()
    });
    let (events, mut received) = tokio::sync::mpsc::channel(16);

    let outcome = adapter.run_step(request, events).await.unwrap();

    assert!(matches!(
        outcome,
        AdapterStepOutcome::Completed {
            output: AdapterOutput::Text(ref text),
            ..
        } if text == "done"
    ));
    let request = requests.recv().unwrap();
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    assert!(request
        .to_ascii_lowercase()
        .contains("authorization: bearer proxy-key\r\n"));
    let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["model"], "fixture-model");
    assert_eq!(body["reasoning_effort"], "high");
    let mut saw_reasoning = false;
    let mut saw_usage = false;
    while let Ok(event) = received.try_recv() {
        match event.kind {
            AdapterEventKind::Block(NormalizedBlock::Reasoning { text, .. }) if text == "why" => {
                saw_reasoning = true;
            }
            AdapterEventKind::Usage(usage) if usage.total_tokens == Some(7) => {
                let context = usage.context_usage.as_ref().expect("full usage snapshot");
                assert_eq!(context.last.input_tokens, 4);
                assert_eq!(context.last.cached_input_tokens, 2);
                assert_eq!(context.last.output_tokens, 3);
                assert_eq!(context.last.reasoning_output_tokens, 1);
                assert_eq!(context.last.total_tokens, 7);
                assert_eq!(context.total, context.last);
                assert_eq!(context.model_context_window, Some(128_000));
                saw_usage = true;
            }
            _ => {}
        }
    }
    assert!(saw_reasoning);
    assert!(saw_usage);
    server.join().unwrap();
}

#[tokio::test]
async fn production_adapter_returns_ordered_tool_batch_from_fragmented_arguments() {
    let stream = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[",
        "{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}},",
        "{\"index\":1,\"id\":\"call-2\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[",
        "{\"index\":0,\"function\":{\"arguments\":\"\\\"a.eps\\\"}\"}},",
        "{\"index\":1,\"function\":{\"arguments\":\"\\\"b.eps\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let (base_url, _requests, server) = fixture(sse_response(stream));
    let client = reqwest::Client::builder().build().unwrap();
    let mut adapter = ProductionOllamaAdapter::with_client(client, None);
    let (_cancel, mut request) = request(&base_url, Vec::new(), Vec::new());
    request.binding.capabilities = Some(ModelCapabilities {
        context_window: Some(128_000),
        ..Default::default()
    });
    let (events, _received) = tokio::sync::mpsc::channel(16);

    let AdapterStepOutcome::NeedsTools { calls, .. } =
        adapter.run_step(request, events).await.unwrap()
    else {
        panic!("expected tool batch");
    };
    assert_eq!(calls[0].id, "call-1");
    assert_eq!(calls[0].arguments, json!({"path":"a.eps"}));
    assert_eq!(calls[1].id, "call-2");
    assert_eq!(calls[1].arguments, json!({"path":"b.eps"}));
    server.join().unwrap();
}

#[tokio::test]
async fn production_adapter_does_not_complete_partial_error_or_eof() {
    for stream in [
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\ndata: {\"error\":{\"message\":\"failed\"}}\n\ndata: [DONE]\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\ndata: [DONE]\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n",
    ] {
        let (base_url, _requests, server) = fixture(sse_response(stream));
        let client = reqwest::Client::builder().build().unwrap();
        let mut adapter = ProductionOllamaAdapter::with_client(client, None);
        let (_cancel, mut request) = request(&base_url, Vec::new(), Vec::new());
    request.binding.capabilities = Some(ModelCapabilities { context_window: Some(128_000), ..Default::default() });
        let (events, _received) = tokio::sync::mpsc::channel(16);

        assert!(matches!(
            adapter.run_step(request, events).await,
            Err(ProviderRuntimeError::Protocol(_))
        ));
        server.join().unwrap();
    }
}
