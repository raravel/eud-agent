use super::*;

#[tokio::test]
async fn cancellation_stops_late_frames_from_reaching_the_event_channel() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut incoming = [0_u8; 8192];
        assert!(socket.read(&mut incoming).await.unwrap() > 0);
        let catalog = json!({"models":{"fixture-model":{
            "supportsThinking":true,"maxOutputTokens":16384
        }}})
        .to_string();
        socket.write_all(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{catalog}",
            catalog.len()
        ).as_bytes()).await.unwrap();
        drop(socket);
        let (mut socket, _) = listener.accept().await.unwrap();
        assert!(socket.read(&mut incoming).await.unwrap() > 0);
        let first = b"data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"before\"}]}}]}}\n\n";
        let late = b"data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"late\"}]},\"finishReason\":\"STOP\"}]}}\n\n";
        socket.write_all(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            first.len() + late.len()
        ).as_bytes()).await.unwrap();
        socket.write_all(first).await.unwrap();
        socket.flush().await.unwrap();
        let _ = release_rx.await;
        let _ = socket.write_all(late).await;
    });
    let mut adapter = adapter(endpoint);
    let (cancel_tx, cancellation) = tokio::sync::watch::channel(7);
    let mut step = request(user_history(), Vec::new());
    step.cancellation = cancellation;
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(8);
    let mut run = Box::pin(adapter.run_step(step, events_tx));

    loop {
        let event = tokio::select! {
            result = &mut run => panic!("adapter finished before cancellation: {result:?}"),
            event = events_rx.recv() => event.unwrap(),
        };
        if matches!(
            event.kind,
            AdapterEventKind::Block(NormalizedBlock::Text { ref text, .. }) if text == "before"
        ) {
            break;
        }
    }
    cancel_tx.send(8).unwrap();
    let error = run.await.unwrap_err();
    let _ = release_tx.send(());

    assert_eq!(error, ProviderRuntimeError::Cancelled);
    while let Ok(event) = events_rx.try_recv() {
        assert!(!matches!(
            event.kind,
            AdapterEventKind::Block(NormalizedBlock::Text { ref text, .. }) if text == "late"
        ));
    }
    server.abort();
}

#[tokio::test]
async fn rejects_a_cancelled_generation_before_transport_admission() {
    let (endpoint, requests) = fixture(Vec::new()).await;
    let mut adapter = adapter(endpoint);
    let (cancellation_tx, cancellation) = tokio::sync::watch::channel(7);
    let mut cancelled = request(user_history(), Vec::new());
    cancelled.cancellation = cancellation;
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(8);
    cancellation_tx.send(8).unwrap();

    let outcome = adapter.run_step(cancelled, events_tx).await.unwrap();

    assert_eq!(outcome, AdapterStepOutcome::Cancelled);
    assert!(requests.lock().is_empty());
}
