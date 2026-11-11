use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::provider_runtime::{NormalizedUsage, ProviderRuntimeError};

use super::events::{
    map_claude_error, omitted_observation, parse_claude_usage, ParsedEvent,
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
    pub(super) tool_parts: BTreeMap<u64, (Option<String>, String, String)>,
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

/// The CLI names a lost OAuth refresh race only in the synthetic assistant text; its
/// `error` field says `server_error`.
fn failed_oauth_refresh(value: &Value) -> bool {
    value
        .pointer("/message/content")
        .and_then(Value::as_array)
        .is_some_and(|content| {
            content.iter().any(|block| {
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.starts_with("Failed to refresh OAuth token"))
            })
        })
}

/// A `user` line the CLI echoes for a message written to its stdin
/// (`--replay-user-messages`): the initial prompt, and each steer at the point
/// the CLI handed it to the model.
pub(super) fn is_user_replay(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("user")
        && value.get("isReplay").and_then(Value::as_bool) == Some(true)
}

impl ClaudeStreamParser {
    pub(super) fn has_result(&self) -> bool {
        self.result_seen
    }

    pub(super) fn apply(
        &mut self,
        value: &Value,
    ) -> Result<Vec<ParsedEvent>, ProviderRuntimeError> {
        if self.result_seen {
            // The result is the turn's answer. The CLI may outlive it (it waits up
            // to 60 seconds for its end-of-turn classifier) and print metadata,
            // side-query output or a replayed steer meanwhile; none of it changes
            // the answer, so none of it may throw the completed turn away. A steer
            // written but never echoed is handled by the process loop, which ends
            // the process at the result.
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        match value.get("type").and_then(Value::as_str) {
            Some("system") if value.get("subtype").and_then(Value::as_str) == Some("init") => {
                self.initialized = true;
                self.session_id = value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                // A malformed list leaves `tools` empty, which `validate_init`
                // refuses by name instead of as an unexplained protocol change.
                self.tools = value
                    .get("tools")
                    .and_then(Value::as_array)
                    .map(|tools| {
                        tools
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
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
                // `mcp_server_errors`/`plugin_errors` are not checked here: the
                // only server this run needs is eud-tools, whose status
                // `validate_init` checks, and a plugin the profile synced that
                // failed to load does not stop the CLI from running the turn.
            }
            Some("system") if value.get("subtype").and_then(Value::as_str) == Some("api_retry") => {
                self.error_code = value
                    .get("error")
                    .and_then(Value::as_str)
                    .map(map_claude_error);
            }
            Some("stream_event") => {
                if let Some(event) = value.get("event") {
                    self.apply_stream_event(event, &mut events);
                }
            }
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
                    self.error_code = Some(if failed_oauth_refresh(value) {
                        "provider_auth_refresh_failed".to_string()
                    } else {
                        map_claude_error(error)
                    });
                }
                self.apply_message(value, &mut events);
            }
            // The CLI adds line types and fields between releases. The stream is
            // a display of a turn the CLI runs and answers by itself, so a line
            // or field the adapter does not know is skipped, never a reason to
            // throw the turn away.
            _ => {}
        }
        Ok(events)
    }

    fn apply_stream_event(&mut self, event: &Value, events: &mut Vec<ParsedEvent>) {
        match event.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                if event.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use")
                {
                    // A block without its name is refused by the process loop,
                    // which checks every streamed `tool_use` before this.
                    let (Some(index), Some(name)) = (
                        event.get("index").and_then(Value::as_u64),
                        event.pointer("/content_block/name").and_then(Value::as_str),
                    ) else {
                        return;
                    };
                    let call_id = event
                        .pointer("/content_block/id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    self.tool_parts
                        .insert(index, (call_id, name.to_string(), String::new()));
                }
            }
            Some("content_block_delta") => {
                let Some(delta) = event.get("delta") else {
                    return;
                };
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            events.push(ParsedEvent::Text(text.to_string()));
                        }
                    }
                    // With `thinking.display: "updates"` a thinking delta may carry
                    // only a token estimate and no text.
                    Some("thinking_delta") => {
                        if let Some(text) = delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .filter(|text| !text.is_empty())
                        {
                            events.push(ParsedEvent::Reasoning(text.to_string()));
                        }
                    }
                    Some("input_json_delta") => {
                        // Only client `tool_use` blocks are tracked; a server
                        // tool's (`server_tool_use`, e.g. `advisor`) input runs
                        // on the API and is no observation.
                        let (Some(index), Some(partial)) = (
                            event.get("index").and_then(Value::as_u64),
                            delta.get("partial_json").and_then(Value::as_str),
                        ) else {
                            return;
                        };
                        let Some((_, _, arguments)) = self.tool_parts.get_mut(&index) else {
                            return;
                        };
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
                            // Arguments the CLI streamed in a shape that does not
                            // parse are still what the model called the tool with.
                            serde_json::from_str(&arguments)
                                .unwrap_or(Value::String(arguments))
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
            _ => {}
        }
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
        for unopened in ["Bash", "Task", "mcp__other__read"] {
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

    /// The CLI offers the API's server-side `advisor` tool on every request. Its
    /// call streams as a `server_tool_use` block whose input arrives as
    /// `input_json_delta`, like a client tool's; refusing that delta ended the
    /// whole turn with `provider_protocol_changed` whenever the model consulted it.
    #[test]
    fn a_server_tool_call_streams_through_without_an_observation() {
        let mut parser = parse_init(&["mcp__eud-tools__read_file"]);
        let mut stream = |event: serde_json::Value| {
            parser
                .apply(&json!({"type": "stream_event", "event": event}))
                .expect("a server tool block parses")
        };
        stream(json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {"type": "server_tool_use", "id": "srvtoolu_1", "name": "advisor", "input": {}},
        }));
        let delta = stream(json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "{}"},
        }));
        let stop = stream(json!({"type": "content_block_stop", "index": 1}));
        assert!(delta.is_empty() && stop.is_empty());
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

    /// What the CLI printed when another Claude process held (or died holding) the OAuth
    /// refresh lock: the error is named `server_error`, which read as "provider connection
    /// closed" and hid that the login token could not be refreshed.
    #[test]
    fn a_failed_oauth_refresh_is_named_as_such() {
        let mut parser = parse_init(&["mcp__eud-tools__read_file"]);
        parser
            .apply(&json!({
                "type": "assistant",
                "error": "server_error",
                "message": {"model": "<synthetic>", "role": "assistant", "content": [
                    {"type": "text", "text": "Failed to refresh OAuth token: another Claude Code process is refreshing it or exited mid-refresh. This is usually transient; retry in a minute."}
                ]},
                "session_id": "native-session",
            }))
            .expect("an assistant line parses");
        assert_eq!(
            parser.error_code.as_deref(),
            Some("provider_auth_refresh_failed")
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

    fn parse_result() -> ClaudeStreamParser {
        let mut parser = parse_init(&["mcp__eud-tools__read_file"]);
        parser
            .apply(&json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "session_id": "native-session",
                "result": "done",
            }))
            .expect("a result line parses");
        parser
    }

    /// The CLI can keep its process alive after `result` (it waits up to 60 seconds for
    /// its end-of-turn classifier) and print metadata meanwhile. Rejecting those lines
    /// threw away a completed turn as "provider_protocol_changed".
    #[test]
    fn metadata_after_the_result_does_not_end_a_completed_run() {
        let mut parser = parse_result();
        for line in [
            json!({"type": "keep_alive"}),
            json!({"type": "system", "subtype": "post_turn_summary", "status_category": "done"}),
            json!({"type": "system", "subtype": "session_state_changed", "state": "idle"}),
            json!({"type": "rate_limit_event"}),
        ] {
            assert!(parser.apply(&line).expect("metadata parses").is_empty());
        }
        assert_eq!(parser.result.as_deref(), Some("done"));
        assert!(!parser.is_error);
    }

    /// The end-of-turn classifier the CLI runs after `result` can print its own
    /// model lines. Treating them as a protocol change threw away turns the
    /// CLI had already answered.
    #[test]
    fn output_after_the_result_never_ends_a_completed_run() {
        for line in [
            json!({"type": "result", "subtype": "success", "is_error": false, "result": "again"}),
            json!({"type": "stream_event", "event": {"type": "message_start"}}),
            json!({"type": "assistant", "message": {"content": []}}),
            json!({"type": "user", "message": {"content": []}}),
            json!({"subtype": "untyped"}),
        ] {
            let mut parser = parse_result();
            assert!(parser.apply(&line).expect("ignored").is_empty(), "{line}");
            assert_eq!(parser.result.as_deref(), Some("done"));
        }
    }

    /// `thinking.display: "updates"` streams thinking deltas that carry only a
    /// token estimate; a delta without its text ended long-thinking turns.
    #[test]
    fn lines_the_adapter_does_not_know_are_skipped() {
        let mut parser = parse_init(&["mcp__eud-tools__read_file"]);
        for line in [
            json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0,
                "delta": {"type": "thinking_delta", "estimated_tokens": 120}}}),
            json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "", "estimated_tokens": null}}}),
            json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 1,
                "delta": {"type": "text_delta"}}}),
            json!({"type": "stream_event", "event": {"type": "content_block_delta"}}),
            json!({"type": "stream_event", "event": {"type": "thinking_update", "index": 0}}),
            json!({"type": "stream_event"}),
            json!({"type": "system", "subtype": "thinking_tokens", "estimated_tokens": 50}),
            json!({"type": "api-request-blob"}),
            json!({"subtype": "untyped"}),
        ] {
            assert!(parser.apply(&line).expect("skipped").is_empty(), "{line}");
        }
    }

    /// A plugin the app profile synced that failed to load is reported on the
    /// init line; it does not stop the CLI, so it does not stop the run.
    #[test]
    fn a_plugin_error_on_init_does_not_end_the_run() {
        let mut parser = ClaudeStreamParser::default();
        parser
            .apply(&json!({
                "type": "system",
                "subtype": "init",
                "session_id": "native-session",
                "tools": ["mcp__eud-tools__read_file"],
                "mcp_servers": [{"name": "eud-tools", "status": "connected"}],
                "plugin_errors": [{"plugin": "synced", "error": "failed to load"}],
            }))
            .expect("an init line parses");
        parser.validate_init(true).expect("eud-tools is connected");
    }

    /// A response the CLI did not stream (its fallback after a stalled stream)
    /// arrives only as the assistant message. Its tool result had no known
    /// call and ended the turn; the call is now taken from the message.
    #[test]
    fn a_tool_call_the_stream_never_showed_is_taken_from_the_message() {
        let mut parser = parse_init(&["mcp__eud-tools__read_file", "Edit"]);
        let started = parser
            .apply(&json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "toolu_1", "name": "Edit",
                 "input": {"file_path": "src/main.eps", "old_string": "a", "new_string": "b"}},
            ]}}))
            .expect("an assistant line parses");
        assert!(matches!(
            started.as_slice(),
            [super::ParsedEvent::ToolObservation { name, arguments: Some(arguments), status: Some(status), .. }]
                if name == "Edit" && status == "started" && arguments["file_path"] == "src/main.eps"
        ));
        let finished = parser
            .apply(&json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"},
            ]}}))
            .expect("a tool result parses");
        assert!(matches!(
            finished.as_slice(),
            [super::ParsedEvent::ToolObservation { name, status: Some(status), .. }]
                if name == "Edit" && status == "completed"
        ));
        let unknown = parser
            .apply(&json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "toolu_unknown"},
            ]}}))
            .expect("a result for an unknown call is skipped");
        assert!(unknown.is_empty());
    }

    /// A streamed call is announced once, not again from its assistant message.
    #[test]
    fn a_streamed_tool_call_is_not_announced_twice() {
        let mut parser = parse_init(&["mcp__eud-tools__read_file"]);
        let mut events = Vec::new();
        for line in [
            json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 0,
                "content_block": {"type": "tool_use", "id": "toolu_1", "name": "mcp__eud-tools__read_file", "input": {}}}}),
            json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{\"path\":\"a\"}"}}}),
            json!({"type": "stream_event", "event": {"type": "content_block_stop", "index": 0}}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "toolu_1", "name": "mcp__eud-tools__read_file", "input": {"path": "a"}},
            ]}}),
        ] {
            events.extend(parser.apply(&line).expect("parses"));
        }
        assert_eq!(events.len(), 1);
    }
}
