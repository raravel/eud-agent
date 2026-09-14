use super::*;

#[tokio::test]
async fn production_adapter_maps_proxy_rate_limit_without_fallback() {
    let body = "{}";
    let response = format!(
        "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (base_url, requests, server) = fixture(response);
    let client = reqwest::Client::builder().build().unwrap();
    let mut adapter = ProductionOllamaAdapter::with_client(client, None);
    let (_cancel, request) = request(&base_url, Vec::new(), Vec::new());
    let (events, _received) = tokio::sync::mpsc::channel(16);

    assert_eq!(
        adapter.run_step(request, events).await,
        Err(ProviderRuntimeError::Protocol(
            "provider_rate_limited".to_string()
        ))
    );
    assert_eq!(requests.try_iter().count(), 1);
    server.join().unwrap();
}

#[tokio::test]
async fn probe_uses_the_configured_v1_path_and_optional_bearer_key() {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 4_096];
        let count = stream.read(&mut buffer).unwrap();
        sender
            .send(String::from_utf8_lossy(&buffer[..count]).into_owned())
            .unwrap();
        let body = r#"{"object":"list","data":[]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let client = reqwest::Client::builder().build().unwrap();
    probe(&client, &format!("http://{address}/v1"), Some("proxy-key"))
        .await
        .unwrap();
    let request = receiver.recv().unwrap();
    assert!(request.starts_with("GET /v1/models HTTP/1.1\r\n"));
    assert!(request
        .to_ascii_lowercase()
        .contains("authorization: bearer proxy-key\r\n"));
    server.join().unwrap();
}
