use super::*;

pub(super) type ToolCallsByBatch<'a> = HashMap<(&'a str, &'a str), Vec<(&'a str, &'a str, bool)>>;

pub(super) fn ensure_direct_provider(provider: ProviderId) -> Result<(), String> {
    if matches!(
        provider,
        ProviderId::Antigravity | ProviderId::OpencodeGo | ProviderId::Ollama
    ) {
        Ok(())
    } else {
        Err("provider transcript is available only to direct providers".to_string())
    }
}

pub(super) fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || session_id.len() > 128
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("provider transcript session id is invalid".to_string());
    }
    Ok(())
}

pub(super) fn validate_tool_coordinates(
    response_id: &str,
    batch_id: &str,
    id: &str,
    name: &str,
) -> Result<(), String> {
    validate_identifier(response_id, "response id")?;
    validate_identifier(batch_id, "batch id")?;
    validate_identifier(id, "tool-call id")?;
    validate_identifier(name, "tool name")
}

pub(super) fn validate_continuation(
    continuation: &crate::provider_runtime::ProviderContinuation,
    provider: ProviderId,
) -> Result<(), String> {
    continuation
        .validate(provider)
        .map_err(|error| error.to_string())?;
    if contains_credential_field(&continuation.data) {
        return Err("provider transcript continuation contains credential material".to_string());
    }
    Ok(())
}

pub(super) fn contains_credential_field(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(
                key.to_ascii_lowercase().replace(['_', '-'], "").as_str(),
                "authorization"
                    | "proxyauthorization"
                    | "apikey"
                    | "accesstoken"
                    | "refreshtoken"
                    | "cookie"
                    | "setcookie"
            ) || contains_credential_field(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(contains_credential_field),
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => false,
    }
}

pub(super) fn validate_identifier(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 256 {
        return Err(format!("provider transcript {label} is invalid"));
    }
    Ok(())
}

pub(super) fn validate_images(images: &[TranscriptImage]) -> Result<(), String> {
    if images.iter().any(|image| {
        image.id.is_empty()
            || image.id.len() > 128
            || !image
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || !matches!(
                image.mime_type.as_str(),
                "image/png" | "image/jpeg" | "image/webp" | "image/gif"
            )
            || image.data_base64.is_empty()
    }) {
        return Err("provider transcript image is invalid".to_string());
    }
    Ok(())
}

pub(super) fn validate_checkpoint(
    checkpoint: &TranscriptCheckpoint,
    provider: ProviderId,
) -> Result<(), String> {
    if checkpoint.blocks.len() > MAX_ENTRIES {
        return Err("provider transcript has too many blocks".to_string());
    }
    if let Some(continuation) = checkpoint.continuation.as_ref() {
        validate_continuation(continuation, provider)?;
    }

    let mut calls: ToolCallsByBatch<'_> = HashMap::new();
    let mut results: HashMap<(&str, &str), Vec<(&str, &str)>> = HashMap::new();
    let mut seen_results = HashSet::new();
    let mut result_phase = HashSet::new();
    for block in &checkpoint.blocks {
        let bytes = serde_json::to_vec(block)
            .map_err(|_| "provider transcript block cannot be serialized".to_string())?;
        if bytes.len() > MAX_ENTRY_BYTES {
            return Err("provider transcript block is too large".to_string());
        }
        match block {
            TranscriptBlock::User {
                request_id, images, ..
            } => {
                validate_identifier(request_id, "request id")?;
                validate_images(images)?;
            }
            TranscriptBlock::AssistantText { response_id, .. }
            | TranscriptBlock::AssistantReasoning { response_id, .. } => {
                validate_identifier(response_id, "response id")?;
                if let TranscriptBlock::AssistantReasoning {
                    continuation: Some(continuation),
                    ..
                } = block
                {
                    validate_continuation(continuation, provider)?;
                }
            }
            TranscriptBlock::ToolCall {
                response_id,
                batch_id,
                id,
                name,
                arguments,
                audit_only,
                continuation,
            } => {
                validate_tool_coordinates(response_id, batch_id, id, name)?;
                if !arguments.is_object() {
                    return Err("provider transcript tool arguments are incomplete".to_string());
                }
                if let Some(continuation) = continuation.as_ref() {
                    validate_continuation(continuation, provider)?;
                }
                if result_phase.contains(&(response_id.as_str(), batch_id.as_str())) {
                    return Err(
                        "provider transcript tool call appears after its batch results".to_string(),
                    );
                }
                let batch = calls
                    .entry((response_id.as_str(), batch_id.as_str()))
                    .or_default();
                if batch.iter().any(|(call_id, _, _)| *call_id == id) {
                    return Err("provider transcript contains a duplicate tool call".to_string());
                }
                batch.push((id, name, *audit_only));
            }
            TranscriptBlock::ToolResult {
                response_id,
                batch_id,
                id,
                name,
                ..
            } => {
                validate_tool_coordinates(response_id, batch_id, id, name)?;
                let key = (response_id.as_str(), batch_id.as_str(), id.as_str());
                if !seen_results.insert(key) {
                    return Err("provider transcript contains a duplicate tool result".to_string());
                }
                let Some(call) = calls
                    .get(&(response_id.as_str(), batch_id.as_str()))
                    .and_then(|batch| batch.iter().find(|(call_id, _, _)| *call_id == id))
                else {
                    return Err("provider transcript tool result has no matching call".to_string());
                };
                if call.1 != name {
                    return Err("provider transcript tool result name mismatch".to_string());
                }
                result_phase.insert((response_id.as_str(), batch_id.as_str()));
                results
                    .entry((response_id.as_str(), batch_id.as_str()))
                    .or_default()
                    .push((id, name));
            }
            TranscriptBlock::Compaction { .. } => {}
        }
    }

    validate_boundary(
        &checkpoint.boundary,
        &checkpoint.blocks,
        &calls,
        &results,
        provider,
    )
}
