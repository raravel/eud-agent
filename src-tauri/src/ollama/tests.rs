mod cancellation;
mod endpoint;
mod followup;
mod output_budget;
mod streaming;
mod structured;
use std::{sync::Arc, time::Duration};

use super::*;
use crate::{
    provider_runtime::{AgentTurnInput, BindingSnapshot, RunId, RunPolicy},
    session::SessionKind,
};

fn sse_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn fixture(
    response: String,
) -> (
    String,
    std::sync::mpsc::Receiver<String>,
    std::thread::JoinHandle<()>,
) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4_096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            bytes.extend_from_slice(&buffer[..count]);
            let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .unwrap_or(0);
            if bytes.len() >= end + 4 + length {
                break;
            }
        }
        sender
            .send(String::from_utf8_lossy(&bytes).into_owned())
            .unwrap();
        stream.write_all(response.as_bytes()).unwrap();
    });
    (format!("http://{address}/v1"), receiver, server)
}

fn request(
    base_url: &str,
    history: Vec<ConversationItem>,
    results: Vec<DirectToolResult>,
) -> (tokio::sync::watch::Sender<u64>, AdapterStepRequest) {
    let (cancel, cancellation) = tokio::sync::watch::channel(3);
    (
        cancel,
        AdapterStepRequest {
            identity: RunIdentity {
                session_id: "session-a".to_string(),
                run_id: RunId::new(9),
                request_id: "request-a".to_string(),
                session_kind: SessionKind::Eps,
                cancellation_generation: 3,
            },
            binding: BindingSnapshot {
                provider: ProviderId::Ollama,
                model: "fixture-model".to_string(),
                reasoning: Some(ReasoningSelection {
                    level: "high".to_string(),
                }),
                base_url: Some(base_url.to_string()),
                capabilities: None,
                conversation: ProviderConversationState::Ollama {
                    transcript_revision: 0,
                },
            },
            kind: AdapterRequestKind::Foreground(AgentTurnInput::text("inspect")),
            policy: RunPolicy {
                active_deadline: Some(Duration::from_secs(60)),
                shutdown_grace: Duration::from_secs(2),
                max_output_bytes: MAX_RESPONSE_BYTES,
                max_output_tokens: None,
                max_tool_rounds: 64,
                allow_resume: true,
            },
            continuation: None,
            history: Arc::from(history),
            prior_tool_results: Arc::from(results),
            tool_descriptors: Arc::from(Vec::<Value>::new()),
            native_mcp_endpoint: None,
            cancellation,
        },
    )
}

#[test]
fn base_url_normalization_accepts_local_http_and_requires_remote_tls() {
    assert_eq!(
        normalize_base_url("http://localhost:11434").unwrap(),
        crate::provider::DEFAULT_OLLAMA_BASE_URL
    );
    assert_eq!(
        normalize_base_url("https://ollama.example.test/openai/v1/").unwrap(),
        "https://ollama.example.test/openai/v1"
    );
    assert_eq!(
        normalize_base_url("http://192.168.0.5:11434/v1"),
        Err("provider_endpoint_invalid".to_string())
    );
    assert!(normalize_base_url("https://user:secret@example.test/v1").is_err());
    assert!(normalize_base_url("http://[::1]:11434/v1").is_ok());
}

#[test]
fn chat_request_uses_tools_reasoning_and_json_schema_without_unsupported_choice_fields() {
    let messages = vec![json!({"role":"user","content":"inspect"})];
    let tools = chat_tools(&[json!({
        "name": "read_file",
        "description": "Read one file",
        "inputSchema": {"type":"object","required":["path"]}
    })]);
    let body = build_chat_request(
        "qwen3:8b",
        Some(&ReasoningSelection {
            level: "high".to_string(),
        }),
        &messages,
        tools,
        Some(&json!({"type":"object","required":["ok"]})),
        None,
    );
    assert_eq!(body["reasoning_effort"], "high");
    assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert!(body.get("tool_choice").is_none());
    assert!(body.get("parallel_tool_calls").is_none());
}

#[test]
fn unstructured_chat_request_does_not_require_an_output_schema() {
    let messages = vec![json!({"role":"user","content":"hi"})];
    let body = build_chat_request(
        "gemma4:e4b",
        None,
        &messages,
        Vec::new(),
        selected_output_schema(false, None),
        None,
    );
    assert!(body.get("response_format").is_none());
}

#[test]
fn streamed_chat_parser_assembles_text_reasoning_tools_and_usage() {
    let mut parser = ChatParser::default();
    parser
        .apply(&json!({"choices":[{"delta":{"reasoning":"why","content":"done","tool_calls":[{"index":0,"id":"call-1","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}))
        .unwrap();
    parser
        .apply(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a.eps\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4,"completion_tokens":3,"total_tokens":7}}))
        .unwrap();
    let step = parser.finish().unwrap();
    assert_eq!(step.reasoning, "why");
    assert_eq!(step.text, "done");
    assert_eq!(step.tool_calls[0].id, "call-1");
    assert_eq!(step.tool_calls[0].arguments, json!({"path":"a.eps"}));
    assert_eq!(step.usage.unwrap().last.total_tokens, 7);
}

#[test]
fn history_coalesces_reasoning_text_and_tool_batch_into_one_assistant_message() {
    let call = |id: &str, path: &str| DirectToolCall {
        id: id.to_string(),
        name: "read_file".to_string(),
        arguments: json!({"path":path}),
    };
    let history = vec![
        ConversationItem::Assistant(NormalizedBlock::Reasoning {
            response_id: "response-a".to_string(),
            text: "why ".to_string(),
            continuation: None,
        }),
        ConversationItem::Assistant(NormalizedBlock::Text {
            response_id: "response-a".to_string(),
            text: "checking".to_string(),
        }),
        ConversationItem::Assistant(NormalizedBlock::ToolCall {
            response_id: "response-a".to_string(),
            batch_id: "batch-a".to_string(),
            call: call("call-1", "a.eps"),
            continuation: None,
        }),
        ConversationItem::Assistant(NormalizedBlock::ToolCall {
            response_id: "response-a".to_string(),
            batch_id: "batch-a".to_string(),
            call: call("call-2", "b.eps"),
            continuation: None,
        }),
    ];
    let messages = conversation_messages(&history).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"], "checking");
    assert_eq!(messages[0]["reasoning_content"], "why ");
    assert_eq!(messages[0]["tool_calls"].as_array().unwrap().len(), 2);
    assert!(messages[0].get("_response_id").is_none());
    assert!(messages[0].get("_batch_id").is_none());
}
