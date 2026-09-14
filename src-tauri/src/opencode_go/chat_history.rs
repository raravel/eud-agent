use super::*;

pub(super) fn chat_runtime_history(history: &[WireHistory]) -> Vec<Value> {
    let mut messages = Vec::with_capacity(history.len());
    let mut assistant: Option<(String, usize)> = None;
    for item in history {
        match item {
            WireHistory::User { text, images, .. } if images.is_empty() => {
                messages.push(json!({"role":"user","content":text}));
                assistant = None;
            }
            WireHistory::User { text, images, .. } => {
                let mut content = vec![json!({"type":"text","text":text})];
                content.extend(images.iter().map(|image| json!({
                    "type":"image_url",
                    "image_url":{"url":format!("data:{};base64,{}",image.mime,image.data_base64)}
                })));
                messages.push(json!({"role":"user","content":content}));
                assistant = None;
            }
            WireHistory::Text { response_id, text } => {
                let message = chat_runtime_assistant(&mut messages, &mut assistant, response_id);
                append_string_field(message, "content", text);
            }
            WireHistory::Reasoning {
                response_id,
                text,
                continuation,
            } => {
                let message = chat_runtime_assistant(&mut messages, &mut assistant, response_id);
                append_string_field(message, "reasoning_content", text);
                if let Some(opaque) = history_continuation_opaque(
                    history,
                    response_id,
                    continuation,
                    "chat_completions",
                ) {
                    message["reasoning_details"] = opaque.clone();
                }
            }
            WireHistory::ToolCall {
                response_id, call, ..
            } => {
                let message = chat_runtime_assistant(&mut messages, &mut assistant, response_id);
                let encoded = json!({
                    "id":call.id,
                    "type":"function",
                    "function":{
                        "name":call.name,
                        "arguments":serde_json::to_string(&call.arguments)
                            .unwrap_or_else(|_| "{}".to_string())
                    }
                });
                match message.get_mut("tool_calls").and_then(Value::as_array_mut) {
                    Some(calls) => calls.push(encoded),
                    None => message["tool_calls"] = json!([encoded]),
                }
            }
            WireHistory::ToolResult { result, .. } => {
                messages.push(json!({
                    "role":"tool",
                    "tool_call_id":result.id,
                    "content":serde_json::to_string(&result.result)
                        .unwrap_or_else(|_| "null".to_string())
                }));
                assistant = None;
            }
            WireHistory::Compaction { summary } => {
                messages.push(json!({
                    "role":"user",
                    "content":format!("[compacted conversation]\n{summary}")
                }));
                assistant = None;
            }
        }
    }
    messages
}

pub(super) fn chat_runtime_assistant<'a>(
    messages: &'a mut Vec<Value>,
    active: &mut Option<(String, usize)>,
    response_id: &str,
) -> &'a mut Value {
    let index = match active {
        Some((active_id, index)) if active_id == response_id => *index,
        _ => {
            messages.push(json!({"role":"assistant","content":Value::Null}));
            let index = messages.len() - 1;
            *active = Some((response_id.to_string(), index));
            index
        }
    };
    &mut messages[index]
}
