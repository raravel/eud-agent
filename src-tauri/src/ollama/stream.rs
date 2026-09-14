use super::*;

pub(super) async fn parse_chat_stream(
    mut response: reqwest::Response,
    cancellation: &mut tokio::sync::watch::Receiver<u64>,
    generation: u64,
    max_response_bytes: usize,
    events: &tokio::sync::mpsc::Sender<AdapterEvent>,
    identity: &RunIdentity,
    response_id: &str,
) -> Result<NormalizedAssistantStep, ProviderRuntimeError> {
    let mut decoder = SseDecoder::default();
    let mut parser = ChatParser::default();
    let mut received = 0_usize;
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk
                .map_err(|_| ProviderRuntimeError::Transport("provider_transport_closed".to_string()))?,
            changed = cancellation.changed() => {
                match changed {
                    Ok(()) if *cancellation.borrow() != generation => {
                        return Err(ProviderRuntimeError::Cancelled);
                    }
                    Ok(()) => continue,
                    Err(_) => {
                        return Err(ProviderRuntimeError::Transport(
                            "provider cancellation channel closed".to_string(),
                        ));
                    }
                }
            }
        };
        let Some(chunk) = chunk else {
            return Err(ProviderRuntimeError::Protocol(
                "provider_stream_incomplete".to_string(),
            ));
        };
        received = received.checked_add(chunk.len()).ok_or_else(|| {
            ProviderRuntimeError::Protocol("provider response is too large".to_string())
        })?;
        if received > max_response_bytes {
            return Err(ProviderRuntimeError::Protocol(
                "provider response is too large".to_string(),
            ));
        }
        for event in decoder
            .push(&chunk)
            .map_err(ProviderRuntimeError::Protocol)?
        {
            if event.data == "[DONE]" {
                decoder.finish().map_err(ProviderRuntimeError::Protocol)?;
                return parser.finish().map_err(ProviderRuntimeError::Protocol);
            }
            let value: Value = serde_json::from_str(&event.data).map_err(|_| {
                ProviderRuntimeError::Protocol("provider_protocol_changed".to_string())
            })?;
            if let Some(delta) = value
                .pointer("/choices/0/delta/reasoning_content")
                .or_else(|| value.pointer("/choices/0/delta/reasoning"))
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                send_event(
                    events,
                    identity,
                    AdapterEventKind::Block(NormalizedBlock::Reasoning {
                        response_id: response_id.to_string(),
                        text: delta.to_string(),
                        continuation: None,
                    }),
                )
                .await?;
            }
            if let Some(delta) = value
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                send_event(
                    events,
                    identity,
                    AdapterEventKind::Block(NormalizedBlock::Text {
                        response_id: response_id.to_string(),
                        text: delta.to_string(),
                    }),
                )
                .await?;
            }
            parser
                .apply(&value)
                .map_err(ProviderRuntimeError::Protocol)?;
        }
    }
}

pub(super) fn normalized_usage(usage: &crate::ipc::ContextUsage) -> NormalizedUsage {
    NormalizedUsage {
        input_tokens: u64::try_from(usage.last.input_tokens).ok(),
        cached_input_tokens: u64::try_from(usage.last.cached_input_tokens).ok(),
        output_tokens: u64::try_from(usage.last.output_tokens).ok(),
        total_tokens: u64::try_from(usage.last.total_tokens).ok(),
        provider_details: None,
        context_usage: Some(usage.clone()),
    }
}

#[derive(Default)]
pub(super) struct ChatParser {
    step: NormalizedAssistantStep,
    calls: BTreeMap<u64, (String, String, String)>,
}

impl ChatParser {
    pub(super) fn apply(&mut self, value: &Value) -> Result<(), String> {
        if value.get("error").is_some() {
            return Err("provider_protocol_changed".to_string());
        }
        if let Some(usage) = value.get("usage") {
            self.step.usage = parse_usage(usage);
        }
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return Ok(());
        };
        if let Some(finish) = choice.get("finish_reason").and_then(Value::as_str) {
            self.step.finish_reason = Some(finish.to_string());
        }
        let delta = choice
            .get("delta")
            .ok_or_else(|| "provider_protocol_changed".to_string())?;
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            self.step.text.push_str(text);
        }
        if let Some(reasoning) = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(Value::as_str)
        {
            self.step.reasoning.push_str(reasoning);
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in tool_calls {
                let index = call
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?;
                let entry = self
                    .calls
                    .entry(index)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    entry.0 = id.to_string();
                }
                if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                    entry.1 = name.to_string();
                }
                if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str)
                {
                    entry.2.push_str(arguments);
                }
            }
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<NormalizedAssistantStep, String> {
        let complete = match self.step.finish_reason.as_deref() {
            Some("stop") => self.calls.is_empty(),
            Some("tool_calls" | "function_call") => !self.calls.is_empty(),
            Some(_) | None => false,
        };
        if !complete {
            return Err("provider_stream_incomplete".to_string());
        }
        for (id, name, arguments) in self.calls.into_values() {
            let arguments = serde_json::from_str(if arguments.trim().is_empty() {
                "{}"
            } else {
                &arguments
            })
            .map_err(|_| "provider returned invalid tool arguments".to_string())?;
            self.step.tool_calls.push(DirectToolCall {
                id,
                name,
                arguments,
            });
        }
        if self.step.text.is_empty() && self.step.tool_calls.is_empty() {
            return Err("provider returned an empty response".to_string());
        }
        Ok(self.step)
    }
}

pub(super) fn parse_usage(value: &Value) -> Option<crate::ipc::ContextUsage> {
    let input = value.get("prompt_tokens").and_then(Value::as_i64)?;
    let output = value
        .get("completion_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cached = value
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let reasoning = value
        .pointer("/completion_tokens_details/reasoning_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let total = value
        .get("total_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(input.saturating_add(output));
    let last = crate::ipc::TokenUsageBreakdown {
        input_tokens: input,
        cached_input_tokens: cached,
        cache_write_input_tokens: 0,
        output_tokens: output,
        reasoning_output_tokens: reasoning,
        total_tokens: total,
    };
    Some(crate::ipc::ContextUsage {
        last: last.clone(),
        total: last,
        model_context_window: None,
    })
}
