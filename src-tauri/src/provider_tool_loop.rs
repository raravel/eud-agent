//! Provider-neutral sequential tool dispatch for direct HTTP providers.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tool_exec::SessionToolRuntime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectToolResult {
    pub id: String,
    pub name: String,
    pub result: Value,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NormalizedAssistantStep {
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<DirectToolCall>,
    pub usage: Option<crate::ipc::ContextUsage>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DirectDispatchBatch {
    pub results: Vec<DirectToolResult>,
    pub stop_for_write_transition: bool,
}

#[derive(Clone)]
pub struct ProviderToolDispatcher {
    runtime: SessionToolRuntime,
}

impl ProviderToolDispatcher {
    pub fn new(runtime: SessionToolRuntime) -> Self {
        Self { runtime }
    }

    pub fn descriptors(&self) -> Vec<Value> {
        if self.runtime.kind() == crate::session::SessionKind::Map {
            crate::tools::map_mcp_tool_descriptors()
        } else {
            crate::tools::mcp_tool_descriptors()
        }
    }

    pub async fn dispatch_batch(
        &self,
        calls: Vec<DirectToolCall>,
        seen_ids: &mut HashSet<String>,
        tools_disabled: bool,
    ) -> Result<DirectDispatchBatch, String> {
        if tools_disabled && !calls.is_empty() {
            return Err("provider_structured_output_invalid".to_string());
        }
        let mut results = Vec::with_capacity(calls.len());
        let mut stop_for_write_transition = false;
        for call in calls {
            validate_tool_call(&call, seen_ids)?;
            let is_write_transition = call.name == crate::tools::REQUEST_WRITE_WORKSPACE_TOOL;
            let outcome = if call.name == crate::tools::ASK_TOOL {
                self.runtime.ask(&call.arguments).await
            } else {
                let runtime = self.runtime.clone();
                let name = call.name.clone();
                let arguments = call.arguments.clone();
                tokio::task::spawn_blocking(move || runtime.execute(&name, &arguments))
                    .await
                    .map_err(|_| "provider tool execution task failed".to_string())?
            };
            match outcome {
                Ok(value) => results.push(DirectToolResult {
                    id: call.id,
                    name: call.name,
                    result: value,
                    is_error: false,
                }),
                Err(message) => results.push(DirectToolResult {
                    id: call.id,
                    name: call.name,
                    result: Value::String(message.chars().take(16_384).collect()),
                    is_error: true,
                }),
            }
            if is_write_transition {
                stop_for_write_transition = true;
                break;
            }
        }
        Ok(DirectDispatchBatch {
            results,
            stop_for_write_transition,
        })
    }
}

fn validate_tool_call(call: &DirectToolCall, seen_ids: &mut HashSet<String>) -> Result<(), String> {
    if call.id.trim().is_empty() || call.id.len() > 256 {
        return Err("provider returned an invalid tool-call id".to_string());
    }
    if call.name.trim().is_empty() || call.name.len() > 128 {
        return Err("provider returned an invalid tool name".to_string());
    }
    if !call.arguments.is_object() {
        return Err("provider returned non-object tool arguments".to_string());
    }
    if !seen_ids.insert(call.id.clone()) {
        return Err("provider returned a duplicate tool-call id".to_string());
    }
    Ok(())
}

pub fn validate_structured_output(schema: &Value, value: &Value) -> Result<(), String> {
    let validator = jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(schema)
        .map_err(|_| "provider_structured_output_invalid".to_string())?;
    if let Err(errors) = validator.validate(value) {
        let mut details = errors
            .take(3)
            .map(|error| error.instance_path.to_string())
            .collect::<Vec<_>>();
        details.sort();
        details.dedup();
        return Err(if details.is_empty() {
            "provider_structured_output_invalid".to_string()
        } else {
            format!("provider_structured_output_invalid: {}", details.join(", "))
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_output_uses_schema_validation_not_substring_extraction() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "ok": { "type": "boolean" } },
            "required": ["ok"],
            "additionalProperties": false
        });
        assert!(validate_structured_output(&schema, &serde_json::json!({"ok": true})).is_ok());
        assert!(
            validate_structured_output(&schema, &Value::String("{\"ok\":true}".to_string()))
                .is_err()
        );
        assert!(
            validate_structured_output(&schema, &serde_json::json!({"ok": true, "extra": 1}))
                .is_err()
        );
    }

    #[test]
    fn duplicate_tool_call_ids_fail_before_dispatch() {
        let mut seen = HashSet::new();
        let call = DirectToolCall {
            id: "call-1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({}),
        };
        validate_tool_call(&call, &mut seen).unwrap();
        assert!(validate_tool_call(&call, &mut seen).is_err());
    }
}
