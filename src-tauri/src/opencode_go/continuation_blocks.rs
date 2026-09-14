use super::*;

pub(super) fn opaque_reasoning_block(
    wire: OpenCodeGoWire,
    event: Option<&str>,
    value: &Value,
    response_id: &str,
) -> Result<Option<NormalizedBlock>, String> {
    let (text, opaque) = match wire {
        OpenCodeGoWire::Responses
            if matches!(
                event,
                Some("response.output_item.added" | "response.output_item.done")
            ) =>
        {
            let Some(item) = value.get("item").filter(|item| {
                item.get("type").and_then(Value::as_str) == Some("reasoning")
                    && item
                        .get("encrypted_content")
                        .and_then(Value::as_str)
                        .is_some()
            }) else {
                return Ok(None);
            };
            if item.get("id").and_then(Value::as_str).is_none() {
                return Err("provider reasoning item has no identity".into());
            }
            (String::new(), item.clone())
        }
        OpenCodeGoWire::AnthropicMessages if event == Some("content_block_delta") => {
            let delta = &value["delta"];
            let kind = delta.get("type").and_then(Value::as_str);
            if !matches!(kind, Some("thinking_delta" | "signature_delta")) {
                return Ok(None);
            }
            let index = value
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "provider thinking block has no identity".to_string())?;
            match kind {
                Some("thinking_delta") => (
                    delta
                        .get("thinking")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "provider thinking fragment is invalid".to_string())?
                        .to_string(),
                    json!({"block_index":index}),
                ),
                Some("signature_delta") => (
                    String::new(),
                    json!({"block_index":index,"signature_fragment":delta.get("signature")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "provider signature fragment is invalid".to_string())?}),
                ),
                _ => return Ok(None),
            }
        }
        OpenCodeGoWire::Responses
        | OpenCodeGoWire::ChatCompletions
        | OpenCodeGoWire::AnthropicMessages => return Ok(None),
    };
    let continuation = ProviderContinuation {
        provider: ProviderId::OpencodeGo,
        data: json!({"wire":wire_name(wire),"opaque":opaque}),
    };
    continuation
        .validate(ProviderId::OpencodeGo)
        .map_err(|error| error.to_string())?;
    Ok(Some(NormalizedBlock::Reasoning {
        response_id: response_id.to_string(),
        text,
        continuation: Some(continuation),
    }))
}

pub(super) fn append_anthropic_thinking(
    content: &mut Vec<Value>,
    blocks: &mut HashMap<(String, u64), usize>,
    response_id: &str,
    text: &str,
    opaque: &Value,
) {
    if let Some(index) = opaque.get("block_index").and_then(Value::as_u64) {
        let content_index = *blocks
            .entry((response_id.to_string(), index))
            .or_insert_with(|| {
                content.push(json!({"type":"thinking","thinking":"","signature":""}));
                content.len() - 1
            });
        let block = &mut content[content_index];
        append_string_field(block, "thinking", text);
        if let Some(signature) = opaque.get("signature_fragment").and_then(Value::as_str) {
            append_string_field(block, "signature", signature);
        }
    } else if let Some(signature) = opaque.get("signature").and_then(Value::as_str) {
        if let Some(previous) = content
            .last_mut()
            .filter(|block| block["type"] == "thinking" && block["signature"] == signature)
        {
            append_string_field(previous, "thinking", text);
        } else {
            content.push(json!({"type":"thinking","thinking":text,"signature":signature}));
        }
    }
}

#[derive(Default)]
pub(super) struct ContinuationAccumulator {
    opaque: Option<Value>,
    signature: String,
}

impl ContinuationAccumulator {
    pub(super) fn observe(&mut self, wire: OpenCodeGoWire, event: Option<&str>, value: &Value) {
        match wire {
            OpenCodeGoWire::Responses => {
                if matches!(
                    event,
                    Some("response.output_item.added" | "response.output_item.done")
                ) {
                    if let Some(item) = value
                        .get("item")
                        .filter(|item| {
                            item.get("type").and_then(Value::as_str) == Some("reasoning")
                        })
                        .filter(|item| item.get("encrypted_content").is_some())
                    {
                        self.opaque = Some(item.clone());
                    }
                }
            }
            OpenCodeGoWire::ChatCompletions => {
                if let Some(delta) = value.pointer("/choices/0/delta") {
                    if let Some(details) = delta
                        .get("reasoning_details")
                        .or_else(|| delta.get("reasoning_signature"))
                    {
                        match (self.opaque.as_mut(), details) {
                            (Some(Value::Array(current)), Value::Array(next)) => {
                                current.extend(next.iter().cloned());
                            }
                            (Some(Value::String(current)), Value::String(next)) => {
                                current.push_str(next);
                            }
                            (Some(current), next) => *current = next.clone(),
                            (None, next) => self.opaque = Some(next.clone()),
                        }
                    }
                }
            }
            OpenCodeGoWire::AnthropicMessages => {
                if event == Some("content_block_delta")
                    && value.pointer("/delta/type").and_then(Value::as_str)
                        == Some("signature_delta")
                {
                    if let Some(signature) =
                        value.pointer("/delta/signature").and_then(Value::as_str)
                    {
                        self.signature.push_str(signature);
                    }
                }
            }
        }
    }

    pub(super) fn finish(
        self,
        wire: OpenCodeGoWire,
    ) -> Result<Option<ProviderContinuation>, ProviderRuntimeError> {
        let opaque = match wire {
            OpenCodeGoWire::AnthropicMessages if !self.signature.is_empty() => {
                Some(json!({"signature":self.signature}))
            }
            OpenCodeGoWire::Responses | OpenCodeGoWire::ChatCompletions => self.opaque,
            OpenCodeGoWire::AnthropicMessages => None,
        };
        let Some(opaque) = opaque else {
            return Ok(None);
        };
        let continuation = ProviderContinuation {
            provider: ProviderId::OpencodeGo,
            data: json!({
                "wire": wire_name(wire),
                "opaque":opaque
            }),
        };
        continuation.validate(ProviderId::OpencodeGo)?;
        Ok(Some(continuation))
    }
}
