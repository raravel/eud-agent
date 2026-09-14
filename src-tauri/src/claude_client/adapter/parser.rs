use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::provider_runtime::{NormalizedUsage, ProviderRuntimeError};

use super::events::{
    map_claude_error, parse_claude_usage, protocol_changed, ParsedEvent,
    MAX_NATIVE_OBSERVATION_BYTES,
};

#[derive(Default)]
pub(super) struct ClaudeStreamParser {
    pub(super) initialized: bool,
    tools: Vec<String>,
    mcp_ready: bool,
    pub(super) session_id: Option<String>,
    pub(super) result: Option<String>,
    result_seen: bool,
    pub(super) usage: Option<NormalizedUsage>,
    pub(super) is_error: bool,
    pub(super) error_code: Option<String>,
    tool_parts: BTreeMap<u64, (Option<String>, String, String)>,
    pub(super) tool_names: BTreeMap<String, String>,
}

impl ClaudeStreamParser {
    pub(super) fn apply(
        &mut self,
        value: &Value,
    ) -> Result<Vec<ParsedEvent>, ProviderRuntimeError> {
        if self.result_seen {
            return Err(protocol_changed());
        }
        let mut events = Vec::new();
        match value.get("type").and_then(Value::as_str) {
            Some("system") if value.get("subtype").and_then(Value::as_str) == Some("init") => {
                self.initialized = true;
                self.session_id = value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.tools = value
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(protocol_changed)?
                    .iter()
                    .map(|tool| {
                        tool.as_str()
                            .map(str::to_string)
                            .ok_or_else(protocol_changed)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                self.mcp_ready = value
                    .get("mcp_servers")
                    .and_then(Value::as_array)
                    .is_some_and(|servers| {
                        servers.iter().any(|server| {
                            server.get("name").and_then(Value::as_str) == Some("eud-tools")
                                && matches!(
                                    server.get("status").and_then(Value::as_str),
                                    Some("connected" | "ready")
                                )
                        })
                    });
                if value
                    .get("mcp_server_errors")
                    .and_then(Value::as_array)
                    .is_some_and(|errors| !errors.is_empty())
                    || value
                        .get("plugin_errors")
                        .and_then(Value::as_array)
                        .is_some_and(|errors| !errors.is_empty())
                {
                    return Err(protocol_changed());
                }
            }
            Some("system") if value.get("subtype").and_then(Value::as_str) == Some("api_retry") => {
                self.error_code = value
                    .get("error")
                    .and_then(Value::as_str)
                    .map(map_claude_error);
            }
            Some("stream_event") => self.apply_stream_event(
                value.get("event").ok_or_else(protocol_changed)?,
                &mut events,
            )?,
            Some("result") => {
                self.result_seen = true;
                self.session_id = value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| self.session_id.clone());
                self.result = value
                    .get("result")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.is_error = value.get("subtype").and_then(Value::as_str) != Some("success")
                    || value.get("is_error").and_then(Value::as_bool) != Some(false);
                self.usage = value.get("usage").and_then(parse_claude_usage);
                if self.is_error {
                    self.error_code = value
                        .get("subtype")
                        .and_then(Value::as_str)
                        .map(map_claude_error)
                        .or_else(|| self.error_code.clone());
                }
            }
            Some("assistant" | "user") => self.apply_message(value, &mut events)?,
            Some(_) => {}
            None => return Err(protocol_changed()),
        }
        Ok(events)
    }

    fn apply_stream_event(
        &mut self,
        event: &Value,
        events: &mut Vec<ParsedEvent>,
    ) -> Result<(), ProviderRuntimeError> {
        match event.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                if event.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use")
                {
                    let index = event
                        .get("index")
                        .and_then(Value::as_u64)
                        .ok_or_else(protocol_changed)?;
                    let name = event
                        .pointer("/content_block/name")
                        .and_then(Value::as_str)
                        .ok_or_else(protocol_changed)?
                        .to_string();
                    let call_id = event
                        .pointer("/content_block/id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    self.tool_parts
                        .insert(index, (call_id.clone(), name.clone(), String::new()));
                }
            }
            Some("content_block_delta") => {
                let delta = event.get("delta").ok_or_else(protocol_changed)?;
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => events.push(ParsedEvent::Text(
                        delta
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(protocol_changed)?
                            .to_string(),
                    )),
                    Some("thinking_delta") => events.push(ParsedEvent::Reasoning(
                        delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .ok_or_else(protocol_changed)?
                            .to_string(),
                    )),
                    Some("input_json_delta") => {
                        let index = event
                            .get("index")
                            .and_then(Value::as_u64)
                            .ok_or_else(protocol_changed)?;
                        let (_, _, arguments) = self
                            .tool_parts
                            .get_mut(&index)
                            .ok_or_else(protocol_changed)?;
                        arguments.push_str(
                            delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .ok_or_else(protocol_changed)?,
                        );
                        if arguments.len() > MAX_NATIVE_OBSERVATION_BYTES {
                            return Err(protocol_changed());
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                if let Some(index) = event.get("index").and_then(Value::as_u64) {
                    if let Some((call_id, name, arguments)) = self.tool_parts.remove(&index) {
                        let arguments = if arguments.is_empty() {
                            json!({})
                        } else {
                            serde_json::from_str(&arguments).map_err(|_| protocol_changed())?
                        };
                        if let Some(call_id) = call_id.as_ref() {
                            self.tool_names.insert(call_id.clone(), name.clone());
                        }
                        events.push(ParsedEvent::ToolObservation {
                            call_id,
                            name,
                            arguments: Some(arguments),
                            result: None,
                            status: Some("started".to_string()),
                        });
                    }
                }
            }
            Some("message_start" | "message_delta" | "message_stop") => {}
            Some(_) => {}
            None => return Err(protocol_changed()),
        }
        Ok(())
    }

    pub(super) fn validate_init(&self, require_mcp: bool) -> Result<(), ProviderRuntimeError> {
        let tools_valid = if require_mcp {
            !self.tools.is_empty()
                && self
                    .tools
                    .iter()
                    .all(|tool| tool.starts_with("mcp__eud-tools__"))
        } else {
            self.tools.is_empty()
        };
        if !self.initialized || (require_mcp && !self.mcp_ready) || !tools_valid {
            return Err(ProviderRuntimeError::Protocol(
                "provider process boundary validation failed".to_string(),
            ));
        }
        Ok(())
    }
}
