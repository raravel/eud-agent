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
            if serde_json::to_vec(&result)
                .map_err(|_| protocol_changed())?
                .len()
                > MAX_NATIVE_OBSERVATION_BYTES
            {
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
