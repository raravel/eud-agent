use serde_json::Value;

use crate::provider_runtime::{AdapterEvent, AdapterEventKind, ProviderRuntimeError, RunIdentity};

pub(super) const MAX_NATIVE_OBSERVATION_BYTES: usize = 64 * 1024;

pub(super) enum ParsedEvent {
    Text(String),
    Reasoning(String),
    ToolObservation {
        call_id: Option<String>,
        name: String,
        arguments: Option<Value>,
        result: Option<Value>,
        status: Option<String>,
    },
}

pub(super) async fn send_event(
    events: &Option<tokio::sync::mpsc::Sender<AdapterEvent>>,
    identity: &RunIdentity,
    kind: AdapterEventKind,
) -> Result<(), ProviderRuntimeError> {
    if let Some(events) = events {
        events
            .send(AdapterEvent {
                identity: identity.clone(),
                kind,
            })
            .await
            .map_err(|_| ProviderRuntimeError::StaleEvent)?;
    }
    Ok(())
}

pub(super) fn protocol_changed() -> ProviderRuntimeError {
    ProviderRuntimeError::Protocol("provider_protocol_changed".to_string())
}

pub(super) fn map_claude_error(error: &str) -> String {
    match error {
        "authentication_failed" | "oauth_org_not_allowed" => "provider_not_authenticated",
        "billing_error" => "provider_quota_exhausted",
        "rate_limit" => "provider_rate_limited",
        "model_not_found" => "provider_model_unavailable",
        "overloaded" | "server_error" => "provider_transport_closed",
        _ => "provider_protocol_changed",
    }
    .to_string()
}

pub(super) fn parse_claude_usage(
    value: &Value,
) -> Option<crate::provider_runtime::NormalizedUsage> {
    let input = u64::try_from(value.get("input_tokens")?.as_i64()?).ok()?;
    let output = u64::try_from(
        value
            .get("output_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    )
    .ok()?;
    let cached = u64::try_from(
        value
            .get("cache_read_input_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    )
    .ok()?;
    let cache_write = value
        .get("cache_creation_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if cache_write < 0 {
        return None;
    }
    let breakdown = crate::ipc::TokenUsageBreakdown {
        input_tokens: i64::try_from(input).ok()?,
        cached_input_tokens: i64::try_from(cached).ok()?,
        cache_write_input_tokens: cache_write,
        output_tokens: i64::try_from(output).ok()?,
        reasoning_output_tokens: 0,
        total_tokens: i64::try_from(input.checked_add(output)?).ok()?,
    };
    Some(crate::provider_runtime::NormalizedUsage {
        input_tokens: Some(input),
        cached_input_tokens: Some(cached),
        output_tokens: Some(output),
        total_tokens: Some(input.saturating_add(output)),
        context_usage: Some(crate::ipc::ContextUsage {
            last: breakdown.clone(),
            total: breakdown,
            model_context_window: None,
        }),
        provider_details: None,
    })
}
