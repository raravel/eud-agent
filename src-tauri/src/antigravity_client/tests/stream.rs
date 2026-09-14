use super::*;

#[tokio::test]
async fn preserves_reasoning_text_order_and_signature_when_stream_completes() {
    let first = sse(json!({"response":{
        "candidates":[{"content":{"parts":[
            {"text":"reason","thought":true,"thoughtSignature":"reason-sig"},
            {"text":"answer"},
            {"thoughtSignature":"sig-1","functionCall":{"id":"call-1","name":"read_file","args":{"path":"a.eps"}}}
        ]},"finishReason":"STOP"}],
        "usageMetadata":{"promptTokenCount":4,"cachedContentTokenCount":2,"candidatesTokenCount":2,"thoughtsTokenCount":1,"totalTokenCount":7}
    }}));
    let (endpoint, _) = fixture(vec![first]).await;
    let mut adapter = adapter(endpoint);
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(16);

    let outcome = adapter
        .run_step(request(user_history(), Vec::new()), events_tx)
        .await
        .unwrap();

    assert!(matches!(outcome, AdapterStepOutcome::NeedsTools { .. }));
    let mut kinds = Vec::new();
    while let Ok(event) = events_rx.try_recv() {
        kinds.push(event.kind);
    }
    assert!(matches!(kinds[0], AdapterEventKind::ResponseStarted { .. }));
    assert!(matches!(
        kinds[1],
        AdapterEventKind::Block(NormalizedBlock::Reasoning {
            continuation: Some(ref value),
            ..
        }) if value.data["thoughtSignature"] == "reason-sig"
    ));
    assert!(matches!(
        kinds[2],
        AdapterEventKind::Block(NormalizedBlock::Text { .. })
    ));
    let AdapterEventKind::Block(NormalizedBlock::ToolCall {
        continuation: Some(value),
        ..
    }) = &kinds[3]
    else {
        panic!("missing signed tool call")
    };
    assert_eq!(value.data["thoughtSignature"], "sig-1");
    let AdapterEventKind::Usage(usage) = &kinds[4] else {
        panic!("missing usage");
    };
    let context = usage.context_usage.as_ref().expect("full usage snapshot");
    assert_eq!(context.last.input_tokens, 4);
    assert_eq!(context.last.cached_input_tokens, 2);
    assert_eq!(context.last.output_tokens, 2);
    assert_eq!(context.last.reasoning_output_tokens, 1);
    assert_eq!(context.last.total_tokens, 7);
    assert_eq!(context.total, context.last);
    assert_eq!(context.model_context_window, Some(128_000));
    assert!(matches!(
        kinds[5],
        AdapterEventKind::ResponseFinished { complete: true, .. }
    ));
}

#[tokio::test]
async fn dropped_cancellation_sender_does_not_starve_a_normal_stream() {
    let answer = sse(
        json!({"response":{"candidates":[{"content":{"parts":[{"text":"done"}]},"finishReason":"STOP"}]}}),
    );
    let (endpoint, _) = fixture(vec![answer]).await;
    let mut adapter = adapter(endpoint);
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(8);

    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        adapter.run_step(request(user_history(), Vec::new()), events_tx),
    )
    .await
    .expect("dropped cancellation sender starved the response")
    .unwrap();

    assert!(matches!(
        outcome,
        AdapterStepOutcome::Completed {
            output: AdapterOutput::Text(ref text),
            ..
        } if text == "done"
    ));
}

#[tokio::test]
async fn rejects_partial_text_when_stream_closes_without_finish_reason() {
    let partial =
        sse(json!({"response":{"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}}));
    let (endpoint, _) = fixture(vec![partial]).await;
    let mut adapter = adapter(endpoint);
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(8);

    let error = adapter
        .run_step(request(user_history(), Vec::new()), events_tx)
        .await
        .unwrap_err();

    assert!(
        matches!(error, ProviderRuntimeError::Protocol(ref detail) if detail.contains("before completion"))
    );
}

#[tokio::test]
async fn preserves_partial_delta_but_fails_a_provider_error_frame() {
    let response = format!(
        "{}{}",
        sse(json!({"response":{"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}})),
        sse(json!({"error":{"message":"fixture failure"}})),
    );
    let (endpoint, _) = fixture(vec![response]).await;
    let mut adapter = adapter(endpoint);
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(8);

    let error = adapter
        .run_step(request(user_history(), Vec::new()), events_tx)
        .await
        .unwrap_err();

    assert!(matches!(error, ProviderRuntimeError::Transport(_)));
    let events = std::iter::from_fn(|| events_rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        AdapterEventKind::Block(NormalizedBlock::Text { text, .. }) if text == "partial"
    )));
    assert!(!events.iter().any(|event| matches!(
        event.kind,
        AdapterEventKind::ResponseFinished { complete: true, .. }
    )));
}
