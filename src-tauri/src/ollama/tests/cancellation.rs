use super::*;

#[tokio::test]
async fn production_adapter_cancels_open_stream_and_rejects_structured_prose() {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted, ready) = std::sync::mpsc::channel();
    let (release, finish) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut incoming = [0_u8; 8192];
        assert!(stream.read(&mut incoming).unwrap() > 0);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        accepted.send(()).unwrap();
        finish.recv().unwrap();
    });
    let client = reqwest::Client::builder().build().unwrap();
    let mut adapter = ProductionOllamaAdapter::with_client(client, None);
    let (cancel, stream_request) = request(&format!("http://{address}/v1"), Vec::new(), Vec::new());
    let (events, mut received) = tokio::sync::mpsc::channel(16);
    let mut running = tokio::spawn(async move { adapter.run_step(stream_request, events).await });
    tokio::task::spawn_blocking(move || ready.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::select! {
        result = &mut running => panic!("adapter finished before open-stream cancellation: {result:?}"),
        event = received.recv() => assert!(matches!(
            event.map(|event| event.kind),
            Some(AdapterEventKind::ResponseStarted { .. })
        )),
    }
    cancel.send(4).unwrap();
    assert!(matches!(
        running.await.unwrap(),
        Ok(AdapterStepOutcome::Cancelled)
    ));
    release.send(()).unwrap();
    server.join().unwrap();

    let stream = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"result: {\\\"ok\\\":true}\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let (base_url, _requests, server) = fixture(sse_response(stream));
    let client = reqwest::Client::builder().build().unwrap();
    let mut adapter = ProductionOllamaAdapter::with_client(client, None);
    let (_cancel, mut request) = request(&base_url, Vec::new(), Vec::new());
    request.kind = AdapterRequestKind::Structured {
        kind: crate::provider_runtime::StructuredJobKind::TaskStateCompiler,
        prompt: "compile".to_string(),
        workspace_root: std::env::temp_dir(),
        output_schema: json!({
            "type":"object",
            "properties":{"ok":{"type":"boolean"}},
            "required":["ok"]
        }),
    };
    let (events, _received) = tokio::sync::mpsc::channel(16);
    assert_eq!(
        adapter.run_step(request, events).await,
        Err(ProviderRuntimeError::StructuredOutputInvalid)
    );
    server.join().unwrap();
}

#[tokio::test]
async fn production_adapter_reports_http_timeout_as_transport_failure() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted, ready) = std::sync::mpsc::channel();
    let (release, finish) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        accepted.send(()).unwrap();
        finish.recv().unwrap();
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(50))
        .build()
        .unwrap();
    let mut adapter = ProductionOllamaAdapter::with_client(client, None);
    let (_cancel, request) = request(&format!("http://{address}/v1"), Vec::new(), Vec::new());
    let (events, _received) = tokio::sync::mpsc::channel(16);
    let running = tokio::spawn(async move { adapter.run_step(request, events).await });
    tokio::task::spawn_blocking(move || ready.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        running.await.unwrap(),
        Err(ProviderRuntimeError::Transport(
            "provider_transport_closed".to_string()
        ))
    );
    release.send(()).unwrap();
    server.join().unwrap();
}

#[tokio::test]
async fn production_adapter_fails_when_stream_cancellation_owner_closes() {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (release, finish) = std::sync::mpsc::channel();
    let server =
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0_u8; 4096];
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&request[..end]).unwrap();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        ).unwrap();
            finish.recv().unwrap();
        });
    let client = reqwest::Client::builder().build().unwrap();
    let mut adapter = ProductionOllamaAdapter::with_client(client, None);
    let (cancel, request) = request(&format!("http://{address}/v1"), Vec::new(), Vec::new());
    let (events, mut received) = tokio::sync::mpsc::channel(16);
    let mut running = tokio::spawn(async move { adapter.run_step(request, events).await });
    tokio::select! {
        outcome = &mut running => panic!("response failed before stream admission: {outcome:?}"),
        event = received.recv() => assert!(matches!(event.unwrap().kind, AdapterEventKind::ResponseStarted { .. })),
    }
    drop(cancel);
    let outcome = running.await.unwrap();
    release.send(()).unwrap();
    server.join().unwrap();
    assert_eq!(
        outcome,
        Err(ProviderRuntimeError::Transport(
            "provider cancellation channel closed".to_string()
        ))
    );
}
