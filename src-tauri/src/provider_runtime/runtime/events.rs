use super::RuntimeEventSink;
use crate::{
    provider::ProviderId,
    provider_runtime::{AdapterEvent, AdapterEventKind, NormalizedBlock, ProviderRuntimeError},
};

pub(super) struct EventContext<'a> {
    pub(super) identity: &'a super::RunIdentity,
    pub(super) sink: Option<&'a dyn RuntimeEventSink>,
    pub(super) blocks: &'a mut Vec<NormalizedBlock>,
    pub(super) started: &'a mut Option<String>,
    pub(super) finished: &'a mut Option<(String, bool)>,
    pub(super) streamed_bytes: &'a mut usize,
    pub(super) max_output_bytes: usize,
    pub(super) expected_provider: ProviderId,
    pub(super) native_tool_events: Option<&'a super::tool_events::RunToolEvents>,
    pub(super) native_completion_in_flight: &'a mut Option<usize>,
}

fn accumulate_semantic_block(blocks: &mut Vec<NormalizedBlock>, block: NormalizedBlock) {
    match (blocks.last_mut(), block) {
        (
            Some(NormalizedBlock::Text {
                response_id: previous_response_id,
                text: previous_text,
            }),
            NormalizedBlock::Text { response_id, text },
        ) if previous_response_id.as_str() == response_id => {
            previous_text.push_str(&text);
        }
        (
            Some(NormalizedBlock::Reasoning {
                response_id: previous_response_id,
                text: previous_text,
                continuation: None,
            }),
            NormalizedBlock::Reasoning {
                response_id,
                text,
                continuation: None,
            },
        ) if previous_response_id.as_str() == response_id => {
            previous_text.push_str(&text);
        }
        (_, block) => blocks.push(block),
    }
}

pub(super) fn handle_event(
    event: AdapterEvent,
    context: EventContext<'_>,
) -> Result<(), ProviderRuntimeError> {
    let EventContext {
        identity,
        sink,
        blocks,
        started,
        finished,
        streamed_bytes,
        max_output_bytes,
        expected_provider,
        native_tool_events,
        native_completion_in_flight,
    } = context;
    if &event.identity != identity {
        return Ok(());
    }
    if finished.is_some() {
        return Err(ProviderRuntimeError::Protocol(
            "provider emitted an event after its terminal boundary".into(),
        ));
    }
    match event.kind {
        AdapterEventKind::ResponseStarted { response_id } => {
            if response_id.is_empty() || started.replace(response_id.clone()).is_some() {
                return Err(ProviderRuntimeError::Protocol(
                    "provider emitted an invalid response start".into(),
                ));
            }
            if let Some(sink) = sink {
                sink.emit(&AdapterEventKind::ResponseStarted { response_id })?;
            }
        }
        AdapterEventKind::Block(block) => {
            let block_response_id = match &block {
                NormalizedBlock::Text { response_id, .. }
                | NormalizedBlock::Reasoning { response_id, .. }
                | NormalizedBlock::ToolCall { response_id, .. }
                | NormalizedBlock::ToolResult { response_id, .. } => response_id,
            };
            if started.as_ref() != Some(block_response_id) {
                return Err(ProviderRuntimeError::Protocol(
                    "provider block does not belong to the active response".into(),
                ));
            }
            let block_bytes = serde_json::to_vec(&block)
                .map_err(|_| ProviderRuntimeError::Protocol("provider block is invalid".into()))?
                .len();
            *streamed_bytes = streamed_bytes.saturating_add(block_bytes);
            if *streamed_bytes > max_output_bytes {
                return Err(ProviderRuntimeError::Protocol(
                    "provider output exceeded its byte limit".into(),
                ));
            }
            match &block {
                NormalizedBlock::Reasoning {
                    continuation: Some(continuation),
                    ..
                }
                | NormalizedBlock::ToolCall {
                    continuation: Some(continuation),
                    ..
                } => continuation.validate(expected_provider)?,
                NormalizedBlock::Text { .. }
                | NormalizedBlock::Reasoning {
                    continuation: None, ..
                }
                | NormalizedBlock::ToolCall {
                    continuation: None, ..
                }
                | NormalizedBlock::ToolResult { .. } => {}
            }
            if let Some(sink) = sink {
                if !matches!(
                    &block,
                    NormalizedBlock::ToolCall { .. } | NormalizedBlock::ToolResult { .. }
                ) {
                    sink.emit(&AdapterEventKind::Block(block.clone()))?;
                }
            }
            accumulate_semantic_block(blocks, block);
        }
        AdapterEventKind::ResponseFinished {
            response_id,
            complete,
            finish_reason,
        } => {
            if started.as_ref() != Some(&response_id) {
                return Err(ProviderRuntimeError::Protocol(
                    "provider finish does not belong to the active response".into(),
                ));
            }
            if let Some(tool_events) = native_tool_events {
                *native_completion_in_flight = Some(tool_events.close_for_native_completion());
            }
            let complete = complete && finish_reason_is_success(finish_reason.as_deref());
            *finished = Some((response_id.clone(), complete));
            if let Some(sink) = sink {
                sink.emit(&AdapterEventKind::ResponseFinished {
                    response_id,
                    finish_reason,
                    complete,
                })?;
            }
        }
        AdapterEventKind::Usage(usage) => {
            let detail_bytes = usage
                .provider_details
                .as_ref()
                .and_then(|value| serde_json::to_vec(value).ok())
                .map_or(0, |bytes| bytes.len());
            *streamed_bytes = streamed_bytes.saturating_add(detail_bytes);
            if *streamed_bytes > max_output_bytes {
                return Err(ProviderRuntimeError::Protocol(
                    "provider output exceeded its byte limit".into(),
                ));
            }
            if let Some(sink) = sink {
                sink.emit(&AdapterEventKind::Usage(usage))?;
            }
        }
        AdapterEventKind::NativeToolObservation {
            call_id,
            mcp_server,
            name,
            arguments,
            result,
            status,
        } => {
            let observation_bytes =
                arguments
                    .iter()
                    .chain(result.iter())
                    .try_fold(0_usize, |total, value| {
                        serde_json::to_vec(value)
                            .map(|bytes| total.saturating_add(bytes.len()))
                            .map_err(|_| {
                                ProviderRuntimeError::Protocol(
                                    "provider tool observation is invalid".into(),
                                )
                            })
                    })?;
            *streamed_bytes = streamed_bytes.saturating_add(observation_bytes);
            if *streamed_bytes > max_output_bytes {
                return Err(ProviderRuntimeError::Protocol(
                    "provider output exceeded its byte limit".into(),
                ));
            }
            if let Some(sink) = sink {
                if mcp_server.as_deref() == Some(crate::mcp::SERVER_NAME) {
                    return Ok(());
                }
                sink.emit(&AdapterEventKind::NativeToolObservation {
                    call_id,
                    mcp_server,
                    name,
                    arguments,
                    result,
                    status,
                })?;
            }
        }
        AdapterEventKind::TransportClosed => {
            return Err(ProviderRuntimeError::Transport(
                "provider transport closed before the response completed".into(),
            ))
        }
    }
    Ok(())
}

fn finish_reason_is_success(reason: Option<&str>) -> bool {
    let Some(reason) = reason else {
        return true;
    };
    let normalized = reason.to_ascii_lowercase();
    ![
        "length",
        "max_tokens",
        "content_filter",
        "filter",
        "error",
        "cancel",
        "incomplete",
    ]
    .iter()
    .any(|failure| normalized.contains(failure))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        provider_runtime::ProviderContinuation,
        provider_tool_loop::{DirectToolCall, DirectToolResult},
    };
    use serde_json::json;

    fn text(response_id: &str, text: &str) -> NormalizedBlock {
        NormalizedBlock::Text {
            response_id: response_id.to_string(),
            text: text.to_string(),
        }
    }

    fn reasoning(
        response_id: &str,
        text: &str,
        continuation: Option<ProviderContinuation>,
    ) -> NormalizedBlock {
        NormalizedBlock::Reasoning {
            response_id: response_id.to_string(),
            text: text.to_string(),
            continuation,
        }
    }

    #[test]
    fn semantic_accumulator_preserves_kind_tool_response_and_continuation_boundaries() {
        let continuation = ProviderContinuation {
            provider: ProviderId::OpencodeGo,
            data: json!({"wire":"chat_completions","opaque":{"signature":"signed"}}),
        };
        let inputs = [
            text("response-1", "A"),
            text("response-1", "B"),
            text("response-2", "C"),
            reasoning("response-1", "D", None),
            reasoning("response-1", "E", None),
            text("response-1", "F"),
            NormalizedBlock::ToolCall {
                response_id: "response-1".to_string(),
                batch_id: "batch-1".to_string(),
                call: DirectToolCall {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    arguments: json!({"path":"src/main.eps"}),
                },
                continuation: Some(continuation.clone()),
            },
            text("response-1", "G"),
            text("response-1", "H"),
            NormalizedBlock::ToolResult {
                response_id: "response-1".to_string(),
                batch_id: "batch-1".to_string(),
                result: DirectToolResult {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    result: json!({"ok":true}),
                    is_error: false,
                },
            },
            reasoning("response-1", "I", None),
            reasoning("response-2", "J", None),
            reasoning("response-2", "K", Some(continuation.clone())),
            reasoning("response-2", "L", None),
            reasoning("response-2", "M", None),
        ];
        let mut blocks = Vec::new();

        for block in inputs {
            accumulate_semantic_block(&mut blocks, block);
        }

        assert_eq!(blocks.len(), 11);
        assert!(matches!(
            &blocks[0],
            NormalizedBlock::Text { response_id, text }
                if response_id == "response-1" && text == "AB"
        ));
        assert!(matches!(
            &blocks[1],
            NormalizedBlock::Text { response_id, text }
                if response_id == "response-2" && text == "C"
        ));
        assert!(matches!(
            &blocks[2],
            NormalizedBlock::Reasoning {
                response_id,
                text,
                continuation: None,
            } if response_id == "response-1" && text == "DE"
        ));
        assert!(matches!(
            &blocks[3],
            NormalizedBlock::Text { response_id, text }
                if response_id == "response-1" && text == "F"
        ));
        assert!(matches!(
            &blocks[4],
            NormalizedBlock::ToolCall {
                response_id,
                batch_id,
                call,
                continuation: Some(observed),
            } if response_id == "response-1"
                && batch_id == "batch-1"
                && call.id == "call-1"
                && call.name == "read_file"
                && observed == &continuation
        ));
        assert!(matches!(
            &blocks[5],
            NormalizedBlock::Text { response_id, text }
                if response_id == "response-1" && text == "GH"
        ));
        assert!(matches!(
            &blocks[6],
            NormalizedBlock::ToolResult {
                response_id,
                batch_id,
                result,
            } if response_id == "response-1"
                && batch_id == "batch-1"
                && result.id == "call-1"
                && result.name == "read_file"
        ));
        assert!(matches!(
            &blocks[7],
            NormalizedBlock::Reasoning {
                response_id,
                text,
                continuation: None,
            } if response_id == "response-1" && text == "I"
        ));
        assert!(matches!(
            &blocks[8],
            NormalizedBlock::Reasoning {
                response_id,
                text,
                continuation: None,
            } if response_id == "response-2" && text == "J"
        ));
        assert!(matches!(
            &blocks[9],
            NormalizedBlock::Reasoning {
                response_id,
                text,
                continuation: Some(observed),
            } if response_id == "response-2" && text == "K" && observed == &continuation
        ));
        assert!(matches!(
            &blocks[10],
            NormalizedBlock::Reasoning {
                response_id,
                text,
                continuation: None,
            } if response_id == "response-2" && text == "LM"
        ));
    }
}
