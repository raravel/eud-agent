use super::*;

pub(super) fn build_runtime_request(
    wire: OpenCodeGoWire,
    max_output_tokens: Option<u64>,
    model: &str,
    history: &[WireHistory],
    tools: Vec<Value>,
    structured: bool,
) -> Result<Value, String> {
    let entries = match wire {
        OpenCodeGoWire::Responses => responses_runtime_history(history),
        OpenCodeGoWire::ChatCompletions => chat_runtime_history(history),
        OpenCodeGoWire::AnthropicMessages => anthropic_runtime_history(history),
    };
    let mut body = match wire {
        OpenCodeGoWire::Responses => json!({
            "model": model,
            "input": entries,
            "tools": tools,
            "stream": true,
            "store": false
        }),
        OpenCodeGoWire::ChatCompletions => json!({
            "model": model,
            "messages": entries,
            "tools": tools,
            "stream": true,
            "stream_options": {"include_usage": true}
        }),
        OpenCodeGoWire::AnthropicMessages => json!({
            "model": model,
            "max_tokens": max_output_tokens
                .ok_or_else(|| "provider_capability_unsupported".to_string())?,
            "messages": entries,
            "tools": tools,
            "stream": true
        }),
    };
    if let Some(limit) = max_output_tokens {
        match wire {
            OpenCodeGoWire::Responses => body["max_output_tokens"] = json!(limit),
            OpenCodeGoWire::ChatCompletions => body["max_tokens"] = json!(limit),
            OpenCodeGoWire::AnthropicMessages => {}
        }
    }
    if tools.is_empty() {
        if let Some(object) = body.as_object_mut() {
            object.remove("tools");
        }
    } else if structured {
        match wire {
            OpenCodeGoWire::Responses => {
                body["tool_choice"] = json!({"type":"function","name":STRUCTURED_TOOL})
            }
            OpenCodeGoWire::ChatCompletions => {
                body["tool_choice"] = json!({"type":"function","function":{"name":STRUCTURED_TOOL}})
            }
            OpenCodeGoWire::AnthropicMessages => {
                body["tool_choice"] =
                    json!({"type":"tool","name":STRUCTURED_TOOL,"disable_parallel_tool_use":true})
            }
        }
    } else if wire == OpenCodeGoWire::AnthropicMessages {
        body["tool_choice"] = json!({"type":"auto","disable_parallel_tool_use":true});
    } else {
        body["parallel_tool_calls"] = Value::Bool(false);
    }
    Ok(body)
}

pub(super) fn add_structured_instructions(
    body: &mut Value,
    wire: OpenCodeGoWire,
) -> Result<(), ProviderRuntimeError> {
    match wire {
        OpenCodeGoWire::ChatCompletions => body
            .get_mut("messages")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                ProviderRuntimeError::Protocol("provider request schema changed".to_string())
            })?
            .insert(
                0,
                json!({"role":"system","content":TASK_COMPILER_RESPONSE_INSTRUCTIONS}),
            ),
        OpenCodeGoWire::Responses => {
            body["instructions"] = json!(TASK_COMPILER_RESPONSE_INSTRUCTIONS);
        }
        OpenCodeGoWire::AnthropicMessages => {
            body["system"] = json!(TASK_COMPILER_RESPONSE_INSTRUCTIONS);
        }
    }
    Ok(())
}

pub(super) fn structured_value(
    calls: &[DirectToolCall],
    _schema: &Value,
) -> Result<Value, ProviderRuntimeError> {
    let [call] = calls else {
        return Err(ProviderRuntimeError::StructuredOutputInvalid);
    };
    if call.name != STRUCTURED_TOOL || !call.arguments.is_object() {
        return Err(ProviderRuntimeError::StructuredOutputInvalid);
    }
    Ok(call.arguments.clone())
}

pub(super) fn structured_tool(schema: &Value, wire: OpenCodeGoWire) -> Value {
    match wire {
        OpenCodeGoWire::Responses => json!({
            "type": "function",
            "name": STRUCTURED_TOOL,
            "description": "Submit the final structured result.",
            "parameters": schema,
            "strict": true
        }),
        OpenCodeGoWire::ChatCompletions => json!({
            "type": "function",
            "function": {
                "name": STRUCTURED_TOOL,
                "description": "Submit the final structured result.",
                "parameters": schema,
                "strict": true
            }
        }),
        OpenCodeGoWire::AnthropicMessages => json!({
            "name": STRUCTURED_TOOL,
            "description": "Submit the final structured result.",
            "input_schema": schema
        }),
    }
}

pub(super) fn wire_tools(descriptors: &[Value], wire: OpenCodeGoWire) -> Vec<Value> {
    descriptors
        .iter()
        .filter_map(|descriptor| {
            let name = descriptor.get("name")?.as_str()?;
            let description = descriptor
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            let schema = descriptor.get("inputSchema").cloned().unwrap_or_else(|| json!({"type":"object"}));
            Some(match wire {
                OpenCodeGoWire::Responses => json!({
                    "type": "function", "name": name, "description": description,
                    "parameters": schema, "strict": false
                }),
                OpenCodeGoWire::ChatCompletions => json!({
                    "type": "function", "function": {"name": name, "description": description, "parameters": schema, "strict": false}
                }),
                OpenCodeGoWire::AnthropicMessages => json!({
                    "name": name, "description": description, "input_schema": schema
                }),
            })
        })
        .collect()
}
