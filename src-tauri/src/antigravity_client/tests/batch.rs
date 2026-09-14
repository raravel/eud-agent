use super::*;

#[tokio::test]
async fn sends_ordered_batch_results_and_signature_on_followup_step() {
    let calls = sse(json!({"response":{"candidates":[{"content":{"parts":[
        {"thoughtSignature":"sig-a","functionCall":{"id":"a","name":"read_file","args":{"path":"a"}}},
        {"thoughtSignature":"sig-b","functionCall":{"id":"b","name":"read_file","args":{"path":"b"}}}
    ]},"finishReason":"STOP"}]}}));
    let answer = sse(
        json!({"response":{"candidates":[{"content":{"parts":[{"text":"done"}]},"finishReason":"STOP"}]}}),
    );
    let (endpoint, requests) = fixture(vec![calls, answer]).await;
    let mut adapter = adapter(endpoint);
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(32);
    let first = adapter
        .run_step(request(user_history(), Vec::new()), events_tx.clone())
        .await
        .unwrap();
    let AdapterStepOutcome::NeedsTools { calls, .. } = first else {
        panic!("expected batch")
    };
    let continuation = |signature: &str| {
        Some(ProviderContinuation {
            provider: ProviderId::Antigravity,
            data: json!({"thoughtSignature":signature}),
        })
    };
    let mut history = user_history();
    history.push(ConversationItem::Assistant(NormalizedBlock::Reasoning {
        response_id: "first".into(),
        text: "thinking".into(),
        continuation: continuation("reason-sig"),
    }));
    for (call, signature) in calls.iter().cloned().zip(["sig-a", "sig-b"]) {
        history.push(ConversationItem::Assistant(NormalizedBlock::ToolCall {
            response_id: "first".into(),
            batch_id: "first".into(),
            call,
            continuation: continuation(signature),
        }));
    }
    let results = vec![
        DirectToolResult {
            id: "a".into(),
            name: "read_file".into(),
            result: json!({"text":"A"}),
            is_error: false,
        },
        DirectToolResult {
            id: "b".into(),
            name: "read_file".into(),
            result: json!({"text":"B"}),
            is_error: false,
        },
    ];

    let outcome = adapter
        .run_step(request(history, results), events_tx)
        .await
        .unwrap();

    assert!(
        matches!(outcome, AdapterStepOutcome::Completed { output: AdapterOutput::Text(ref text), .. } if text == "done")
    );
    let sent = requests.lock();
    let contents = sent[1]["request"]["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 3);
    assert_eq!(contents[1]["parts"][0]["thoughtSignature"], "reason-sig");
    assert_eq!(contents[1]["parts"][1]["thoughtSignature"], "sig-a");
    assert_eq!(contents[1]["parts"][2]["thoughtSignature"], "sig-b");
    assert_eq!(contents[2]["parts"][0]["functionResponse"]["id"], "a");
    assert_eq!(contents[2]["parts"][1]["functionResponse"]["id"], "b");
    assert_eq!(
        sent[1]["request"]["generationConfig"]["maxOutputTokens"],
        8_192
    );
}

#[tokio::test]
async fn preserves_distinct_call_identity_when_unnamed_calls_arrive_in_separate_frames() {
    let frames = ["a", "b"].map(|path| {
        sse(json!({"response":{"candidates":[{"content":{"parts":[{
            "functionCall":{"name":"read_file","args":{"path":path}}
        }]}}]}}))
    });
    let terminal = sse(json!({"response":{"candidates":[{"finishReason":"STOP"}]}}));
    let (endpoint, _) = fixture(vec![frames.concat() + &terminal]).await;
    let mut adapter = adapter(endpoint);
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(8);

    let outcome = adapter
        .run_step(request(user_history(), Vec::new()), events_tx)
        .await
        .unwrap();

    let AdapterStepOutcome::NeedsTools { calls, .. } = outcome else {
        panic!("expected tool batch");
    };
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].id, calls[1].id);
    assert_eq!(calls[0].arguments, json!({"path":"a"}));
    assert_eq!(calls[1].arguments, json!({"path":"b"}));
}
