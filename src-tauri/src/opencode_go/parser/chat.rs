use super::*;

pub(super) fn parse_chat_event(
    step: &mut NormalizedAssistantStep,
    calls: &mut BTreeMap<u64, (String, String, String)>,
    value: &Value,
) -> Result<(), String> {
    // OpenCode can report an upstream failure inside an HTTP 200 SSE stream.
    // Check it before the choices-only path, including after partial output.
    if value.get("error").is_some_and(|error| !error.is_null()) {
        return Err("provider_transport_closed".to_string());
    }
    if let Some(usage) = value.get("usage") {
        step.usage = parse_usage(usage, None);
    }
    let Some(choice) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return Ok(());
    };
    if let Some(finish) = choice.get("finish_reason").and_then(Value::as_str) {
        step.finish_reason = Some(finish.to_string());
    }
    let delta = choice
        .get("delta")
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    if let Some(text) = delta.get("content").and_then(Value::as_str) {
        step.text.push_str(text);
    }
    if let Some(reasoning) = delta
        .get("reasoning_content")
        .or_else(|| delta.get("reasoning"))
        .and_then(Value::as_str)
    {
        step.reasoning.push_str(reasoning);
    }
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for call in tool_calls {
            let index = call
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            let entry = calls
                .entry(index)
                .or_insert_with(|| (String::new(), String::new(), String::new()));
            if let Some(id) = call.get("id").and_then(Value::as_str) {
                entry.0 = id.to_string();
            }
            if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                entry.1 = name.to_string();
            }
            if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
                entry.2.push_str(arguments);
            }
        }
    }
    Ok(())
}
