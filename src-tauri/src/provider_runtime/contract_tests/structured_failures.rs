use std::{io::Write, net::TcpListener, sync::mpsc, thread};

use serde_json::json;

use crate::provider_runtime::{ProviderRuntimeError, RunOutcome, RuntimeExecutor};

use super::{
    fixtures::{read_request, request_body, sse, transcript_root, RuntimeFixture},
    runtime,
};

#[derive(Clone, Copy)]
enum Failure {
    Error,
    Timeout,
    Drop,
}

struct StructuredServer {
    base_url: String,
    accepted: tokio::sync::oneshot::Receiver<()>,
    release: mpsc::Sender<()>,
    requests: mpsc::Receiver<String>,
    server: thread::JoinHandle<()>,
}

impl StructuredServer {
    fn new(failure: Failure) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind structured fixture");
        let address = listener.local_addr().expect("structured fixture address");
        let (captured, requests) = mpsc::channel();
        let (accepted, accepted_receiver) = tokio::sync::oneshot::channel();
        let (release, released) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().expect("accept first main turn");
            captured
                .send(read_request(&mut first))
                .expect("capture first main turn");
            first.write_all(sse(concat!(
                "data: {\"id\":\"main-before\",\"choices\":[{\"delta\":{\"reasoning_content\":\"main reason\",\"content\":\"main-kept\"},\"finish_reason\":\"stop\"}],\"usage\":{\"total_tokens\":7}}\n\n",
                "data: [DONE]\n\n"
            )).as_bytes()).expect("write first main response");
            drop(first);
            let (mut auxiliary, _) = listener.accept().expect("accept auxiliary turn");
            captured
                .send(read_request(&mut auxiliary))
                .expect("capture auxiliary turn");
            auxiliary.write_all(concat!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                "data: {\"id\":\"auxiliary\",\"choices\":[{\"delta\":{\"reasoning_content\":\"auxiliary-private\",\"content\":\"{\\\"ok\\\":\"}}],\"usage\":{\"total_tokens\":99}}\n\n"
            ).as_bytes()).expect("write partial auxiliary response");
            accepted
                .send(())
                .expect("signal auxiliary request in flight");
            match failure {
                Failure::Error => auxiliary
                    .write_all(
                        b"data: {\"error\":{\"message\":\"controlled auxiliary failure\"}}\n\n",
                    )
                    .expect("write auxiliary failure"),
                Failure::Timeout | Failure::Drop => {
                    if released.recv().is_err() {
                        return;
                    }
                    let _ = auxiliary.write_all(b"data: [DONE]\n\n");
                }
            }
            drop(auxiliary);
            let (mut followup, _) = listener.accept().expect("accept main followup");
            captured
                .send(read_request(&mut followup))
                .expect("capture main followup");
            followup.write_all(sse("data: {\"id\":\"main-after\",\"choices\":[{\"delta\":{\"content\":\"main-continued\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n").as_bytes())
                .expect("write main followup response");
        });
        Self {
            base_url: format!("http://{address}/v1"),
            accepted: accepted_receiver,
            release,
            requests,
            server,
        }
    }
}

async fn structured_failure_preserves_main(failure: Failure) {
    // Given: an actual foreground checkpoint and an auxiliary HTTP response with partial output.
    let mut server = StructuredServer::new(failure);
    let fixture = RuntimeFixture::new("structured-failure-isolation");
    let mut binding = fixture.binding(&server.base_url);
    let mut runtime = runtime(&fixture, binding.clone());
    let conversation = match runtime
        .run_foreground(fixture.foreground(binding.clone(), 50_001))
        .await
    {
        RunOutcome::Completed { conversation, .. } => conversation,
        other => panic!("main setup failed: {other:?}"),
    };
    binding.conversation = conversation.clone();
    let events = fixture.events.snapshot();
    let workspace = runtime.current_workspace();
    let project = fixture.tools.current_project_id();
    let pointer = transcript_root(&fixture.dirs, &fixture.session_id).join("current.json");
    let checkpoint = std::fs::read(&pointer).expect("read main checkpoint pointer");
    let source =
        std::fs::read(fixture.root.join("project/src/main.eps")).expect("read main source");
    let job = fixture.structured(binding.clone(), 50_002,
        json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}));
    let deadline = job
        .policy
        .active_deadline
        .expect("fixture structured deadline");

    // When: the production structured request errors, times out, or its owning Future is dropped.
    let mut execution = runtime.run_structured(job);
    match failure {
        Failure::Error => assert!(matches!(
            execution.await,
            RunOutcome::Failed(
                ProviderRuntimeError::Protocol(_) | ProviderRuntimeError::Transport(_)
            )
        )),
        Failure::Timeout | Failure::Drop => {
            tokio::select! {
                accepted = &mut server.accepted => accepted.expect("auxiliary request reached server"),
                outcome = &mut execution => panic!("auxiliary completed before controlled interruption: {outcome:?}"),
            }
            match failure {
                Failure::Timeout => {
                    tokio::time::pause();
                    tokio::time::advance(deadline).await;
                    assert_eq!(
                        execution.await,
                        RunOutcome::Failed(ProviderRuntimeError::TimedOut)
                    );
                    tokio::time::resume();
                }
                Failure::Drop => drop(execution),
                Failure::Error => unreachable!("error outcome is handled separately"),
            }
            server
                .release
                .send(())
                .expect("release cancelled auxiliary HTTP response");
        }
    }

    // Then: main content, usage, workspace and durable checkpoint stay intact, and follow-up resumes.
    assert_eq!(runtime.conversation_state(), conversation);
    assert_eq!(runtime.current_workspace(), workspace);
    assert_eq!(fixture.tools.current_project_id(), project);
    assert_eq!(fixture.events.snapshot(), events);
    assert_eq!(
        std::fs::read(pointer).expect("read preserved main checkpoint"),
        checkpoint
    );
    assert_eq!(
        std::fs::read(fixture.root.join("project/src/main.eps")).expect("read preserved source"),
        source
    );
    fixture
        .tools
        .begin_request(
            "structured-failure-followup",
            &fixture.root.join("project").to_string_lossy(),
        )
        .expect("begin main followup");
    let mut followup = fixture.foreground(binding, 50_003);
    followup.identity.request_id = "structured-failure-followup".to_string();
    assert!(matches!(runtime.run_foreground(followup).await,
        RunOutcome::Completed { ref text, .. } if text == "main-continued"));
    let _ = server.requests.recv().expect("first main wire");
    let auxiliary_wire = request_body(&server.requests.recv().expect("auxiliary wire"));
    assert_eq!(auxiliary_wire["model"], "fixture-model");
    let wire = request_body(&server.requests.recv().expect("main followup wire"));
    let messages = wire["messages"].as_array().expect("main followup messages");
    assert!(messages
        .iter()
        .any(|message| message["content"] == "main-kept"));
    assert!(!messages
        .iter()
        .any(|message| message["reasoning_content"] == "auxiliary-private"));
    assert!(!messages
        .iter()
        .any(|message| message["content"] == "{\"ok\":"));
    server.server.join().expect("join structured HTTP fixture");
}

#[tokio::test]
async fn c07_actual_structured_error_preserves_main_and_followup() {
    structured_failure_preserves_main(Failure::Error).await;
}

#[tokio::test]
async fn c07_actual_structured_timeout_preserves_main_and_followup() {
    structured_failure_preserves_main(Failure::Timeout).await;
}

#[tokio::test]
async fn c07_actual_structured_future_drop_preserves_main_and_followup() {
    structured_failure_preserves_main(Failure::Drop).await;
}
