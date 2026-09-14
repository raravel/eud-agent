use super::*;

#[derive(Debug)]
pub(super) enum WireHistory {
    User {
        request_id: String,
        text: String,
        images: Vec<crate::provider_runtime::ConversationImage>,
    },
    Text {
        response_id: String,
        text: String,
    },
    Reasoning {
        response_id: String,
        text: String,
        continuation: Option<ProviderContinuation>,
    },
    ToolCall {
        response_id: String,
        call: DirectToolCall,
        continuation: Option<ProviderContinuation>,
    },
    ToolResult {
        batch_id: String,
        result: DirectToolResult,
    },
    Compaction {
        summary: String,
    },
}

pub(super) fn runtime_history(
    request: &AdapterStepRequest,
) -> Result<(Vec<WireHistory>, Option<Value>, bool), ProviderRuntimeError> {
    let mut history = request
        .history
        .iter()
        .map(wire_history_entry)
        .collect::<Vec<_>>();
    if let Some(continuation) = &request.continuation {
        for item in history.iter_mut().rev() {
            match item {
                WireHistory::Reasoning {
                    continuation: slot, ..
                }
                | WireHistory::ToolCall {
                    continuation: slot, ..
                } => {
                    if slot.is_none() {
                        *slot = Some(continuation.clone());
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    match &request.kind {
        AdapterRequestKind::Foreground(turn) => {
            if !history
                .iter()
                .any(|entry| matches!(entry, WireHistory::User { request_id, .. } if request_id == &request.identity.request_id))
            {
                let images = crate::provider_transcript::conversation_images(&turn.image_paths)
                    .map_err(ProviderRuntimeError::Protocol)?;
                history.push(WireHistory::User {
                    request_id: request.identity.request_id.clone(),
                    text: turn.text.clone(),
                    images,
                });
            }
            Ok((history, turn.output_schema.clone(), turn.forbid_tools))
        }
        AdapterRequestKind::Structured {
            prompt,
            output_schema,
            ..
        } => Ok((
            vec![WireHistory::User {
                request_id: request.identity.request_id.clone(),
                text: prompt.clone(),
                images: Vec::new(),
            }],
            Some(output_schema.clone()),
            true,
        )),
    }
}

pub(super) fn wire_history_entry(item: &ConversationItem) -> WireHistory {
    match item {
        ConversationItem::User {
            request_id,
            text,
            images,
        } => WireHistory::User {
            request_id: request_id.clone(),
            text: text.clone(),
            images: images.clone(),
        },
        ConversationItem::Assistant(NormalizedBlock::Text { response_id, text }) => {
            WireHistory::Text {
                response_id: response_id.clone(),
                text: text.clone(),
            }
        }
        ConversationItem::Assistant(NormalizedBlock::Reasoning {
            response_id,
            text,
            continuation,
        }) => WireHistory::Reasoning {
            response_id: response_id.clone(),
            text: text.clone(),
            continuation: continuation.clone(),
        },
        ConversationItem::Assistant(NormalizedBlock::ToolCall {
            response_id,
            call,
            continuation,
            ..
        }) => WireHistory::ToolCall {
            response_id: response_id.clone(),
            call: call.clone(),
            continuation: continuation.clone(),
        },
        ConversationItem::Assistant(NormalizedBlock::ToolResult {
            batch_id, result, ..
        }) => WireHistory::ToolResult {
            batch_id: batch_id.clone(),
            result: result.clone(),
        },
        ConversationItem::Compaction { summary, .. } => WireHistory::Compaction {
            summary: summary.clone(),
        },
    }
}

pub(super) fn continuation_opaque<'a>(
    continuation: &'a Option<ProviderContinuation>,
    wire: &str,
) -> Option<&'a Value> {
    continuation
        .as_ref()
        .filter(|candidate| candidate.provider == ProviderId::OpencodeGo)
        .filter(|candidate| candidate.data.get("wire").and_then(Value::as_str) == Some(wire))
        .and_then(|candidate| candidate.data.get("opaque"))
}

pub(super) fn history_continuation_opaque<'a>(
    history: &'a [WireHistory],
    response_id: &str,
    direct: &'a Option<ProviderContinuation>,
    wire: &str,
) -> Option<&'a Value> {
    continuation_opaque(direct, wire).or_else(|| {
        history.iter().find_map(|item| match item {
            WireHistory::Reasoning {
                response_id: candidate,
                continuation,
                ..
            }
            | WireHistory::ToolCall {
                response_id: candidate,
                continuation,
                ..
            } if candidate == response_id => continuation_opaque(continuation, wire),
            _ => None,
        })
    })
}
