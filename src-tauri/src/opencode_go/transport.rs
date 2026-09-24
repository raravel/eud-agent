use super::*;

pub(super) fn append_string_field(message: &mut Value, field: &str, delta: &str) {
    match &mut message[field] {
        Value::String(value) => value.push_str(delta),
        value => *value = Value::String(delta.to_string()),
    }
}

pub(super) fn response_is_complete(step: &NormalizedAssistantStep, structured: bool) -> bool {
    let terminal = match step.finish_reason.as_deref() {
        Some("completed") => true,
        Some("stop" | "end_turn" | "stop_sequence") => step.tool_calls.is_empty(),
        Some("tool_calls" | "tool_use" | "function_call") => !step.tool_calls.is_empty(),
        Some(_) | None => false,
    };
    if !terminal {
        return false;
    }
    if structured {
        return step.tool_calls.len() == 1
            && step.tool_calls[0].name == STRUCTURED_TOOL
            && step.tool_calls[0].arguments.is_object();
    }
    !step.text.is_empty() || !step.tool_calls.is_empty()
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

pub(super) async fn send_event(
    sender: &tokio::sync::mpsc::Sender<AdapterEvent>,
    identity: &RunIdentity,
    kind: AdapterEventKind,
) -> Result<(), ProviderRuntimeError> {
    sender
        .send(AdapterEvent {
            identity: identity.clone(),
            kind,
        })
        .await
        .map_err(|_| ProviderRuntimeError::Cancelled)
}

pub(super) fn map_stream_error(error: String) -> ProviderRuntimeError {
    match error.as_str() {
        "provider_cancelled" => ProviderRuntimeError::Cancelled,
        "provider_transport_closed" => ProviderRuntimeError::Transport(error),
        _ => ProviderRuntimeError::Protocol(error),
    }
}

pub(super) fn wire_path(wire: OpenCodeGoWire) -> &'static str {
    match wire {
        OpenCodeGoWire::Responses => "responses",
        OpenCodeGoWire::ChatCompletions => "chat/completions",
        OpenCodeGoWire::AnthropicMessages => "messages",
    }
}

pub(super) fn wire_name(wire: OpenCodeGoWire) -> &'static str {
    match wire {
        OpenCodeGoWire::Responses => "responses",
        OpenCodeGoWire::ChatCompletions => "chat_completions",
        OpenCodeGoWire::AnthropicMessages => "anthropic_messages",
    }
}

pub(super) fn authenticated_request(
    client: &reqwest::Client,
    url: String,
    wire: OpenCodeGoWire,
    api_key: &str,
    session_id: &str,
) -> reqwest::RequestBuilder {
    let request = match wire {
        OpenCodeGoWire::AnthropicMessages => client
            .post(url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        OpenCodeGoWire::Responses | OpenCodeGoWire::ChatCompletions => {
            client.post(url).bearer_auth(api_key)
        }
    };
    request.header("x-opencode-session", session_id)
}

pub(super) fn parse_usage(
    value: &Value,
    prior: Option<&crate::ipc::ContextUsage>,
) -> Option<crate::ipc::ContextUsage> {
    let input = value
        .get("input_tokens")
        .or_else(|| value.get("prompt_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| prior.map(|usage| usage.last.input_tokens).unwrap_or(0));
    let output = value
        .get("output_tokens")
        .or_else(|| value.get("completion_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| prior.map(|usage| usage.last.output_tokens).unwrap_or(0));
    let cached = value
        .get("cached_tokens")
        .or_else(|| value.pointer("/input_tokens_details/cached_tokens"))
        .or_else(|| value.pointer("/prompt_tokens_details/cached_tokens"))
        .or_else(|| value.get("cache_read_input_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            prior
                .map(|usage| usage.last.cached_input_tokens)
                .unwrap_or(0)
        });
    let cache_write = value
        .get("cache_creation_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            prior
                .map(|usage| usage.last.cache_write_input_tokens)
                .unwrap_or(0)
        });
    let reasoning = value
        .get("reasoning_tokens")
        .or_else(|| value.pointer("/output_tokens_details/reasoning_tokens"))
        .or_else(|| value.pointer("/completion_tokens_details/reasoning_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            prior
                .map(|usage| usage.last.reasoning_output_tokens)
                .unwrap_or(0)
        });
    let total = value
        .get("total_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(input.saturating_add(output));
    let last = crate::ipc::TokenUsageBreakdown {
        input_tokens: input,
        cached_input_tokens: cached,
        cache_write_input_tokens: cache_write,
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

pub(super) async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err("provider response is too large".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "provider_transport_closed".to_string())?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("provider response is too large".to_string());
    }
    Ok(bytes.to_vec())
}

pub(super) fn status_error(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        401 => "provider_not_authenticated",
        402 | 403 => "provider_quota_exhausted",
        404 => "provider_model_unavailable",
        429 => "provider_rate_limited",
        500..=599 => "provider_transport_closed",
        _ => "provider_protocol_changed",
    }
    .to_string()
}

const MAX_ERROR_DETAIL_CHARS: usize = 300;

/// Maps a failed inference response to its status code and keeps the provider's own
/// error message so the user can tell a per-model window limit from a dead key.
pub(super) async fn inference_status_error(response: reqwest::Response) -> String {
    let status = response.status();
    let code = status_error(status);
    let detail = bounded_body(response)
        .await
        .ok()
        .and_then(|bytes| error_detail(&bytes));
    match detail {
        Some(detail) => format!("{code} (HTTP {}): {detail}", status.as_u16()),
        None => format!("{code} (HTTP {})", status.as_u16()),
    }
}

fn error_detail(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let message = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .or_else(|| value.get("error"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.to_string());
    let detail: String = message
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_ERROR_DETAIL_CHARS)
        .collect();
    (!detail.is_empty()).then_some(detail)
}
