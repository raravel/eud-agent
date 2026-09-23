use super::*;
use std::sync::Arc;
use std::time::Duration;

use crate::provider::ModelCapabilities;
use crate::provider_runtime::{
    AdapterOutput, AgentTurnInput, BindingSnapshot, ConversationItem, RunId, RunPolicy,
    StructuredJobKind, WorkspaceAccess,
};

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("eud-claude-adapter-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn adapter(root: &Path, script: &Path) -> ProductionClaudeCodeAdapter {
    ProductionClaudeCodeAdapter::new(
        CLAUDE_PROVIDER_DEFAULT.to_string(),
        which::which("powershell.exe").unwrap(),
        root.join("profile"),
    )
    .unwrap()
    .with_prefix_args(vec![
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        script.display().to_string(),
    ])
}

fn identity(run_id: u64, generation: u64) -> RunIdentity {
    RunIdentity {
        session_id: "app-session".to_string(),
        run_id: RunId::new(run_id),
        request_id: format!("request-{run_id}"),
        session_kind: crate::session::SessionKind::Eps,
        cancellation_generation: generation,
    }
}

fn binding(session_id: Option<&str>) -> BindingSnapshot {
    BindingSnapshot {
        provider: ProviderId::ClaudeCode,
        model: CLAUDE_PROVIDER_DEFAULT.to_string(),
        reasoning: None,
        base_url: None,
        capabilities: Some(ModelCapabilities {
            vision: true,
            tool_calls: true,
            strict_structured_output: true,
            reasoning_levels: Vec::new(),
            native_compaction: true,
            context_window: None,
            hosted_web_search: false,
        }),
        conversation: ProviderConversationState::ClaudeCode {
            session_id: session_id.map(str::to_string),
        },
    }
}

fn policy() -> RunPolicy {
    RunPolicy {
        active_deadline: Some(Duration::from_secs(10)),
        shutdown_grace: Duration::from_secs(2),
        max_output_bytes: MAX_STDOUT_BYTES,
        max_output_tokens: None,
        max_tool_rounds: 64,
        allow_resume: true,
    }
}

fn foreground_request(
    root: &Path,
    run_id: u64,
    generation: u64,
    session_id: Option<&str>,
    cancellation: tokio::sync::watch::Receiver<u64>,
) -> AdapterStepRequest {
    let mut turn = AgentTurnInput::text("hello").with_access(WorkspaceAccess::Read);
    turn.workspace_root = Some(root.to_path_buf());
    AdapterStepRequest {
        identity: identity(run_id, generation),
        binding: binding(session_id),
        kind: AdapterRequestKind::Foreground(turn),
        policy: policy(),
        continuation: None,
        history: Arc::<[ConversationItem]>::from([]),
        prior_tool_results: Arc::from([]),
        tool_descriptors: Arc::from([]),
        native_mcp_endpoint: Some("http://127.0.0.1:43123/mcp/run-token".to_string()),
        cancellation,
    }
}

fn structured_request(
    root: &Path,
    run_id: u64,
    output_schema: Value,
    cancellation: tokio::sync::watch::Receiver<u64>,
) -> AdapterStepRequest {
    AdapterStepRequest {
        identity: identity(run_id, 0),
        binding: binding(Some("foreground-session")),
        kind: AdapterRequestKind::Structured {
            kind: StructuredJobKind::TaskStateCompiler,
            prompt: "compile".to_string(),
            workspace_root: root.to_path_buf(),
            output_schema,
        },
        policy: policy(),
        continuation: None,
        history: Arc::from([]),
        prior_tool_results: Arc::from([]),
        tool_descriptors: Arc::from([]),
        native_mcp_endpoint: None,
        cancellation,
    }
}

fn write_script(root: &Path, body: &str) -> PathBuf {
    let script = root.join("fake-claude.ps1");
    std::fs::write(&script, body).unwrap();
    script
}

async fn assert_cli_process_stopped(pid: u32) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let output = tokio::process::Command::new("tasklist.exe")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                .output()
                .await
                .unwrap();
            if !String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropped Claude process tree must terminate");
}

#[tokio::test]
async fn production_process_preserves_stream_order_native_tools_and_resume() {
    let fixture = FixtureDir::new();
    let args_file = fixture.0.join("args.json");
    let escaped_args = args_file.display().to_string().replace('\'', "''");
    let script = write_script(
        &fixture.0,
        &format!(
            r#"param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Rest)
[IO.File]::WriteAllText('{escaped_args}', ($Rest | ConvertTo-Json -Compress))
$null = [Console]::In.ReadLine()
if ($Rest -contains '--mcp-config') {{
  [Console]::Out.WriteLine('{{"type":"system","subtype":"init","session_id":"native-session","tools":["mcp__eud-tools__read_file"],"mcp_servers":[{{"name":"eud-tools","status":"connected"}}]}}')
}} else {{
  [Console]::Out.WriteLine('{{"type":"system","subtype":"init","session_id":"native-session","tools":[],"mcp_servers":[]}}')
}}
[Console]::Out.WriteLine('{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"thinking_delta","thinking":"reason"}}}}}}')
if ($Rest -contains '--mcp-config') {{
  [Console]::Out.WriteLine('{{"type":"stream_event","event":{{"type":"content_block_start","index":1,"content_block":{{"type":"tool_use","id":"call-1","name":"mcp__eud-tools__read_file"}}}}}}')
  [Console]::Out.WriteLine('{{"type":"stream_event","event":{{"type":"content_block_delta","index":1,"delta":{{"type":"input_json_delta","partial_json":"{{}}"}}}}}}')
  [Console]::Out.WriteLine('{{"type":"stream_event","event":{{"type":"content_block_stop","index":1}}}}')
  [Console]::Out.WriteLine('{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"call-1","content":"ok"}}]}}}}')
}}
[Console]::Out.WriteLine('{{"type":"stream_event","event":{{"type":"content_block_delta","index":2,"delta":{{"type":"text_delta","text":"answer"}}}}}}')
[Console]::Out.WriteLine('{{"type":"result","subtype":"success","session_id":"native-session","is_error":false,"result":"answer","usage":{{"input_tokens":13,"cache_read_input_tokens":4,"cache_creation_input_tokens":6,"output_tokens":7}}}}')
exit 0
"#
        ),
    );
    let mut adapter = adapter(&fixture.0, &script);
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(16);
    let outcome = adapter
        .run_step(
            foreground_request(&fixture.0, 7, 0, Some("native-session"), cancel_rx),
            events_tx,
        )
        .await
        .unwrap();

    assert!(matches!(outcome, AdapterStepOutcome::Completed {
        output: AdapterOutput::Text(ref text),
        native_conversation: Some(ProviderConversationState::ClaudeCode { session_id: Some(ref id) }),
        ..
    } if text == "answer" && id == "native-session"));
    let mut kinds = Vec::new();
    while let Ok(event) = events_rx.try_recv() {
        kinds.push(event.kind);
    }
    assert!(matches!(kinds.as_slice(), [
        AdapterEventKind::ResponseStarted { .. },
        AdapterEventKind::Block(NormalizedBlock::Reasoning { text, .. }),
        AdapterEventKind::NativeToolObservation { mcp_server: Some(server), name, call_id: Some(completed_id), arguments: Some(arguments), status: Some(completed), .. },
        AdapterEventKind::NativeToolObservation { mcp_server: Some(result_server), call_id: Some(result_id), result: Some(result), status: Some(result_status), .. },
        AdapterEventKind::Block(NormalizedBlock::Text { text: answer, .. }),
        AdapterEventKind::Usage(_),
        AdapterEventKind::ResponseFinished { complete: true, .. },
    ] if text == "reason" && name == "mcp__eud-tools__read_file" && server == "eud-tools" && result_server == "eud-tools" && completed_id == "call-1" && arguments == &json!({}) && completed == "started" && result_id == "call-1" && result == "ok" && result_status == "completed" && answer == "answer"));
    let context = kinds
        .iter()
        .find_map(|kind| match kind {
            AdapterEventKind::Usage(usage) => usage.context_usage.as_ref(),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        context.last,
        crate::ipc::TokenUsageBreakdown {
            input_tokens: 13,
            cached_input_tokens: 4,
            cache_write_input_tokens: 6,
            output_tokens: 7,
            reasoning_output_tokens: 0,
            total_tokens: 20
        }
    );
    assert_eq!(context.total, context.last);
    assert_eq!(context.model_context_window, None);
    let args: Value = serde_json::from_slice(&std::fs::read(&args_file).unwrap()).unwrap();
    let args = args.as_array().unwrap();
    assert!(args.iter().any(|value| value == "--resume"));
    assert!(args.iter().any(|value| value == "native-session"));
    assert!(args.iter().any(|value| value == "--strict-mcp-config"));
    assert!(args.iter().any(|value| value
        .as_str()
        .is_some_and(|arg| arg.contains("/mcp/run-token"))));
    // An interactive turn edits the project tree with the built-in file tools.
    let tools_index = args.iter().position(|value| value == "--tools").unwrap();
    let tools = args.get(tools_index + 1).and_then(Value::as_str).unwrap();
    for tool in ["Read", "Edit", "Write", "Glob", "Grep"] {
        assert!(tools.contains(tool), "{tool} must be available: {tools}");
    }
    // `build_run` stays the only way to run anything.
    assert!(!tools.contains("Bash"), "{tools}");
    let allowed_index = args
        .iter()
        .position(|value| value == "--allowedTools")
        .unwrap();
    let allowed = args.get(allowed_index + 1).and_then(Value::as_str).unwrap();
    assert!(allowed.contains("mcp__eud-tools__*"), "{allowed}");
    assert!(allowed.contains("Edit"), "{allowed}");
    // The map, the verified references and the history itself are never written.
    let denied_index = args
        .iter()
        .position(|value| value == "--disallowedTools")
        .unwrap();
    let denied = args.get(denied_index + 1).and_then(Value::as_str).unwrap();
    for rule in [
        "Edit(maps/**)",
        "Edit(references/**)",
        "Edit(.git/**)",
        "Edit(.claude/**)",
        "Edit(.mcp.json)",
    ] {
        assert!(denied.contains(rule), "{rule} must be denied: {denied}");
    }

    let (_compact_cancel_tx, compact_cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (compact_events_tx, mut compact_events_rx) = tokio::sync::mpsc::channel(16);
    let state = adapter
        .compact(identity(16, 0), compact_cancel_rx, compact_events_tx)
        .await
        .unwrap();
    assert_eq!(
        state,
        ProviderConversationState::ClaudeCode {
            session_id: Some("native-session".to_string())
        }
    );
    let args: Value = serde_json::from_slice(&std::fs::read(args_file).unwrap()).unwrap();
    let args = args.as_array().unwrap();
    assert!(args.iter().any(|value| value == "--resume"));
    assert!(args.iter().any(|value| value == "native-session"));
    assert!(!args.iter().any(|value| value == "--mcp-config"));
    let tools_index = args.iter().position(|value| value == "--tools").unwrap();
    assert_eq!(args.get(tools_index + 1).and_then(Value::as_str), Some(""));
    assert!(matches!(
        compact_events_rx.recv().await.unwrap().kind,
        AdapterEventKind::ResponseStarted { .. }
    ));
    assert!(matches!(
        compact_events_rx.recv().await.unwrap().kind,
        AdapterEventKind::Block(NormalizedBlock::Reasoning { .. })
    ));
    assert!(matches!(
        compact_events_rx.recv().await.unwrap().kind,
        AdapterEventKind::Block(NormalizedBlock::Text { .. })
    ));
    assert!(matches!(
        compact_events_rx.recv().await.unwrap().kind,
        AdapterEventKind::Usage(_)
    ));
    assert!(matches!(
        compact_events_rx.recv().await.unwrap().kind,
        AdapterEventKind::ResponseFinished { complete: true, .. }
    ));
}

#[tokio::test]
async fn partial_nonzero_exit_poisoned_native_continuation() {
    let fixture = FixtureDir::new();
    let script = write_script(
        &fixture.0,
        r#"param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Rest)
$null = [Console]::In.ReadLine()
[Console]::Out.WriteLine('{"type":"system","subtype":"init","session_id":"changed-session","tools":["mcp__eud-tools__read_file"],"mcp_servers":[{"name":"eud-tools","status":"connected"}]}')
[Console]::Out.WriteLine('{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}}')
exit 9
"#,
    );
    let mut adapter = adapter(&fixture.0, &script);
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(8);
    let error = adapter
        .run_step(
            foreground_request(&fixture.0, 8, 0, Some("previous-session"), cancel_rx),
            events_tx,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderRuntimeError::Transport(_)));
    assert!(matches!(
        events_rx.recv().await.unwrap().kind,
        AdapterEventKind::ResponseStarted { .. }
    ));
    assert!(
        matches!(events_rx.recv().await.unwrap().kind, AdapterEventKind::Block(NormalizedBlock::Text { text, .. }) if text == "partial")
    );
    assert!(events_rx.try_recv().is_err());

    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (events_tx, _) = tokio::sync::mpsc::channel(1);
    let error = adapter
        .run_step(
            foreground_request(&fixture.0, 9, 0, Some("previous-session"), cancel_rx),
            events_tx,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProviderRuntimeError::Protocol(ref detail) if detail.contains("continuation is unknown"))
    );
}

#[test]
fn native_tool_result_preserves_execution_failure_without_duplicate_start() {
    let mut parser = ClaudeStreamParser::default();
    let start = parser.apply(&json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call-1","name":"mcp__eud-tools__read_file"}}})).unwrap();
    assert!(start.is_empty());
    let call = parser
        .apply(&json!({"type":"stream_event","event":{"type":"content_block_stop","index":0}}))
        .unwrap();
    assert!(
        matches!(call.as_slice(), [ParsedEvent::ToolObservation { arguments: Some(_), status: Some(status), .. }] if status == "started")
    );
    let result = parser.apply(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call-1","content":"file unavailable","is_error":true}]}})).unwrap();
    assert!(
        matches!(result.as_slice(), [ParsedEvent::ToolObservation { result: Some(value), status: Some(status), .. }] if status == "failed" && value == "file unavailable")
    );
}

fn started_native_tool(parser: &mut ClaudeStreamParser, call_id: &str, name: &str) {
    parser
        .apply(&json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":call_id,"name":name}}}))
        .unwrap();
    parser
        .apply(&json!({"type":"stream_event","event":{"type":"content_block_stop","index":0}}))
        .unwrap();
}

#[test]
fn native_tool_result_image_blocks_are_exempt_from_observation_limit() {
    let mut parser = ClaudeStreamParser::default();
    started_native_tool(&mut parser, "call-1", "mcp__eud-tools__map_draft_render");
    let oversized_png = "A".repeat(events::MAX_NATIVE_OBSERVATION_BYTES * 3);
    let result = parser
        .apply(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call-1","content":[
            {"type":"text","text":"{\"image\":{\"mimeType\":\"image/png\",\"width\":704,\"height\":448}}"},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":oversized_png}}
        ]}]}}))
        .unwrap();
    assert!(
        matches!(result.as_slice(), [ParsedEvent::ToolObservation { name, result: Some(value), status: Some(status), .. }]
            if status == "completed" && name == "mcp__eud-tools__map_draft_render" && value[1]["type"] == "image")
    );
}

#[test]
fn native_tool_result_text_over_observation_limit_is_still_rejected() {
    let mut parser = ClaudeStreamParser::default();
    started_native_tool(&mut parser, "call-1", "mcp__eud-tools__read_file");
    let oversized_text = "x".repeat(events::MAX_NATIVE_OBSERVATION_BYTES + 1);
    let result = parser.apply(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call-1","content":[
        {"type":"text","text":oversized_text},
        {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}
    ]}]}}));
    assert!(matches!(result, Err(ProviderRuntimeError::Protocol(_))));
}

#[tokio::test]
async fn native_image_tool_result_echo_over_one_mib_completes() {
    let fixture = FixtureDir::new();
    // Claude Code echoes the whole tool_result on one stream-json line, so a
    // rendered map image is bounded only by the raw stdout ceiling.
    let image_bytes = 2 * 1024 * 1024;
    let script = write_script(
        &fixture.0,
        &format!(
            r#"param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Rest)
$null = [Console]::In.ReadLine()
[Console]::Out.WriteLine('{{"type":"system","subtype":"init","session_id":"native-session","tools":["mcp__eud-tools__map_draft_render"],"mcp_servers":[{{"name":"eud-tools","status":"connected"}}]}}')
[Console]::Out.WriteLine('{{"type":"stream_event","event":{{"type":"content_block_start","index":0,"content_block":{{"type":"tool_use","id":"call-1","name":"mcp__eud-tools__map_draft_render"}}}}}}')
[Console]::Out.WriteLine('{{"type":"stream_event","event":{{"type":"content_block_stop","index":0}}}}')
$image = 'A' * {image_bytes}
[Console]::Out.WriteLine('{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"call-1","content":[{{"type":"text","text":"{{\"image\":{{\"mimeType\":\"image/png\"}}}}"}},{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"' + $image + '"}}}}]}}]}}}}')
[Console]::Out.WriteLine('{{"type":"stream_event","event":{{"type":"content_block_delta","index":1,"delta":{{"type":"text_delta","text":"looks right"}}}}}}')
[Console]::Out.WriteLine('{{"type":"result","subtype":"success","session_id":"native-session","is_error":false,"result":"looks right","usage":{{"input_tokens":1,"output_tokens":1}}}}')
exit 0
"#
        ),
    );
    let mut adapter = adapter(&fixture.0, &script);
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(16);
    let outcome = adapter
        .run_step(
            foreground_request(&fixture.0, 12, 0, Some("native-session"), cancel_rx),
            events_tx,
        )
        .await
        .unwrap();
    assert!(matches!(outcome, AdapterStepOutcome::Completed {
        output: AdapterOutput::Text(ref text),
        ..
    } if text == "looks right"));
    let mut observed_image = false;
    while let Ok(event) = events_rx.try_recv() {
        if let AdapterEventKind::NativeToolObservation {
            result: Some(result),
            status: Some(status),
            ..
        } = event.kind
        {
            assert_eq!(status, "completed");
            assert_eq!(result[1]["type"], "image");
            assert_eq!(
                result[1]["source"]["data"].as_str().unwrap().len(),
                image_bytes
            );
            observed_image = true;
        }
    }
    assert!(observed_image);
}

#[tokio::test]
async fn malformed_and_oversized_process_output_are_rejected() {
    let fixture = FixtureDir::new();
    let script = write_script(
        &fixture.0,
        r#"param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Rest)
$null = [Console]::In.ReadLine()
[Console]::Out.WriteLine('not-json')
"#,
    );
    let mut malformed_adapter = adapter(&fixture.0, &script);
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (events_tx, _) = tokio::sync::mpsc::channel(1);
    let error = malformed_adapter
        .run_step(
            foreground_request(&fixture.0, 10, 0, None, cancel_rx),
            events_tx,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderRuntimeError::Protocol(_)));

    // A valid init line followed by one line just over the raw stdout ceiling.
    let oversized_bytes = MAX_STDOUT_BYTES + 1;
    write_script(
        &fixture.0,
        &format!(
            r#"param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Rest)
$null = [Console]::In.ReadLine()
[Console]::Out.WriteLine('{{"type":"system","subtype":"init","session_id":"native-session","tools":["mcp__eud-tools__read_file"],"mcp_servers":[{{"name":"eud-tools","status":"connected"}}]}}')
[Console]::Out.Write(('x' * {oversized_bytes}))
"#
        ),
    );
    let mut oversized_adapter = adapter(&fixture.0, &script);
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(16);
    let error = oversized_adapter
        .run_step(
            foreground_request(&fixture.0, 11, 0, None, cancel_rx),
            events_tx,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderRuntimeError::Protocol(_)));
}

#[tokio::test]
async fn structured_process_isolated_and_schema_strict() {
    let fixture = FixtureDir::new();
    let args_file = fixture.0.join("structured-args.json");
    let escaped_args = args_file.display().to_string().replace('\'', "''");
    let script = write_script(
        &fixture.0,
        &format!(
            r#"param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Rest)
[IO.File]::WriteAllText('{escaped_args}', ($Rest | ConvertTo-Json -Compress))
[Console]::Out.Write('{{"type":"result","subtype":"success","is_error":false,"structured_output":{{"ok":true}}}}')
exit 0
"#
        ),
    );
    let mut adapter = adapter(&fixture.0, &script);
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let schema = json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false});
    let request = AdapterStepRequest {
        identity: identity(10, 0),
        binding: binding(Some("foreground-session")),
        kind: AdapterRequestKind::Structured {
            kind: StructuredJobKind::TaskStateCompiler,
            prompt: "compile".to_string(),
            workspace_root: fixture.0.clone(),
            output_schema: schema,
        },
        policy: policy(),
        continuation: None,
        history: Arc::from([]),
        prior_tool_results: Arc::from([]),
        tool_descriptors: Arc::from([]),
        native_mcp_endpoint: None,
        cancellation: cancel_rx,
    };
    let mut forbidden = request.clone();
    forbidden.native_mcp_endpoint = Some("http://127.0.0.1:1/mcp/stale".to_string());
    let (forbidden_events, _) = tokio::sync::mpsc::channel(1);
    let error = adapter
        .run_step(forbidden, forbidden_events)
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProviderRuntimeError::Protocol(ref detail) if detail.contains("tool authority"))
    );
    assert!(!args_file.exists());
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(4);
    let outcome = adapter.run_step(request, events_tx).await.unwrap();
    assert!(matches!(outcome, AdapterStepOutcome::Completed {
        output: AdapterOutput::Structured(ref value),
        native_conversation: None,
        ..
    } if value == &json!({"ok":true})));
    assert!(matches!(
        events_rx.recv().await.unwrap().kind,
        AdapterEventKind::ResponseStarted { .. }
    ));
    assert!(matches!(
        events_rx.recv().await.unwrap().kind,
        AdapterEventKind::Block(NormalizedBlock::Text { .. })
    ));
    assert!(matches!(
        events_rx.recv().await.unwrap().kind,
        AdapterEventKind::ResponseFinished { complete: true, .. }
    ));
    let args: Value = serde_json::from_slice(&std::fs::read(args_file).unwrap()).unwrap();
    let args = args.as_array().unwrap();
    assert!(args.iter().any(|value| value == "--no-session-persistence"));
    assert!(args.iter().any(|value| value == "--strict-mcp-config"));
    let tools_index = args.iter().position(|value| value == "--tools").unwrap();
    assert_eq!(args.get(tools_index + 1).and_then(Value::as_str), Some(""));
    assert!(!args.iter().any(|value| value == "--resume"));
    assert!(!args.iter().any(|value| value == "--mcp-config"));

    std::fs::write(
        &script,
        r#"param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Rest)
[Console]::Out.Write('{"type":"result","subtype":"success","is_error":false,"structured_output":{"ok":"not-a-boolean"}}')
exit 0
"#,
    )
    .unwrap();
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let invalid = AdapterStepRequest {
        identity: identity(12, 0),
        binding: binding(Some("foreground-session")),
        kind: AdapterRequestKind::Structured {
            kind: StructuredJobKind::TaskStateCompiler,
            prompt: "compile".to_string(),
            workspace_root: fixture.0.clone(),
            output_schema: json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}),
        },
        policy: policy(),
        continuation: None,
        history: Arc::from([]),
        prior_tool_results: Arc::from([]),
        tool_descriptors: Arc::from([]),
        native_mcp_endpoint: None,
        cancellation: cancel_rx,
    };
    let (events_tx, _) = tokio::sync::mpsc::channel(1);
    let error = adapter.run_step(invalid, events_tx).await.unwrap_err();
    assert_eq!(error, ProviderRuntimeError::StructuredOutputInvalid);

    std::fs::write(
        &script,
        r#"param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Rest)
[Console]::Out.Write('{"type":"result","subtype":"error_during_execution","is_error":true,"structured_output":{"ok":true}}')
exit 0
"#,
    )
    .unwrap();
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let error_envelope = structured_request(
        &fixture.0,
        13,
        json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}),
        cancel_rx,
    );
    let (events_tx, _) = tokio::sync::mpsc::channel(1);
    let error = adapter
        .run_step(error_envelope, events_tx)
        .await
        .unwrap_err();
    assert_eq!(error, ProviderRuntimeError::StructuredOutputInvalid);
}

#[tokio::test]
async fn cancellation_terminates_hung_cli_and_rejects_late_events() {
    let fixture = FixtureDir::new();
    let child_pid_file = fixture.0.join("child-pid");
    let escaped_child_pid = child_pid_file.display().to_string().replace('\'', "''");
    let script = write_script(
        &fixture.0,
        &format!(
            r#"param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Rest)
$null = [Console]::In.ReadLine()
[Console]::Out.WriteLine('{{"type":"system","subtype":"init","session_id":"changed-session","tools":["mcp__eud-tools__read_file"],"mcp_servers":[{{"name":"eud-tools","status":"connected"}}]}}')
$child = Start-Process powershell.exe -WindowStyle Hidden -ArgumentList @('-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 60') -PassThru
$marker = '{escaped_child_pid}.tmp'
[IO.File]::WriteAllText($marker, $child.Id.ToString())
[IO.File]::Move($marker, '{escaped_child_pid}')
Start-Sleep -Seconds 60
[Console]::Out.WriteLine('{{"type":"result","subtype":"success","session_id":"changed-session","is_error":false,"result":"late"}}')
"#
        ),
    );
    let mut adapter = adapter(&fixture.0, &script);
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(8);
    let request = foreground_request(&fixture.0, 11, 0, Some("previous-session"), cancel_rx);
    let run = tokio::spawn(async move { adapter.run_step(request, events_tx).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !child_pid_file.is_file() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let child_pid = std::fs::read_to_string(&child_pid_file).unwrap();
    let alive_query = format!(
        "if (Get-Process -Id {} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}",
        child_pid.trim()
    );
    let alive = tokio::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &alive_query])
        .status()
        .await
        .unwrap();
    assert!(alive.success());
    cancel_tx.send(1).unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(outcome, AdapterStepOutcome::Cancelled);
    assert!(matches!(
        events_rx.recv().await.unwrap().kind,
        AdapterEventKind::ResponseStarted { .. }
    ));
    assert!(events_rx.try_recv().is_err());
    let process_query = format!(
        "if (Get-Process -Id {} -ErrorAction SilentlyContinue) {{ exit 1 }} else {{ exit 0 }}",
        child_pid.trim()
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let process_check = tokio::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", &process_query])
                .status()
                .await
                .unwrap();
            if process_check.success() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn dropping_inflight_future_poisoned_native_continuation() {
    let fixture = FixtureDir::new();
    let ready_file = fixture.0.join("drop-ready");
    let escaped_ready = ready_file.display().to_string().replace('\'', "''");
    let script = write_script(
        &fixture.0,
        &format!(
            r#"param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Rest)
$null = [Console]::In.ReadLine()
[Console]::Out.WriteLine('{{"type":"system","subtype":"init","session_id":"possibly-changed","tools":["mcp__eud-tools__read_file"],"mcp_servers":[{{"name":"eud-tools","status":"connected"}}]}}')
$child = Start-Process powershell.exe -WindowStyle Hidden -ArgumentList @('-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 60') -PassThru
[IO.File]::WriteAllText('{escaped_ready}', (@($PID, $child.Id) | ConvertTo-Json -Compress))
Start-Sleep -Seconds 60
"#
        ),
    );
    let mut adapter = adapter(&fixture.0, &script);
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(8);
    let request = foreground_request(&fixture.0, 14, 0, Some("previous-session"), cancel_rx);
    let mut run = Box::pin(adapter.run_step(request, events_tx));
    let processes = tokio::select! {
        result = &mut run => panic!("fake CLI ended before drop barrier: {result:?}"),
        result = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(processes) = std::fs::read(&ready_file).ok()
                    .and_then(|bytes| serde_json::from_slice::<[u32; 2]>(&bytes).ok()) {
                    break processes;
                }
                tokio::task::yield_now().await;
            }
        }) => result.unwrap(),
    };
    drop(run);
    for pid in processes {
        assert_cli_process_stopped(pid).await;
    }

    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (events_tx, _) = tokio::sync::mpsc::channel(1);
    let error = adapter
        .run_step(
            foreground_request(&fixture.0, 15, 0, Some("previous-session"), cancel_rx),
            events_tx,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProviderRuntimeError::Protocol(ref detail) if detail.contains("continuation is unknown"))
    );
}
