use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    provider::ProviderId,
    provider_runtime::{NormalizedUsage, ProviderContinuation, ProviderRuntimeError},
};

pub(in crate::antigravity_client) fn thought_continuation(
    part: &Value,
) -> Result<Option<ProviderContinuation>, ProviderRuntimeError> {
    let Some(signature) = part.get("thoughtSignature").and_then(Value::as_str) else {
        return Ok(None);
    };
    if signature.is_empty() {
        return Err(ProviderRuntimeError::Protocol(
            "provider_protocol_changed".into(),
        ));
    }
    let continuation = ProviderContinuation {
        provider: ProviderId::Antigravity,
        data: json!({"thoughtSignature":signature}),
    };
    continuation.validate(ProviderId::Antigravity)?;
    Ok(Some(continuation))
}

pub(in crate::antigravity_client) fn parse_usage(value: &Value) -> Option<NormalizedUsage> {
    let last = crate::ipc::TokenUsageBreakdown {
        input_tokens: value
            .get("promptTokenCount")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        cached_input_tokens: value
            .get("cachedContentTokenCount")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        cache_write_input_tokens: 0,
        output_tokens: value
            .get("candidatesTokenCount")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        reasoning_output_tokens: value
            .get("thoughtsTokenCount")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        total_tokens: value
            .get("totalTokenCount")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    };
    Some(NormalizedUsage {
        context_usage: Some(crate::ipc::ContextUsage {
            last: last.clone(),
            total: last,
            model_context_window: None,
        }),
        input_tokens: value.get("promptTokenCount").and_then(Value::as_u64),
        cached_input_tokens: value.get("cachedContentTokenCount").and_then(Value::as_u64),
        output_tokens: value.get("candidatesTokenCount").and_then(Value::as_u64),
        total_tokens: value.get("totalTokenCount").and_then(Value::as_u64),
        provider_details: value
            .get("thoughtsTokenCount")
            .cloned()
            .map(|tokens| json!({"reasoningOutputTokens":tokens})),
    })
}

pub(super) fn signed_session_id(session_id: &str) -> String {
    let digest = Sha256::digest(session_id.as_bytes());
    i64::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ])
    .to_string()
}

pub(super) fn stable_uuid(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let a = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    let b = u16::from_be_bytes([digest[4], digest[5]]);
    let c = u16::from_be_bytes([digest[6], digest[7]]) & 0x0fff;
    let d = u16::from_be_bytes([digest[8], digest[9]]) & 0x0fff;
    let e = u64::from_be_bytes([
        digest[8], digest[9], digest[10], digest[11], digest[12], digest[13], digest[14],
        digest[15],
    ]) & 0x0000_ffff_ffff_ffff;
    format!("{a:08x}-{b:04x}-4{c:03x}-a{d:03x}-{e:012x}")
}
