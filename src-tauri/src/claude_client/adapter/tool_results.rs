use serde_json::Value;

use crate::provider_runtime::ProviderRuntimeError;

use super::events::{protocol_changed, ParsedEvent, MAX_NATIVE_OBSERVATION_BYTES};
use super::parser::ClaudeStreamParser;

impl ClaudeStreamParser {
    pub(super) fn apply_message(
        &mut self,
        value: &Value,
        events: &mut Vec<ParsedEvent>,
    ) -> Result<(), ProviderRuntimeError> {
        let Some(content) = value.pointer("/message/content").and_then(Value::as_array) else {
            return Ok(());
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let call_id = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .ok_or_else(protocol_changed)?
                .to_string();
            let name = self
                .tool_names
                .remove(&call_id)
                .ok_or_else(protocol_changed)?;
            let result = block.get("content").cloned().ok_or_else(protocol_changed)?;
            if non_image_observation_bytes(&result)? > MAX_NATIVE_OBSERVATION_BYTES {
                return Err(protocol_changed());
            }
            events.push(ParsedEvent::ToolObservation {
                call_id: Some(call_id),
                name,
                arguments: None,
                result: Some(result),
                status: Some(
                    if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                        "failed"
                    } else {
                        "completed"
                    }
                    .to_string(),
                ),
            });
        }
        Ok(())
    }
}

/// Size of an echoed tool result excluding MCP `image` content blocks. Rendered
/// map images are bounded only by the raw stdout ceiling: their base64 payload
/// scales with the requested crop, not with anything the observation limit is
/// meant to catch, and the `eud-tools` observation is never forwarded anyway.
fn non_image_observation_bytes(result: &Value) -> Result<usize, ProviderRuntimeError> {
    let serialized_len = |value: &Value| {
        serde_json::to_vec(value)
            .map(|bytes| bytes.len())
            .map_err(|_| protocol_changed())
    };
    let Some(blocks) = result.as_array() else {
        return serialized_len(result);
    };
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) != Some("image"))
        .try_fold(0_usize, |total, block| {
            serialized_len(block).map(|len| total.saturating_add(len))
        })
}
