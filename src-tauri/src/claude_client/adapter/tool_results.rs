use serde_json::{json, Value};

use super::events::{omitted_observation, ParsedEvent, MAX_NATIVE_OBSERVATION_BYTES};
use super::parser::ClaudeStreamParser;

impl ClaudeStreamParser {
    pub(super) fn apply_message(&mut self, value: &Value, events: &mut Vec<ParsedEvent>) {
        let Some(content) = value.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    self.apply_unstreamed_call(block, events);
                    continue;
                }
                Some("tool_result") => {}
                _ => continue,
            }
            let Some(call_id) = block.get("tool_use_id").and_then(Value::as_str) else {
                continue;
            };
            // A result for a call this run never saw starting (a server tool's,
            // or one the CLI ran under a name it reports differently) is the
            // CLI's own business; the turn goes on without an observation.
            let Some(name) = self.tool_names.remove(call_id) else {
                continue;
            };
            let mut result = block.get("content").cloned().unwrap_or(Value::Null);
            let bytes = non_image_observation_bytes(&result);
            if bytes > MAX_NATIVE_OBSERVATION_BYTES {
                result = omitted_observation(bytes);
            }
            events.push(ParsedEvent::ToolObservation {
                call_id: Some(call_id.to_string()),
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
    }

    /// A `tool_use` the assistant message carries but the stream never
    /// started, which happens when the CLI answers a request without streaming
    /// it (its fallback after a stalled stream). Without this its result had
    /// no name to go by, and the edit it made was never claimed.
    fn apply_unstreamed_call(&mut self, block: &Value, events: &mut Vec<ParsedEvent>) {
        let (Some(call_id), Some(name)) = (
            block.get("id").and_then(Value::as_str),
            block.get("name").and_then(Value::as_str),
        ) else {
            return;
        };
        if self.tool_names.contains_key(call_id)
            || self
                .tool_parts
                .values()
                .any(|(id, _, _)| id.as_deref() == Some(call_id))
        {
            return;
        }
        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
        let bytes = serde_json::to_vec(&input).map_or(0, |bytes| bytes.len());
        let arguments = if bytes > MAX_NATIVE_OBSERVATION_BYTES {
            let mut omitted = omitted_observation(bytes);
            if let (Some(object), Some(fields)) = (omitted.as_object_mut(), input.as_object()) {
                for key in ["file_path", "notebook_path"] {
                    if let Some(path) = fields.get(key).filter(|path| path.is_string()) {
                        object.insert(key.to_string(), path.clone());
                    }
                }
            }
            omitted
        } else {
            input
        };
        self.tool_names
            .insert(call_id.to_string(), name.to_string());
        events.push(ParsedEvent::ToolObservation {
            call_id: Some(call_id.to_string()),
            name: name.to_string(),
            arguments: Some(arguments),
            result: None,
            status: Some("started".to_string()),
        });
    }
}

/// Size of an echoed tool result excluding MCP `image` content blocks. Rendered
/// map images are bounded only by the raw stdout ceiling: their base64 payload
/// scales with the requested crop, not with anything the observation limit is
/// meant to catch, and the `eud-tools` observation is never forwarded anyway.
fn non_image_observation_bytes(result: &Value) -> usize {
    let serialized_len = |value: &Value| serde_json::to_vec(value).map_or(0, |bytes| bytes.len());
    let Some(blocks) = result.as_array() else {
        return serialized_len(result);
    };
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) != Some("image"))
        .fold(0_usize, |total, block| {
            total.saturating_add(serialized_len(block))
        })
}
