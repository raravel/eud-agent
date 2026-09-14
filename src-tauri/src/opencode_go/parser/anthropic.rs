use super::*;

pub(super) fn parse_anthropic_event(
    step: &mut NormalizedAssistantStep,
    calls: &mut BTreeMap<u64, (String, String, String)>,
    event: Option<&str>,
    value: &Value,
) -> Result<(), String> {
    let kind = event.or_else(|| value.get("type").and_then(Value::as_str));
    match kind {
        Some("message_start") => {
            step.usage = value
                .pointer("/message/usage")
                .and_then(|usage| parse_usage(usage, None))
        }
        Some("content_block_start") => {
            let index = value
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            let block = value
                .get("content_block")
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?;
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?;
                let arguments = block
                    .get("input")
                    .filter(|input| input.as_object().is_some_and(|input| !input.is_empty()))
                    .map(|input| serde_json::to_string(input).unwrap_or_default())
                    .unwrap_or_default();
                calls.insert(index, (id.to_string(), name.to_string(), arguments));
            }
        }
        Some("content_block_delta") => {
            let index = value
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            let delta = value
                .get("delta")
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => step.text.push_str(
                    delta
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "provider_protocol_changed".to_string())?,
                ),
                Some("thinking_delta") => step.reasoning.push_str(
                    delta
                        .get("thinking")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "provider_protocol_changed".to_string())?,
                ),
                Some("input_json_delta") => calls
                    .entry(index)
                    .or_insert_with(|| (String::new(), String::new(), String::new()))
                    .2
                    .push_str(
                        delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .ok_or_else(|| "provider_protocol_changed".to_string())?,
                    ),
                _ => {}
            }
        }
        Some("message_delta") => {
            step.finish_reason = value
                .pointer("/delta/stop_reason")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(usage) = value.get("usage") {
                step.usage = parse_usage(usage, step.usage.as_ref());
            }
        }
        Some("error") => return Err("provider_transport_closed".to_string()),
        _ => {}
    }
    Ok(())
}
