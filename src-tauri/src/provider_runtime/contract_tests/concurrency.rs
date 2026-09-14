use std::{io::Write, net::TcpListener, sync::mpsc, thread};

use crate::provider_runtime::{RunOutcome, RuntimeExecutor};

use super::{fixtures::RuntimeFixture, runtime};

struct HeldResponse {
    base_url: String,
    accepted: tokio::sync::oneshot::Receiver<()>,
    release: mpsc::Sender<()>,
    server: thread::JoinHandle<()>,
}

impl HeldResponse {
    fn new(answer: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind held HTTP fixture");
        let address = listener.local_addr().expect("held fixture address");
        let (accepted_send, accepted) = tokio::sync::oneshot::channel();
        let (release, release_receive) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept held request");
            let _ = super::fixtures::read_request(&mut stream);
            accepted_send.send(()).expect("signal held request");
            release_receive.recv().expect("release held response");
            let events = format!(
                "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{answer}\"}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{events}",
                events.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });
        Self {
            base_url: format!("http://{address}/v1"),
            accepted,
            release,
            server,
        }
    }

    async fn wait_until_accepted(&mut self) {
        tokio::time::timeout(std::time::Duration::from_secs(5), &mut self.accepted)
            .await
            .expect("runtime did not reach held HTTP response")
            .expect("HTTP fixture exited before request");
    }

    fn finish(self) {
        self.release.send(()).expect("release HTTP fixture");
        self.server.join().expect("join held HTTP fixture");
    }
}

#[tokio::test]
async fn c18_actual_eps_and_map_runtimes_overlap_and_cancel_independently() {
    // Given: independent EPS and Map runtimes with production adapters held in-flight.
    let mut eps_http = HeldResponse::new("eps late answer");
    let mut map_http = HeldResponse::new("map answer");
    let eps = RuntimeFixture::new_kind("c18-eps", crate::session::SessionKind::Eps);
    let map = eps.sibling_session("runtime-map-session", crate::session::SessionKind::Map);
    let eps_binding = eps.binding(&eps_http.base_url);
    let mut map_binding = map.binding(&map_http.base_url);
    map_binding.provider = crate::provider::ProviderId::OpencodeGo;
    map_binding.model = "fixture-chat".to_string();
    map_binding.reasoning = None;
    map_binding.base_url = None;
    map_binding.conversation = crate::provider::ProviderConversationState::OpencodeGo {
        transcript_revision: 0,
    };
    let catalog_body = r#"{"opencode-go":{"npm":"@ai-sdk/openai-compatible","models":{"fixture-chat":{"provider":{"npm":"@ai-sdk/openai-compatible"},"tool_call":true,"structured_output":true}}}}"#;
    let models_body = r#"{"data":[{"id":"fixture-chat"}]}"#;
    let json_response = |body: &str| {
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    };
    let catalog = super::fixtures::HttpFixture::scripted([json_response(catalog_body)]);
    let models = super::fixtures::HttpFixture::scripted([json_response(models_body)]);
    let map_adapter = crate::opencode_go::OpenCodeGoAdapter::new_for_test(
        reqwest::Client::new(),
        map_http.base_url.clone(),
        models.base_url.clone(),
        format!("{}/models.dev", catalog.base_url),
        "fixture-key".to_string(),
    );
    let mut eps_runtime = runtime(&eps, eps_binding.clone());
    let mut map_runtime = crate::provider_runtime::ProviderRuntime::new(
        Box::new(map_adapter),
        map_binding.clone(),
        map.dirs.clone(),
        map.root.clone(),
        map.tools.clone(),
        map.cancellation.subscribe(),
        map.events.clone(),
    )
    .expect("construct actual OpenCode Go Map runtime");
    let eps_request = eps.foreground(eps_binding, 18_001);
    let map_request = map.foreground(map_binding, 18_002);
    let eps_task = tokio::spawn(async move { eps_runtime.run_foreground(eps_request).await });
    let map_task = tokio::spawn(async move { map_runtime.run_foreground(map_request).await });
    eps_http.wait_until_accepted().await;
    map_http.wait_until_accepted().await;

    // When: only the EPS cancellation generation advances and the Map response is released.
    eps.cancellation.send(1).expect("cancel EPS runtime");
    map_http.finish();
    let eps_outcome = eps_task.await.expect("join EPS runtime");
    let map_outcome = map_task.await.expect("join Map runtime");
    eps_http.finish();
    catalog.join();
    models.join();

    // Then: EPS cancels while Map completes and neither sink receives the other's answer.
    assert_eq!(eps_outcome, RunOutcome::Cancelled);
    assert!(matches!(
        map_outcome,
        RunOutcome::Completed { ref text, .. } if text == "map answer"
    ));
    assert!(!eps.events.snapshot().iter().any(|event| {
        matches!(event, super::fixtures::EventSummary::Text(text) if text == "map answer")
    }));
    assert!(!map.events.snapshot().iter().any(|event| {
        matches!(event, super::fixtures::EventSummary::Text(text) if text == "eps late answer")
    }));
}
