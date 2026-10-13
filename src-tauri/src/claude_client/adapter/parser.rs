use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::provider_runtime::{NormalizedUsage, ProviderRuntimeError};

use super::events::{
    map_claude_error, omitted_observation, parse_claude_usage, protocol_changed, ParsedEvent,
    MAX_NATIVE_OBSERVATION_BYTES,
};

#[derive(Default)]
pub(super) struct ClaudeStreamParser {
    pub(super) initialized: bool,
    tools: Vec<String>,
    /// The `eud-tools` status the init line reported; `None` when it listed
    /// no such server.
    mcp_status: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) result: Option<String>,
    result_seen: bool,
    pub(super) usage: Option<NormalizedUsage>,
    pub(super) is_error: bool,
    pub(super) error_code: Option<String>,
    tool_parts: BTreeMap<u64, (Option<String>, String, String)>,
    /// Streamed argument bytes of a tool block past the observation limit,
    /// which stops buffering it.
    omitted_arguments: BTreeMap<u64, usize>,
    /// The file paths an omitted tool block named before it was dropped, so
    /// its observation still says which file a large `Write` changed.
    omitted_paths: BTreeMap<u64, serde_json::Map<String, Value>>,
    pub(super) tool_names: BTreeMap<String, String>,
}

/// The `file_path`/`notebook_path` string values a tool's partial arguments
/// already carry. Claude Code streams them before a file's content, so they
/// survive when the content itself is too large to keep.
fn path_fields(partial: &str) -> serde_json::Map<String, Value> {
    let mut fields = serde_json::Map::new();
    for key in ["file_path", "notebook_path"] {
        let quoted = format!("\"{key}\"");
        let Some(start) = partial.find(&quoted) else {
            continue;
        };
        let Some(rest) = partial[start + quoted.len()..]
            .trim_start()
            .strip_prefix(':')
        else {
            continue;
        };
        if let Some(Ok(path)) = serde_json::Deserializer::from_str(rest.trim_start())
            .into_iter::<String>()
            .next()
        {
            fields.insert(key.to_string(), Value::String(path));
        }
    }
    fields
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
                self.mcp_status = value
                    .get("mcp_servers")
                    .and_then(Value::as_array)
                    .and_then(|servers| {
                        servers.iter().find(|server| {
                            server.get("name").and_then(Value::as_str) == Some("eud-tools")
                        })
                    })
                    .map(|server| {
                        server
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string()
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
                    // A named API error (assistant line or retry) beats the result subtype,
                    // which reads "success" even when authentication failed.
                    self.error_code = self.error_code.clone().or_else(|| {
                        value
                            .get("subtype")
                            .and_then(Value::as_str)
                            .map(map_claude_error)
                    });
                }
            }
            Some("assistant" | "user") => {
                if let Some(error) = value.get("error").and_then(Value::as_str) {
                    self.error_code = Some(map_claude_error(error));
                }
                self.apply_message(value, &mut events)?
            }
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
                        let partial = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .ok_or_else(protocol_changed)?;
                        if let Some(bytes) = self.omitted_arguments.get_mut(&index) {
                            *bytes = bytes.saturating_add(partial.len());
                        } else {
                            arguments.push_str(partial);
                            if arguments.len() > MAX_NATIVE_OBSERVATION_BYTES {
                                self.omitted_arguments.insert(index, arguments.len());
                                self.omitted_paths.insert(index, path_fields(arguments));
                                *arguments = String::new();
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                if let Some(index) = event.get("index").and_then(Value::as_u64) {
                    if let Some((call_id, name, arguments)) = self.tool_parts.remove(&index) {
                        let arguments = if let Some(bytes) = self.omitted_arguments.remove(&index) {
                            let mut omitted = omitted_observation(bytes);
                            if let (Some(object), Some(paths)) =
                                (omitted.as_object_mut(), self.omitted_paths.remove(&index))
                            {
                                object.extend(paths);
                            }
                            omitted
                        } else if arguments.is_empty() {
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
        if !self.initialized {
            return Err(boundary_violation("no init line"));
        }
        if require_mcp {
            let status = self.mcp_status.as_deref();
            if !matches!(status, Some("connected" | "ready")) {
                return Err(boundary_violation(format_args!(
                    "eud-tools MCP status {}",
                    status.unwrap_or("absent")
                )));
            }
            if self.tools.is_empty() {
                return Err(boundary_violation("init listed no tools"));
            }
            let unopened = self
                .tools
                .iter()
                .filter(|tool| !super::request::tool_is_authorized(tool))
                .map(String::as_str)
                .collect::<Vec<_>>();
            if !unopened.is_empty() {
                return Err(boundary_violation(format_args!(
                    "init listed unopened tools: {}",
                    tool_list(&unopened)
                )));
            }
        } else if !self.tools.is_empty() {
            let tools = self.tools.iter().map(String::as_str).collect::<Vec<_>>();
            return Err(boundary_violation(format_args!(
                "compaction init listed tools: {}",
                tool_list(&tools)
            )));
        }
        Ok(())
    }

    /// Whether the model's call to `name` may proceed, `status` being the
    /// observation's (`None` for the streamed `tool_use` block). A tool this run
    /// authorized always may. A tool the CLI opened without this run
    /// authorizing it never may. A name the CLI never opened cannot run — the
    /// CLI answers it with an error result the model corrects, which is how a
    /// model writing `fs_edit` for `mcp__eud-tools__fs_edit` recovers — so
    /// only a successful result for it ends the run.
    pub(super) fn admits_tool(&self, name: &str, require_mcp: bool, status: Option<&str>) -> bool {
        if require_mcp && super::request::tool_is_authorized(name) {
            return true;
        }
        self.initialized
            && !self.tools.iter().any(|tool| tool == name)
            && status != Some("completed")
    }
}

/// The run-ending refusal of the process boundary, naming which check refused
/// it, so a report of the error says what the CLI actually did.
pub(super) fn boundary_violation(detail: impl std::fmt::Display) -> ProviderRuntimeError {
    ProviderRuntimeError::Protocol(format!(
        "provider process boundary validation failed: {detail}"
    ))
}

/// At most a few CLI-reported names, each bounded, so the error stays one line.
pub(super) fn tool_list(names: &[&str]) -> String {
    const SHOWN: usize = 5;
    let mut shown = names
        .iter()
        .take(SHOWN)
        .map(|name| name.chars().take(80).collect::<String>())
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > SHOWN {
        shown.push_str(&format!(" (+{} more)", names.len() - SHOWN));
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::ClaudeStreamParser;
    use serde_json::json;

    fn parse_init(tools: &[&str]) -> ClaudeStreamParser {
        let mut parser = ClaudeStreamParser::default();
        parser
            .apply(&json!({
                "type": "system",
                "subtype": "init",
                "session_id": "native-session",
                "tools": tools,
                "mcp_servers": [{"name": "eud-tools", "status": "connected"}],
            }))
            .expect("an init line parses");
        parser
    }

    #[test]
    fn an_interactive_init_may_list_the_native_file_tools() {
        // What the real CLI reports once `--tools Read,Edit,...` is passed.
        // Requiring every tool to be an eud-tools MCP tool failed every
        // interactive turn with "provider process boundary validation failed".
        parse_init(&[
            "Read",
            "Edit",
            "Write",
            "Glob",
            "Grep",
            "mcp__eud-tools__read_file",
        ])
        .validate_init(true)
        .expect("the tools this run opened are authorized");
    }

    #[test]
    fn an_init_that_lists_an_unopened_tool_still_ends_the_run() {
        for unopened in ["Bash", "WebFetch", "mcp__other__read"] {
            assert!(
                parse_init(&["mcp__eud-tools__read_file", unopened])
                    .validate_init(true)
                    .is_err(),
                "{unopened} was never opened and must end the run"
            );
        }
    }

    #[test]
    fn compaction_still_runs_with_no_tools_at_all() {
        let mut parser = ClaudeStreamParser::default();
        parser
            .apply(&json!({
                "type": "system",
                "subtype": "init",
                "session_id": "native-session",
                "tools": [],
                "mcp_servers": [],
            }))
            .expect("an init line parses");
        parser
            .validate_init(false)
            .expect("compaction has no tools");
        assert!(parser.validate_init(true).is_err());
    }

    fn protocol_message(
        result: Result<(), crate::provider_runtime::ProviderRuntimeError>,
    ) -> String {
        match result {
            Err(crate::provider_runtime::ProviderRuntimeError::Protocol(message)) => message,
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }

    #[test]
    fn a_refused_init_names_the_check_that_refused_it() {
        let mut parser = ClaudeStreamParser::default();
        parser
            .apply(&json!({
                "type": "system",
                "subtype": "init",
                "session_id": "native-session",
                "tools": ["Read"],
                "mcp_servers": [{"name": "eud-tools", "status": "failed"}],
            }))
            .expect("an init line parses");
        let message = protocol_message(parser.validate_init(true));
        assert!(
            message.starts_with("provider process boundary validation failed")
                && message.contains("eud-tools MCP status failed"),
            "{message}"
        );

        let message = protocol_message(
            parse_init(&["mcp__eud-tools__read_file", "Bash"]).validate_init(true),
        );
        assert!(message.contains("unopened tools: Bash"), "{message}");

        let message = protocol_message(ClaudeStreamParser::default().validate_init(true));
        assert!(message.contains("no init line"), "{message}");
    }

    /// What the CLI prints when its OAuth pair is dead: the error is named only on the
    /// synthetic assistant message, and the result says `subtype: "success"`. Mapping the
    /// subtype reported "provider_protocol_changed" instead of asking for a new login.
    #[test]
    fn an_assistant_error_names_the_failed_run() {
        let mut parser = parse_init(&["mcp__eud-tools__read_file"]);
        parser
            .apply(&json!({
                "type": "assistant",
                "error": "authentication_failed",
                "message": {"model": "<synthetic>", "role": "assistant", "content": [
                    {"type": "text", "text": "Failed to authenticate: OAuth session expired and could not be refreshed"}
                ]},
                "session_id": "native-session",
            }))
            .expect("an assistant line parses");
        parser
            .apply(&json!({
                "type": "result",
                "subtype": "success",
                "is_error": true,
                "terminal_reason": "api_error",
                "session_id": "native-session",
                "result": "Failed to authenticate: OAuth session expired and could not be refreshed",
            }))
            .expect("a result line parses");
        assert!(parser.is_error);
        assert_eq!(
            parser.error_code.as_deref(),
            Some("provider_not_authenticated")
        );
    }

    #[test]
    fn a_failed_result_without_a_named_error_keeps_its_subtype_mapping() {
        let mut parser = parse_init(&["mcp__eud-tools__read_file"]);
        parser
            .apply(&json!({
                "type": "result",
                "subtype": "error_during_execution",
                "is_error": true,
                "session_id": "native-session",
            }))
            .expect("a result line parses");
        assert_eq!(
            parser.error_code.as_deref(),
            Some("provider_protocol_changed")
        );
    }
}
