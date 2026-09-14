use super::*;

pub(super) fn chat_tools(descriptors: &[Value]) -> Vec<Value> {
    descriptors
        .iter()
        .filter_map(|descriptor| {
            let name = descriptor.get("name")?.as_str()?;
            let description = descriptor
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            let schema = descriptor
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({"type":"object"}));
            Some(json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": description,
                    "parameters": schema,
                    "strict": true
                }
            }))
        })
        .collect()
}

pub(super) fn selected_output_schema(
    structured: bool,
    output_schema: Option<&Value>,
) -> Option<&Value> {
    if structured {
        output_schema
    } else {
        None
    }
}

pub(super) fn build_chat_request(
    model: &str,
    reasoning: Option<&ReasoningSelection>,
    messages: &[Value],
    tools: Vec<Value>,
    output_schema: Option<&Value>,
    max_output_tokens: Option<u64>,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true}
    });
    if let Some(max_tokens) = max_output_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(reasoning) = reasoning {
        body["reasoning_effort"] = Value::String(reasoning.level.clone());
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(schema) = output_schema {
        body["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {
                "name": STRUCTURED_OUTPUT_NAME,
                "strict": true,
                "schema": schema
            }
        });
    }
    body
}

pub(super) fn user_message(
    text: &str,
    image_paths: &[std::path::PathBuf],
) -> Result<Value, String> {
    if image_paths.is_empty() {
        return Ok(json!({"role":"user","content":text}));
    }
    let images = crate::provider_transcript::conversation_images(image_paths)?;
    let mut content = vec![json!({"type":"text","text":text})];
    content.extend(images.iter().map(|image| {
        json!({
            "type":"image_url",
        "image_url":{"url":format!("data:{};base64,{}", image.mime, image.data_base64)}
        })
    }));
    Ok(json!({"role":"user","content":content}))
}

pub(super) fn stored_user_message(
    text: &str,
    images: &[crate::provider_runtime::ConversationImage],
) -> Value {
    if images.is_empty() {
        return json!({"role":"user","content":text});
    }
    let mut content = vec![json!({"type":"text","text":text})];
    content.extend(images.iter().map(|image| {
        json!({
            "type":"image_url",
            "image_url":{"url":format!("data:{};base64,{}", image.mime, image.data_base64)}
        })
    }));
    json!({"role":"user","content":content})
}

pub(super) fn tool_result_message(result: &DirectToolResult) -> Value {
    json!({
        "role":"tool",
        "tool_call_id":result.id,
        "content":serde_json::to_string(&result.result).unwrap_or_else(|_| "null".to_string())
    })
}

#[derive(Default)]
struct AssistantMessage {
    response_id: String,
    text: String,
    reasoning: String,
    tool_calls: Vec<Value>,
}

fn flush_assistant(messages: &mut Vec<Value>, pending: &mut Option<AssistantMessage>) {
    let Some(assistant) = pending.take() else {
        return;
    };
    let mut message = json!({
        "role":"assistant",
        "content":if assistant.text.is_empty() {
            Value::Null
        } else {
            Value::String(assistant.text)
        }
    });
    if !assistant.reasoning.is_empty() {
        message["reasoning_content"] = Value::String(assistant.reasoning);
    }
    if !assistant.tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(assistant.tool_calls);
    }
    messages.push(message);
}

fn assistant_for<'a>(
    messages: &mut Vec<Value>,
    pending: &'a mut Option<AssistantMessage>,
    response_id: &str,
) -> &'a mut AssistantMessage {
    if pending
        .as_ref()
        .is_some_and(|assistant| assistant.response_id != response_id)
    {
        flush_assistant(messages, pending);
    }
    pending.get_or_insert_with(|| AssistantMessage {
        response_id: response_id.to_string(),
        ..AssistantMessage::default()
    })
}

pub(super) fn conversation_messages(items: &[ConversationItem]) -> Result<Vec<Value>, String> {
    let mut messages = Vec::with_capacity(items.len());
    let mut pending = None;
    for item in items {
        match item {
            ConversationItem::User { text, images, .. } => {
                flush_assistant(&mut messages, &mut pending);
                messages.push(stored_user_message(text, images));
            }
            ConversationItem::Compaction { summary, .. } => {
                flush_assistant(&mut messages, &mut pending);
                messages.push(
                    json!({"role":"user","content":format!("[compacted conversation]\n{summary}")}),
                );
            }
            ConversationItem::Assistant(NormalizedBlock::Text { response_id, text }) => {
                assistant_for(&mut messages, &mut pending, response_id)
                    .text
                    .push_str(text);
            }
            ConversationItem::Assistant(NormalizedBlock::Reasoning {
                response_id, text, ..
            }) => {
                assistant_for(&mut messages, &mut pending, response_id)
                    .reasoning
                    .push_str(text);
            }
            ConversationItem::Assistant(NormalizedBlock::ToolCall {
                response_id, call, ..
            }) => {
                let encoded = serde_json::to_string(&call.arguments)
                    .map_err(|_| "provider_protocol_changed".to_string())?;
                assistant_for(&mut messages, &mut pending, response_id)
                    .tool_calls
                    .push(json!({
                        "id":call.id,
                        "type":"function",
                        "function":{"name":call.name,"arguments":encoded}
                    }));
            }
            ConversationItem::Assistant(NormalizedBlock::ToolResult { result, .. }) => {
                flush_assistant(&mut messages, &mut pending);
                messages.push(tool_result_message(result));
            }
        }
    }
    flush_assistant(&mut messages, &mut pending);
    Ok(messages)
}
