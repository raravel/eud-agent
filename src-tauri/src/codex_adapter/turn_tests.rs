use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

use super::*;
use crate::{
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{
        AgentTurnInput, BindingSnapshot, RunId, RunIdentity, RunPolicy, WorkspaceAccess,
    },
    session::SessionKind,
};

async fn read_line<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}

async fn write_line<W: tokio::io::AsyncWrite + Unpin>(writer: &mut W, value: Value) {
    writer
        .write_all(value.to_string().as_bytes())
        .await
        .unwrap();
    writer.write_all(b"\n").await.unwrap();
    writer.flush().await.unwrap();
}

fn request(cancel: tokio::sync::watch::Receiver<u64>) -> AdapterStepRequest {
    AdapterStepRequest {
        identity: RunIdentity {
            session_id: "session-1".to_string(),
            run_id: RunId::new(1),
            request_id: "request-1".to_string(),
            session_kind: SessionKind::Eps,
            cancellation_generation: 0,
        },
        binding: BindingSnapshot {
            provider: ProviderId::Codex,
            model: "gpt-test".to_string(),
            reasoning: None,
            base_url: None,
            capabilities: None,
            conversation: ProviderConversationState::Codex { thread_id: None },
        },
        kind: AdapterRequestKind::Foreground(AgentTurnInput::text("hello")),
        policy: RunPolicy {
            active_deadline: None,
            shutdown_grace: std::time::Duration::from_secs(1),
            max_output_bytes: 1024,
            max_output_tokens: None,
            max_tool_rounds: 64,
            allow_resume: true,
        },
        continuation: None,
        history: Arc::from([]),
        prior_tool_results: Arc::from([]),
        tool_descriptors: Arc::from([]),
        native_mcp_endpoint: Some("http://127.0.0.1:1/mcp".to_string()),
        cancellation: cancel,
    }
}

#[tokio::test]
async fn pre_cancelled_native_turn_does_not_wait_for_a_completion_event() {
    // Given: cancellation advanced before Codex could create a native turn.
    let (client_write, _server_read) = tokio::io::duplex(1024);
    let (_server_write, client_read) = tokio::io::duplex(1024);
    let (mut client, mut native_events) =
        CodexAppServerClient::new_with_stdio(client_read, client_write);
    let (_cancel, cancel_rx) = tokio::sync::watch::channel(1_u64);
    let request = request(cancel_rx);
    let (event_tx, _events) = tokio::sync::mpsc::channel(1);

    // When: the adapter begins the already-cancelled turn.
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        execute_native_turn(
            &mut client,
            &mut native_events,
            AgentTurnInput::text("cancelled before start"),
            &request,
            &event_tx,
        ),
    )
    .await;

    // Then: it terminates as cancelled without awaiting an impossible TurnComplete event.
    assert_eq!(outcome, Ok(Err(ProviderRuntimeError::Cancelled)));
}

#[tokio::test]
async fn production_transport_normalizes_reasoning_text_and_native_tool_observation() {
    let (client_write, server_read) = tokio::io::duplex(16 * 1024);
    let (mut server_write, client_read) = tokio::io::duplex(16 * 1024);
    let (mut client, mut native_events) =
        CodexAppServerClient::new_with_stdio(client_read, client_write);
    let expected_usage = crate::ipc::ContextUsage {
        last: crate::ipc::TokenUsageBreakdown {
            input_tokens: 31,
            cached_input_tokens: 17,
            cache_write_input_tokens: 4,
            output_tokens: 9,
            reasoning_output_tokens: 6,
            total_tokens: 40,
        },
        total: crate::ipc::TokenUsageBreakdown {
            input_tokens: 71,
            cached_input_tokens: 39,
            cache_write_input_tokens: 8,
            output_tokens: 21,
            reasoning_output_tokens: 13,
            total_tokens: 92,
        },
        model_context_window: Some(128_000),
    };
    let wire_usage = expected_usage.clone();
    let server = tokio::spawn(async move {
        let mut requests = BufReader::new(server_read);
        let initialize = read_line(&mut requests).await;
        write_line(
            &mut server_write,
            json!({"id":initialize["id"],"result":{}}),
        )
        .await;
        let _initialized = read_line(&mut requests).await;
        let start = read_line(&mut requests).await;
        write_line(&mut server_write, json!({"id":start["id"],"result":{}})).await;
        write_line(
            &mut server_write,
            json!({"method":"thread/started","params":{"thread":{"id":"thread-1"}}}),
        )
        .await;
        let turn = read_line(&mut requests).await;
        write_line(
            &mut server_write,
            json!({"id":turn["id"],"result":{"turn":{"id":"turn-1"}}}),
        )
        .await;
        write_line(
            &mut server_write,
            json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1","items":[],"status":"inProgress"}}}),
        )
        .await;
        write_line(
            &mut server_write,
            json!({"method":"item/reasoning/textDelta","params":{"delta":"why"}}),
        )
        .await;
        write_line(
            &mut server_write,
            json!({"method":"item/started","params":{"item":{"type":"webSearch","query":"docs"}}}),
        )
        .await;
        write_line(
            &mut server_write,
            json!({"method":"item/agentMessage/delta","params":{"delta":"answer"}}),
        )
        .await;
        write_line(&mut server_write, json!({"method":"thread/tokenUsage/updated","params":{"turnId":"turn-1","tokenUsage":wire_usage}})).await;
        write_line(&mut server_write, json!({"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}})).await;
    });
    let (_cancel, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let request = request(cancel_rx);
    let (event_tx, mut events) = tokio::sync::mpsc::channel(16);
    let outcome = execute_native_turn(
        &mut client,
        &mut native_events,
        AgentTurnInput::text("hello").with_access(WorkspaceAccess::Read),
        &request,
        &event_tx,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        (
            AdapterOutput::Text("answer".to_string()),
            Some("thread-1".to_string())
        )
    );
    drop(event_tx);
    let mut saw_reasoning = false;
    let mut saw_tool = false;
    let mut context_usage = None;
    while let Some(event) = events.recv().await {
        match event.kind {
            AdapterEventKind::Block(NormalizedBlock::Reasoning { text, .. }) => {
                saw_reasoning = text == "why"
            }
            AdapterEventKind::NativeToolObservation { name, .. } => saw_tool = name == "web_search",
            AdapterEventKind::Usage(usage) => context_usage = usage.context_usage,
            _ => {}
        }
    }
    assert!(saw_reasoning);
    assert!(saw_tool);
    assert_eq!(context_usage, Some(expected_usage));
    server.await.unwrap();
}
