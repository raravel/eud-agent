use super::*;

pub(super) struct StreamEventRequest<'a> {
    pub(super) wire: OpenCodeGoWire,
    pub(super) cancellation: &'a mut tokio::sync::watch::Receiver<u64>,
    pub(super) generation: u64,
    pub(super) events: &'a tokio::sync::mpsc::Sender<AdapterEvent>,
    pub(super) identity: &'a RunIdentity,
    pub(super) response_id: &'a str,
}

pub(super) async fn parse_stream_events(
    mut response: reqwest::Response,
    request: StreamEventRequest<'_>,
) -> Result<ParsedStream, String> {
    let StreamEventRequest {
        wire,
        cancellation,
        generation,
        events,
        identity,
        response_id,
    } = request;
    let mut decoder = SseDecoder::default();
    let mut parser = WireParser::new(wire);
    let mut continuation = ContinuationAccumulator::default();
    let mut blocks = block_order::BlockOrder::default();
    let mut terminal = false;
    let mut received_bytes = 0usize;
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(|_| "provider_transport_closed".to_string())?,
            changed = cancellation.changed() => {
                if changed.is_err() || *cancellation.borrow() != generation {
                    return Err("provider_cancelled".to_string());
                }
                continue;
            }
        };
        let Some(chunk) = chunk else {
            break;
        };
        received_bytes = received_bytes.saturating_add(chunk.len());
        if received_bytes > MAX_RESPONSE_BYTES {
            return Err("provider response is too large".to_string());
        }
        let decoded = decoder.push(&chunk)?;
        for event in decoded {
            if event.data == "[DONE]" {
                let step = parser.finish()?;
                if !response_is_complete(&step, false) {
                    return Err("provider response was incomplete".to_string());
                }
                let continuation = continuation
                    .finish(wire)
                    .map_err(|error| error.to_string())?;
                let deferred_blocks = blocks.finish(&step, continuation.as_ref(), response_id)?;
                return Ok(ParsedStream {
                    step,
                    continuation,
                    deferred_blocks,
                });
            }
            let value: Value = serde_json::from_str(&event.data)
                .map_err(|_| "provider_protocol_changed".to_string())?;
            let kind = event
                .event
                .as_deref()
                .or_else(|| value.get("type").and_then(Value::as_str));
            let event_terminal = match wire {
                OpenCodeGoWire::Responses => kind == Some("response.completed"),
                OpenCodeGoWire::ChatCompletions => false,
                OpenCodeGoWire::AnthropicMessages => kind == Some("message_stop"),
            };
            terminal |= event_terminal;
            continuation.observe(wire, event.event.as_deref(), &value);
            blocks.observe(wire, event.event.as_deref(), &value);
            let delta_blocks =
                wire_delta_blocks(wire, event.event.as_deref(), &value, response_id)?;
            for block in delta_blocks {
                if let Some(block) = blocks.push(block) {
                    send_event(events, identity, AdapterEventKind::Block(block))
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
            parser.apply(event.event.as_deref(), value)?;
        }
    }
    decoder.finish()?;
    if !terminal {
        return Err("provider_transport_closed".to_string());
    }
    let step = parser.finish()?;
    if !response_is_complete(&step, false) {
        return Err("provider response was incomplete".to_string());
    }
    let continuation = continuation
        .finish(wire)
        .map_err(|error| error.to_string())?;
    let deferred_blocks = blocks.finish(&step, continuation.as_ref(), response_id)?;
    Ok(ParsedStream {
        step,
        continuation,
        deferred_blocks,
    })
}

pub(super) fn wire_delta_blocks(
    wire: OpenCodeGoWire,
    event: Option<&str>,
    value: &Value,
    response_id: &str,
) -> Result<Vec<NormalizedBlock>, String> {
    if let Some(block) = continuation_blocks::opaque_reasoning_block(
        wire,
        event.or_else(|| value.get("type").and_then(Value::as_str)),
        value,
        response_id,
    )? {
        return Ok(vec![block]);
    }
    let (text, reasoning) = match wire {
        OpenCodeGoWire::Responses => {
            match event.or_else(|| value.get("type").and_then(Value::as_str)) {
                Some("response.output_text.delta") => {
                    (value.get("delta").and_then(Value::as_str), None)
                }
                Some("response.reasoning_summary_text.delta") => {
                    (None, value.get("delta").and_then(Value::as_str))
                }
                _ => (None, None),
            }
        }
        OpenCodeGoWire::ChatCompletions => {
            let delta = value.pointer("/choices/0/delta");
            (
                delta
                    .and_then(|item| item.get("content"))
                    .and_then(Value::as_str),
                delta
                    .and_then(|item| {
                        item.get("reasoning_content")
                            .or_else(|| item.get("reasoning"))
                    })
                    .and_then(Value::as_str),
            )
        }
        OpenCodeGoWire::AnthropicMessages => {
            let delta = value.get("delta");
            match delta
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
            {
                Some("text_delta") => (
                    delta
                        .and_then(|item| item.get("text"))
                        .and_then(Value::as_str),
                    None,
                ),
                Some("thinking_delta") => (
                    None,
                    delta
                        .and_then(|item| item.get("thinking"))
                        .and_then(Value::as_str),
                ),
                _ => (None, None),
            }
        }
    };
    let mut blocks = Vec::with_capacity(2);
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        blocks.push(NormalizedBlock::Text {
            response_id: response_id.to_string(),
            text: text.to_string(),
        });
    }
    if let Some(reasoning) = reasoning.filter(|reasoning| !reasoning.is_empty()) {
        blocks.push(NormalizedBlock::Reasoning {
            response_id: response_id.to_string(),
            text: reasoning.to_string(),
            continuation: None,
        });
    }
    Ok(blocks)
}

#[cfg(test)]
pub(super) async fn parse_stream(
    mut response: reqwest::Response,
    wire: OpenCodeGoWire,
    cancellation: &mut tokio::sync::watch::Receiver<u64>,
    generation: u64,
) -> Result<NormalizedAssistantStep, String> {
    let mut decoder = SseDecoder::default();
    let mut parser = WireParser::new(wire);
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(|_| "provider_transport_closed".to_string())?,
            changed = cancellation.changed() => {
                if changed.is_err() || *cancellation.borrow() != generation {
                    return Err("provider_cancelled".to_string());
                }
                continue;
            }
        };
        let Some(chunk) = chunk else { break };
        for event in decoder.push(&chunk)? {
            if event.data == "[DONE]" {
                return parser.finish();
            }
            let value: Value = serde_json::from_str(&event.data)
                .map_err(|_| "provider_protocol_changed".to_string())?;
            parser.apply(event.event.as_deref(), value)?;
        }
    }
    decoder.finish()?;
    parser.finish()
}
