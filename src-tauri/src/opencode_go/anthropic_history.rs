use super::*;

pub(super) fn anthropic_runtime_history(history: &[WireHistory]) -> Vec<Value> {
    let mut messages = Vec::with_capacity(history.len());
    let mut assistant: Option<(String, usize)> = None;
    let mut results: Option<(String, usize)> = None;
    let mut thinking_blocks = HashMap::<(String, u64), usize>::new();
    for item in history {
        match item {
            WireHistory::User { text, images, .. } => {
                let mut content = vec![json!({"type":"text","text":text})];
                content.extend(images.iter().map(|image| {
                    json!({
                        "type":"image",
                        "source":{"type":"base64","media_type":image.mime,"data":image.data_base64}
                    })
                }));
                messages.push(json!({"role":"user","content":content}));
                assistant = None;
                results = None;
            }
            WireHistory::Text { response_id, text } => {
                let message =
                    anthropic_runtime_assistant(&mut messages, &mut assistant, response_id);
                if let Some(content) = message["content"].as_array_mut() {
                    content.push(json!({"type":"text","text":text}));
                }
            }
            WireHistory::Reasoning {
                response_id,
                text,
                continuation,
            } => {
                if let Some(opaque) = history_continuation_opaque(
                    history,
                    response_id,
                    continuation,
                    "anthropic_messages",
                ) {
                    let message =
                        anthropic_runtime_assistant(&mut messages, &mut assistant, response_id);
                    if let Some(content) = message["content"].as_array_mut() {
                        continuation_blocks::append_anthropic_thinking(
                            content,
                            &mut thinking_blocks,
                            response_id,
                            text,
                            opaque,
                        );
                    }
                }
            }
            WireHistory::ToolCall {
                response_id, call, ..
            } => {
                let message =
                    anthropic_runtime_assistant(&mut messages, &mut assistant, response_id);
                if let Some(content) = message["content"].as_array_mut() {
                    content.push(json!({
                        "type":"tool_use",
                        "id":call.id,
                        "name":call.name,
                        "input":call.arguments
                    }));
                }
            }
            WireHistory::ToolResult {
                batch_id, result, ..
            } => {
                let index = match &results {
                    Some((active_batch, index)) if active_batch == batch_id => *index,
                    _ => {
                        messages.push(json!({"role":"user","content":[]}));
                        let index = messages.len() - 1;
                        results = Some((batch_id.clone(), index));
                        index
                    }
                };
                if let Some(content) = messages[index]["content"].as_array_mut() {
                    content.push(json!({
                        "type":"tool_result",
                        "tool_use_id":result.id,
                        "content":serde_json::to_string(&result.result)
                            .unwrap_or_else(|_| "null".to_string()),
                        "is_error":result.is_error
                    }));
                }
                assistant = None;
            }
            WireHistory::Compaction { summary } => {
                messages.push(json!({
                    "role":"user",
                    "content":[{"type":"text","text":format!("[compacted conversation]\n{summary}")}]
                }));
                assistant = None;
                results = None;
            }
        }
    }
    messages
}

pub(super) fn anthropic_runtime_assistant<'a>(
    messages: &'a mut Vec<Value>,
    active: &mut Option<(String, usize)>,
    response_id: &str,
) -> &'a mut Value {
    let index = match active {
        Some((active_id, index)) if active_id == response_id => *index,
        _ => {
            messages.push(json!({"role":"assistant","content":[]}));
            let index = messages.len() - 1;
            *active = Some((response_id.to_string(), index));
            index
        }
    };
    &mut messages[index]
}
