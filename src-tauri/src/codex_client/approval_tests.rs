use super::approvals::{record_file_change, FileChangePaths};
use super::handle_server_request;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};

#[test]
fn read_only_workers_can_request_mcp_approval_on_fresh_and_resumed_threads() {
    let endpoint = Some("http://127.0.0.1:54321/mcp");
    let start = super::thread_start_params(None, endpoint, false, false).unwrap();
    let resume = super::thread_resume_params("worker", None, endpoint, false, false).unwrap();
    for params in [start, resume] {
        assert_eq!(params["approvalPolicy"], "on-request");
        assert_eq!(params["config"]["web_search"], "disabled");
    }
    let structured = super::thread_start_params(None, None, false, false).unwrap();
    assert_eq!(structured["approvalPolicy"], "never");
}

#[tokio::test]
async fn read_only_workers_approve_only_app_mcp_tool_calls() {
    for (params, action) in [
        (
            json!({"serverName": "eud-tools", "_meta": {"codex_approval_kind": "mcp_tool_call"}}),
            "accept",
        ),
        (
            json!({"serverName": "other", "_meta": {"codex_approval_kind": "mcp_tool_call"}}),
            "decline",
        ),
        (
            json!({"serverName": "eud-tools", "_meta": {"codex_approval_kind": "other"}}),
            "decline",
        ),
    ] {
        let (reader, writer) = tokio::io::duplex(4096);
        let writer = std::sync::Arc::new(tokio::sync::Mutex::new(writer));
        handle_server_request(
            &writer,
            "mcpServer/elicitation/request",
            json!(1),
            Some(&params),
            false,
            &FileChangePaths::new(),
            None,
        )
        .await
        .unwrap();
        let line = BufReader::new(reader)
            .lines()
            .next_line()
            .await
            .unwrap()
            .unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["result"]["action"], action);
    }
}

async fn reply_to(
    method: &str,
    params: &Value,
    native_tools: bool,
    file_changes: &FileChangePaths,
) -> Value {
    let (reader, writer) = tokio::io::duplex(4096);
    let writer = std::sync::Arc::new(tokio::sync::Mutex::new(writer));
    handle_server_request(
        &writer,
        method,
        json!("request"),
        Some(params),
        native_tools,
        file_changes,
        None,
    )
    .await
    .unwrap();
    let line = BufReader::new(reader)
        .lines()
        .next_line()
        .await
        .unwrap()
        .unwrap();
    serde_json::from_str(&line).unwrap()
}

/// The `item/started` of one `fileChange` item touching `paths`.
fn started_file_change(file_changes: &mut FileChangePaths, item_id: &str, paths: &[&str]) {
    let changes = paths
        .iter()
        .map(
            |path| json!({"path": path, "kind": {"type": "update", "move_path": null}, "diff": ""}),
        )
        .collect::<Vec<_>>();
    record_file_change(
        file_changes,
        "item/started",
        Some(&json!({"threadId": "t", "turnId": "u", "item": {
            "type": "fileChange", "id": item_id, "status": "inProgress", "changes": changes
        }})),
    );
}

#[tokio::test]
async fn native_file_changes_are_approved_in_both_protocols() {
    let mut file_changes = FileChangePaths::new();
    started_file_change(
        &mut file_changes,
        "item-spec",
        &["E:/project/.eud-agent/workspace/specs/game.md"],
    );
    let reply = reply_to(
        "item/fileChange/requestApproval",
        &json!({"threadId": "t", "turnId": "u", "itemId": "item-spec", "startedAtMs": 0}),
        true,
        &file_changes,
    )
    .await;
    assert_eq!(reply["id"], "request");
    assert_eq!(reply["result"]["decision"], "accept");

    let reply = reply_to(
        "applyPatchApproval",
        &json!({"conversationId": "c", "callId": "p", "fileChanges": {
            "E:/project/.eud-agent/workspace/specs/game.md": {"type": "add", "content": "x"}
        }}),
        true,
        &file_changes,
    )
    .await;
    assert_eq!(reply["result"]["decision"], "approved");
}

#[tokio::test]
async fn native_changes_to_agent_instruction_files_are_declined() {
    // Given: file changes the app-server announced, one touching only
    // ordinary sources, the others an agent-instruction file.
    let mut file_changes = FileChangePaths::new();
    started_file_change(&mut file_changes, "ordinary", &["E:/project/src/main.eps"]);
    started_file_change(
        &mut file_changes,
        "agents",
        &["E:/project/src/main.eps", "E:/project/AGENTS.md"],
    );
    started_file_change(
        &mut file_changes,
        "workflow",
        &[r"E:\project\.github\workflows\ci.yml"],
    );
    let approval =
        |item: &str| json!({"threadId": "t", "turnId": "u", "itemId": item, "startedAtMs": 0});

    // Then: only the ordinary change is accepted.
    for (item, decision) in [
        ("ordinary", "accept"),
        ("agents", "decline"),
        ("workflow", "decline"),
        // An item the app-server never announced cannot be checked.
        ("unknown", "decline"),
    ] {
        let reply = reply_to(
            "item/fileChange/requestApproval",
            &approval(item),
            true,
            &file_changes,
        )
        .await;
        assert_eq!(reply["result"]["decision"], decision, "{item}");
    }

    // A completed item is forgotten.
    record_file_change(
        &mut file_changes,
        "item/completed",
        Some(&json!({"item": {"type": "fileChange", "id": "ordinary", "changes": []}})),
    );
    let reply = reply_to(
        "item/fileChange/requestApproval",
        &approval("ordinary"),
        true,
        &file_changes,
    )
    .await;
    assert_eq!(reply["result"]["decision"], "decline");

    // The legacy protocol names its paths, a move target included.
    for (changes, decision) in [
        (
            json!({"E:/project/src/a.eps": {"type": "update", "unified_diff": ""}}),
            "approved",
        ),
        (
            json!({"E:/project/docs/CLAUDE.md": {"type": "add", "content": "x"}}),
            "denied",
        ),
        (
            json!({"E:/project/notes.md": {"type": "update", "unified_diff": "", "move_path": "E:/project/.codex/notes.md"}}),
            "denied",
        ),
        (json!({}), "denied"),
    ] {
        let reply = reply_to(
            "applyPatchApproval",
            &json!({"conversationId": "c", "callId": "p", "fileChanges": changes}),
            true,
            &file_changes,
        )
        .await;
        assert_eq!(reply["result"]["decision"], decision, "{changes}");
    }
}

#[tokio::test]
async fn structured_jobs_and_permission_escalation_remain_declined() {
    for (method, native_tools, decision) in [
        ("item/fileChange/requestApproval", false, "decline"),
        ("applyPatchApproval", false, "denied"),
        ("item/permissions/requestApproval", true, "decline"),
    ] {
        let (reader, writer) = tokio::io::duplex(4096);
        let writer = std::sync::Arc::new(tokio::sync::Mutex::new(writer));
        handle_server_request(
            &writer,
            method,
            json!("restricted"),
            None,
            native_tools,
            &FileChangePaths::new(),
            None,
        )
        .await
        .unwrap();
        let line = BufReader::new(reader)
            .lines()
            .next_line()
            .await
            .unwrap()
            .unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["result"]["decision"], decision, "{method}");
    }
}

#[tokio::test]
#[ignore = "requires isolated QA root, Codex executable, auth file and model"]
async fn live_codex_saves_spec_with_native_patch() {
    use super::{
        AgentTurnInput, AppServerEvent, CodexAppServerClient, CodexLaunchConfig,
        CodexModelSelection, WorkspaceAccess,
    };
    use std::{path::PathBuf, time::Duration};

    let root = PathBuf::from(std::env::var_os("EUD_APPROVAL_QA_ROOT").expect("QA root"));
    let project = root.join("project");
    let profile = root.join("profile");
    std::fs::create_dir_all(project.join(".eud-agent/workspace/specs")).unwrap();
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::copy(
        std::env::var_os("EUD_CODEX_LIVE_AUTH_JSON").expect("auth source"),
        profile.join("auth.json"),
    )
    .unwrap();
    std::fs::write(
        profile.join("config.toml"),
        "cli_auth_credentials_store = \"file\"\n",
    )
    .unwrap();
    crate::provider_secrets::harden_private_path(&profile).unwrap();
    crate::provider_secrets::harden_private_path(&profile.join("auth.json")).unwrap();
    let launch = CodexLaunchConfig {
        executable: PathBuf::from(
            std::env::var_os("EUD_CODEX_LIVE_EXECUTABLE").expect("executable"),
        ),
        executable_args: Vec::new(),
        profile_dir: profile,
        large_context_models: Default::default(),
    };
    let (mut client, mut events) = CodexAppServerClient::spawn_app_server(
        &project,
        &launch,
        None,
        WorkspaceAccess::Write,
        true,
        None,
    )
    .await
    .unwrap();
    client.set_model_selection(Some(CodexModelSelection {
        model: std::env::var("EUD_CODEX_LIVE_MODEL").expect("model"),
        reasoning_effort: "low".to_string(),
    }));
    let mut input = AgentTurnInput::text(format!(
        "Use apply_patch to create the spec file {} containing exactly 'Native spec write verified' followed by a newline, and {} containing exactly 'spec.md' followed by a newline. Do not use shell writes or other file tools. Then answer SPEC_SAVED. This is a disposable test project.",
        project.join(".eud-agent/workspace/specs/spec.md").display(),
        project.join(".eud-agent/workspace/specs/index.md").display(),
    )).with_access(WorkspaceAccess::Write);
    input.workspace_root = Some(project.clone());
    let scenario = async {
        let turn = client.run_turn(input);
        let observations = async {
            let mut patch_status = Vec::new();
            let mut usage_seen = false;
            while let Some(event) = events.recv().await {
                match event {
                    AppServerEvent::ToolCallCompleted { name, status, .. }
                        if name == "fileChange" =>
                    {
                        eprintln!("native patch status: {status:?}");
                        patch_status.push(status);
                    }
                    AppServerEvent::TokenUsageUpdated { token_usage, .. } => {
                        usage_seen = token_usage.total.total_tokens > 0
                    }
                    AppServerEvent::Error(error) => panic!("native error: {error}"),
                    AppServerEvent::TurnComplete => break,
                    _ => {}
                }
            }
            (patch_status, usage_seen)
        };
        let (result, observations) = tokio::join!(turn, observations);
        result.unwrap();
        observations
    };
    let (statuses, usage_seen) = tokio::time::timeout(Duration::from_secs(240), scenario)
        .await
        .unwrap();
    assert!(usage_seen, "native turn must report usage");
    assert!(
        statuses
            .iter()
            .any(|status| status.as_deref() == Some("completed")),
        "native patch must succeed: {statuses:?}"
    );
    assert_eq!(
        std::fs::read_to_string(project.join(".eud-agent/workspace/specs/spec.md")).unwrap(),
        "Native spec write verified\n"
    );
    assert_eq!(
        std::fs::read_to_string(project.join(".eud-agent/workspace/specs/index.md")).unwrap(),
        "spec.md\n"
    );
    eprintln!("SPEC_SAVED: both spec files contain the expected bytes");
}
