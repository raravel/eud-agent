use super::*;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

pub(super) const WIRES: [OpenCodeGoWire; 3] = [
    OpenCodeGoWire::Responses,
    OpenCodeGoWire::ChatCompletions,
    OpenCodeGoWire::AnthropicMessages,
];

pub(super) struct Fixture {
    pub adapter: OpenCodeGoAdapter,
    _server: Server,
}

struct Server(tokio::task::JoinHandle<()>);

impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn serve(
    wire: OpenCodeGoWire,
    first: String,
    tail: Option<(String, tokio::sync::oneshot::Receiver<()>)>,
) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let npm = match wire {
        OpenCodeGoWire::Responses => "@ai-sdk/openai",
        OpenCodeGoWire::ChatCompletions => "@ai-sdk/openai-compatible",
        OpenCodeGoWire::AnthropicMessages => "@ai-sdk/anthropic",
    };
    let server = tokio::spawn(async move {
        let bodies = [
            json!({"data":[{"id":"fixture-wire"}]}).to_string(),
            json!({"opencode-go":{"npm":npm,"models":{"fixture-wire":{
                "tool_call":true,"structured_output":true,"limit":{"output":1024,"context":128000}
            }}}})
            .to_string(),
        ];
        for body in bodies {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()
            ).as_bytes()).await.unwrap();
        }
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        let size = first.len() + tail.as_ref().map_or(0, |(body, _)| body.len());
        socket.write_all(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
        ).as_bytes()).await.unwrap();
        socket.write_all(first.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
        if let Some((body, release)) = tail {
            let _ = release.await;
            let _ = socket.write_all(body.as_bytes()).await;
        }
    });
    Fixture {
        adapter: OpenCodeGoAdapter::new_for_test(
            reqwest::Client::new(),
            format!("{base}/v1"),
            format!("{base}/v1"),
            format!("{base}/models.dev"),
            "fixture-key".to_string(),
        ),
        _server: Server(server),
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) {
    let mut bytes = Vec::new();
    loop {
        assert!(bytes.len() < 1024 * 1024);
        let mut chunk = [0_u8; 4096];
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + length {
                return;
            }
        }
    }
}

pub(super) fn frame(event: Option<&str>, value: Value) -> String {
    let event = event.map_or(String::new(), |event| format!("event: {event}\n"));
    format!("{event}data: {value}\n\n")
}

pub(super) fn text_frame(wire: OpenCodeGoWire, text: &str) -> String {
    match wire {
        OpenCodeGoWire::Responses => {
            frame(Some("response.output_text.delta"), json!({"delta":text}))
        }
        OpenCodeGoWire::ChatCompletions => {
            frame(None, json!({"choices":[{"delta":{"content":text}}]}))
        }
        OpenCodeGoWire::AnthropicMessages => frame(
            Some("content_block_delta"),
            json!({"index":0,"delta":{"type":"text_delta","text":text}}),
        ),
    }
}

pub(super) fn terminal(wire: OpenCodeGoWire, tools: bool, truncated: bool) -> String {
    match wire {
        OpenCodeGoWire::Responses => frame(
            Some("response.completed"),
            json!({"response":{"status":if truncated {"incomplete"} else {"completed"}}}),
        ),
        OpenCodeGoWire::ChatCompletions => {
            frame(
                None,
                json!({"choices":[{"delta":{},"finish_reason":if truncated {"length"} else if tools {"tool_calls"} else {"stop"}}]}),
            ) + "data: [DONE]\n\n"
        }
        OpenCodeGoWire::AnthropicMessages => {
            frame(
                Some("message_delta"),
                json!({"delta":{"stop_reason":if truncated {"max_tokens"} else if tools {"tool_use"} else {"end_turn"}}}),
            ) + &frame(Some("message_stop"), json!({"type":"message_stop"}))
        }
    }
}

pub(super) fn call_frame(wire: OpenCodeGoWire, index: usize, name: &str, args: &str) -> String {
    let id = format!("call-{index}");
    match wire {
        OpenCodeGoWire::Responses => frame(
            Some("response.output_item.added"),
            json!({"item":{"type":"function_call","id":format!("item-{index}"),"call_id":id,"name":name,"arguments":args}}),
        ),
        OpenCodeGoWire::ChatCompletions => frame(
            None,
            json!({"choices":[{"delta":{"tool_calls":[{"index":index,"id":id,"function":{"name":name,"arguments":args}}]}}]}),
        ),
        OpenCodeGoWire::AnthropicMessages => {
            frame(
                Some("content_block_start"),
                json!({"index":index,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
            ) + &frame(
                Some("content_block_delta"),
                json!({"index":index,"delta":{"type":"input_json_delta","partial_json":args}}),
            )
        }
    }
}
