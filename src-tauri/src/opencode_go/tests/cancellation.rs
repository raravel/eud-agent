use super::*;

#[tokio::test]
async fn production_adapter_rejects_partial_error_and_incomplete_eof() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::routing::{get, post};

    let attempts = Arc::new(AtomicUsize::new(0));
    let stream_attempts = attempts.clone();
    let app = axum::Router::new()
        .route(
            "/models.dev",
            get(|| async {
                axum::Json(json!({"opencode-go":{
                    "npm":"@ai-sdk/openai-compatible",
                    "models":{"fixture-chat":{"tool_call":true}}
                }}))
            }),
        )
        .route(
            "/v1/models",
            get(|| async { axum::Json(json!({"data":[{"id":"fixture-chat"}]})) }),
        )
        .route(
            "/v1/chat/completions",
            post(move || {
                let attempt = stream_attempts.fetch_add(1, Ordering::SeqCst);
                async move {
                    match attempt {
                        0 => concat!(
                            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
                            "data: {\"error\":{\"type\":\"server_error\"}}\n\n"
                        ),
                        1 => "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
                        2 => "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\ndata: [DONE]\n\n",
                        _ => "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n",
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{address}");
    let mut adapter = OpenCodeGoAdapter::new_for_test(
        reqwest::Client::new(),
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
    for attempt in 0..4 {
        let (events, mut received) = tokio::sync::mpsc::channel(16);
        let result = adapter
            .run_step(
                fixture_request("fixture-chat", vec![user.clone()], cancellation.clone()),
                events,
            )
            .await;
        if attempt < 2 {
            assert!(matches!(result, Err(ProviderRuntimeError::Transport(_))));
        } else {
            assert!(matches!(result, Err(ProviderRuntimeError::Protocol(_))));
        }
        let kinds = std::iter::from_fn(|| received.try_recv().ok())
            .map(|event| event.kind)
            .collect::<Vec<_>>();
        assert!(kinds.iter().any(|kind| matches!(
            kind,
            AdapterEventKind::Block(NormalizedBlock::Text { text, .. })
                if text == "partial"
        )));
        assert_eq!(
            kinds
                .iter()
                .any(|kind| matches!(kind, AdapterEventKind::TransportClosed)),
            attempt < 2
        );
    }
    server.abort();
}

#[tokio::test]
async fn production_adapter_emits_delta_before_terminal_frame() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let bodies = [
            json!({"data":[{"id":"fixture-chat"}]}).to_string(),
            json!({"opencode-go":{
                "npm":"@ai-sdk/openai-compatible",
                "models":{"fixture-chat":{"tool_call":true}}
            }})
            .to_string(),
        ];
        for body in bodies {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; 8_192];
            let _ = socket.read(&mut request).await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        }
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0u8; 8_192];
        let _ = socket.read(&mut request).await.unwrap();
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let first = b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"}}]}\n\n";
        socket
            .write_all(format!("{:X}\r\n", first.len()).as_bytes())
            .await
            .unwrap();
        socket.write_all(first).await.unwrap();
        socket.write_all(b"\r\n").await.unwrap();
        socket.flush().await.unwrap();
        let _ = release_receiver.await;
        let terminal = b"data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        socket
            .write_all(format!("{:X}\r\n", terminal.len()).as_bytes())
            .await
            .unwrap();
        socket.write_all(terminal).await.unwrap();
        socket.write_all(b"\r\n0\r\n\r\n").await.unwrap();
    });
    let base = format!("http://{address}");
    let mut adapter = OpenCodeGoAdapter::new_for_test(
        reqwest::Client::new(),
        format!("{base}/v1"),
        format!("{base}/v1"),
        format!("{base}/models.dev"),
        "fixture-key".to_string(),
    );
    let (_cancel, cancellation) = tokio::sync::watch::channel(0);
    let request = fixture_request(
        "fixture-chat",
        vec![ConversationItem::User {
            request_id: "fixture-request".to_string(),
            text: "inspect".to_string(),
            images: Vec::new(),
        }],
        cancellation,
    );
    let (events, mut received) = tokio::sync::mpsc::channel(16);
    let run = tokio::spawn(async move { adapter.run_step(request, events).await });
    loop {
        let event = received.recv().await.unwrap();
        if matches!(
            event.kind,
            AdapterEventKind::Block(NormalizedBlock::Reasoning { ref text, .. })
                if text == "think"
        ) {
            break;
        }
    }
    assert!(!run.is_finished());
    release_sender.send(()).unwrap();
    assert!(matches!(
        run.await.unwrap().unwrap(),
        AdapterStepOutcome::Completed {
            output: AdapterOutput::Text(ref text),
            ..
        } if text == "done"
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn cancelled_generation_never_starts_transport() {
    let mut adapter = OpenCodeGoAdapter::new_for_test(
        reqwest::Client::new(),
        "http://127.0.0.1:1/v1".to_string(),
        "http://127.0.0.1:1/v1".to_string(),
        "http://127.0.0.1:1/models.dev".to_string(),
        "fixture-key".to_string(),
    );
    let (_cancel, cancellation) = tokio::sync::watch::channel(1);
    let request = fixture_request("fixture-chat", Vec::new(), cancellation);
    let (events, _received) = tokio::sync::mpsc::channel(4);
    assert_eq!(
        adapter.run_step(request, events).await,
        Ok(AdapterStepOutcome::Cancelled)
    );
}
