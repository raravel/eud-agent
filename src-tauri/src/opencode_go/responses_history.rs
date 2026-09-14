use super::*;

pub(super) fn responses_runtime_history(history: &[WireHistory]) -> Vec<Value> {
    let mut input = Vec::with_capacity(history.len());
    let mut reasoning_items = HashMap::<(String, Option<String>), usize>::new();
    let mut assistant: Option<(String, usize)> = None;
    for item in history {
        match item {
            WireHistory::User { text, images, .. } => {
                let mut content = vec![json!({"type":"input_text","text":text})];
                content.extend(images.iter().map(|image| {
                    json!({
                        "type":"input_image",
                        "image_url":format!("data:{};base64,{}",image.mime,image.data_base64)
                    })
                }));
                input.push(json!({"role":"user","content":content}));
                assistant = None;
            }
            WireHistory::Text { response_id, text } => {
                let index = ensure_responses_assistant(&mut input, &mut assistant, response_id);
                if let Some(content) = input[index]["content"].as_array_mut() {
                    content.push(json!({"type":"output_text","text":text}));
                }
            }
            WireHistory::Reasoning {
                response_id,
                continuation,
                ..
            } => {
                if let Some(opaque) = continuation_opaque(continuation, "responses") {
                    let key = (
                        response_id.clone(),
                        opaque.get("id").and_then(Value::as_str).map(str::to_string),
                    );
                    if let Some(index) = reasoning_items.get(&key) {
                        input[*index] = opaque.clone();
                    } else {
                        reasoning_items.insert(key, input.len());
                        input.push(opaque.clone());
                        assistant = None;
                    }
                }
            }
            WireHistory::ToolCall { call, .. } => {
                input.push(json!({
                    "type":"function_call",
                    "call_id":call.id,
                    "name":call.name,
                    "arguments":serde_json::to_string(&call.arguments)
                        .unwrap_or_else(|_| "{}".to_string())
                }));
                assistant = None;
            }
            WireHistory::ToolResult { result, .. } => {
                input.push(json!({
                    "type":"function_call_output",
                    "call_id":result.id,
                    "output":serde_json::to_string(&result.result)
                        .unwrap_or_else(|_| "null".to_string())
                }));
                assistant = None;
            }
            WireHistory::Compaction { summary } => {
                input.push(json!({
                    "role":"user",
                    "content":[{"type":"input_text","text":format!("[compacted conversation]\n{summary}")}]
                }));
                assistant = None;
            }
        }
    }
    input
}

pub(super) fn ensure_responses_assistant(
    input: &mut Vec<Value>,
    active: &mut Option<(String, usize)>,
    response_id: &str,
) -> usize {
    if let Some((active_id, index)) = active {
        if active_id == response_id {
            return *index;
        }
    }
    input.push(json!({"role":"assistant","content":[]}));
    let index = input.len() - 1;
    *active = Some((response_id.to_string(), index));
    index
}
