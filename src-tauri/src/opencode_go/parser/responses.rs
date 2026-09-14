use super::*;

pub(super) fn parse_responses_event(
    step: &mut NormalizedAssistantStep,
    calls: &mut Vec<ResponsesToolCall>,
    event: Option<&str>,
    value: &Value,
) -> Result<(), String> {
    let kind = event.or_else(|| value.get("type").and_then(Value::as_str));
    match kind {
        Some("response.output_text.delta") => {
            step.text.push_str(
                value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?,
            );
        }
        Some("response.reasoning_summary_text.delta") => {
            step.reasoning.push_str(
                value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?,
            );
        }
        Some("response.output_item.added" | "response.output_item.done") => {
            let item = value
                .get("item")
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                let id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?;
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?;
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match calls.iter_mut().find(|call| call.id == id) {
                    Some(_) if kind == Some("response.output_item.added") => {
                        return Err("provider returned duplicate tool call id".to_string());
                    }
                    Some(call) => {
                        if !arguments.is_empty() {
                            call.arguments = arguments.to_string();
                        }
                    }
                    None => calls.push(ResponsesToolCall {
                        id: id.to_string(),
                        item_id: item.get("id").and_then(Value::as_str).map(str::to_string),
                        name: name.to_string(),
                        arguments: arguments.to_string(),
                    }),
                }
            }
        }
        Some("response.function_call_arguments.delta") => {
            let id = value
                .get("call_id")
                .or_else(|| value.get("item_id"))
                .and_then(Value::as_str)
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            let delta = value
                .get("delta")
                .and_then(Value::as_str)
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            calls
                .iter_mut()
                .find(|call| call.id == id || call.item_id.as_deref() == Some(id))
                .ok_or_else(|| "provider returned arguments for an unknown tool call".to_string())?
                .arguments
                .push_str(delta);
        }
        Some("response.completed") => {
            step.finish_reason = Some(
                value
                    .pointer("/response/status")
                    .and_then(Value::as_str)
                    .unwrap_or("completed")
                    .to_string(),
            );
            if let Some(usage) = value.pointer("/response/usage") {
                step.usage = parse_usage(usage, None);
            }
        }
        Some("error") | Some("response.failed") => {
            return Err("provider_transport_closed".to_string())
        }
        _ => {}
    }
    Ok(())
}
