use std::sync::Arc;

use serde_json::json;

use crate::{
    provider::{ModelCapabilities, ProviderConversationState, ProviderId},
    provider_runtime::{
        BindingSnapshot, CompactionRequest, ProviderRuntime, RunOutcome, RuntimeExecutor,
    },
};

use super::fixtures::RuntimeFixture;

fn thread_id(state: &ProviderConversationState) -> String {
    match state {
        ProviderConversationState::Codex {
            thread_id: Some(thread_id),
        } => thread_id.clone(),
        other => panic!("expected committed Codex thread, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "requires isolated Codex executable, auth JSON, model, and network access"]
async fn live_codex_runtime_foreground_resume_structured_compact_cancel_and_restart() {
    // Given: only an explicit executable and copied auth file inside the disposable fixture root.
    let executable = std::path::PathBuf::from(
        std::env::var_os("EUD_CODEX_LIVE_EXECUTABLE").expect("set EUD_CODEX_LIVE_EXECUTABLE"),
    );
    let auth_source = std::path::PathBuf::from(
        std::env::var_os("EUD_CODEX_LIVE_AUTH_JSON").expect("set EUD_CODEX_LIVE_AUTH_JSON"),
    );
    let model = std::env::var("EUD_CODEX_LIVE_MODEL").expect("set EUD_CODEX_LIVE_MODEL");
    let fixture = RuntimeFixture::new("live-codex");
    fixture.install_live_map_from_env();
    let project_id = fixture.root.join("project").to_string_lossy().into_owned();
    std::fs::create_dir_all(fixture.dirs.codex_home_dir()).expect("create isolated Codex profile");
    let isolated_auth = fixture.dirs.codex_home_dir().join("auth.json");
    std::fs::copy(auth_source, &isolated_auth).expect("copy isolated Codex auth");
    crate::provider_secrets::harden_private_path(&fixture.dirs.codex_home_dir())
        .expect("harden isolated Codex profile");
    crate::provider_secrets::harden_private_path(&isolated_auth)
        .expect("harden isolated Codex auth");
    let mut config = fixture
        .dirs
        .load_config()
        .expect("load isolated app config");
    config.providers.codex.executable_override = Some(executable.to_string_lossy().into_owned());
    fixture
        .dirs
        .save_config(&config)
        .expect("save isolated Codex executable override");
    let launch = crate::codex_client::resolve_codex_launch_config(&fixture.dirs)
        .expect("resolve isolated Codex launch");
    let (mut inventory, _inventory_events) =
        crate::codex_client::CodexAppServerClient::spawn_app_server(
            &fixture.root,
            &launch,
            None,
            crate::provider_runtime::WorkspaceAccess::Read,
            false,
        )
        .await
        .expect("start authenticated model inventory");
    let models = tokio::time::timeout(std::time::Duration::from_secs(60), inventory.list_models())
        .await
        .expect("Codex model inventory timed out")
        .expect("read authenticated model inventory");
    assert!(
        models.iter().any(|available| available.model == model),
        "requested Codex live model is absent from authenticated model/list"
    );
    eprintln!("Codex live model verified by model/list: {model}");
    drop(inventory);
    let binding = BindingSnapshot {
        provider: ProviderId::Codex,
        model,
        reasoning: None,
        base_url: None,
        capabilities: Some(ModelCapabilities {
            tool_calls: true,
            strict_structured_output: true,
            native_compaction: true,
            ..ModelCapabilities::default()
        }),
        conversation: ProviderConversationState::Codex { thread_id: None },
    };
    let adapter = super::super::production_adapter(
        &binding.to_binding(),
        &fixture.dirs,
        fixture.root.clone(),
    )
    .expect("construct production Codex adapter");
    let events: Arc<dyn crate::provider_runtime::RuntimeEventSink> = fixture.events.clone();
    let mut runtime = ProviderRuntime::new(
        adapter,
        binding.clone(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        events,
    )
    .expect("construct Codex runtime");
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let mut first = fixture.foreground(binding.clone(), 30_001);
    first.turn.text = format!("Use the EUD MCP tools: call list_files and project_status, then read_file for src/main.eps twice. Remember this token for the next turn: {nonce}. Reply briefly after all four tools finish.");

    // When: the native adapter is driven through every common runtime lifecycle surface.
    let first_state = match runtime.run_foreground(first).await {
        RunOutcome::Completed { conversation, .. } => conversation,
        other => panic!("Codex foreground failed: {other:?}"),
    };
    let first_thread = thread_id(&first_state);
    let receipt = crate::provider_tool_loop::RunGate::new(
        fixture.identity(30_001),
        fixture.tools.clone(),
        crate::provider_runtime::WorkspaceAccess::Read,
        None,
    )
    .receipt_path()
    .expect("native receipt path");
    let receipt: serde_json::Value = serde_json::from_slice(
        &std::fs::read(receipt).expect("read native MCP completion receipt"),
    )
    .expect("decode native MCP completion receipt");
    let completions: Vec<crate::provider_tool_loop::DurableToolCompletion> =
        serde_json::from_value(receipt["completions"].clone()).expect("decode MCP completions");
    for (name, minimum) in [("list_files", 1), ("project_status", 1), ("read_file", 2)] {
        assert!(
            completions
                .iter()
                .filter(|item| item.name == name && !item.is_error)
                .count()
                >= minimum,
            "live Codex did not complete required MCP tool {name}"
        );
    }
    assert!(completions
        .iter()
        .all(|item| item.run_id == fixture.identity(30_001).run_id
            && item.request_id == fixture.request_id
            && item.call_id.is_some()));
    runtime
        .acknowledge_persisted()
        .await
        .expect("acknowledge observed native checkpoint");
    fixture
        .tools
        .begin_request("codex-live-resume", &project_id)
        .expect("begin Codex resume");
    let mut resumed_binding = binding.clone();
    resumed_binding.conversation = first_state;
    let mut resumed = fixture.foreground(resumed_binding.clone(), 30_002);
    resumed.identity.request_id = "codex-live-resume".to_string();
    resumed.binding = resumed_binding.clone();
    resumed.turn.text = "Return the token from the previous turn.".to_string();
    let resumed_state = match runtime.run_foreground(resumed).await {
        RunOutcome::Completed { text, conversation } => {
            assert!(
                text.contains(&nonce),
                "native session did not resume context"
            );
            conversation
        }
        other => panic!("Codex resume failed: {other:?}"),
    };
    assert_eq!(thread_id(&resumed_state), first_thread);
    let mut job_binding = binding.clone();
    job_binding.conversation = resumed_state.clone();
    let mut job = fixture.structured(
        job_binding.clone(),
        30_003,
        json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"],
            "additionalProperties": false
        }),
    );
    job.prompt = "Return one JSON object with value set to isolated.".to_string();
    job.policy.active_deadline = Some(crate::provider_runtime::TASK_STATE_COMPILER_DEADLINE);
    std::fs::create_dir_all(&job.workspace_root).expect("prepare isolated structured workspace");
    let structured_outcome = runtime.run_structured(job).await;
    assert!(
        matches!(
            &structured_outcome,
            RunOutcome::Structured { value, .. } if value == &json!({"value":"isolated"})
        ),
        "Codex structured job failed: {structured_outcome:?}"
    );
    assert_eq!(runtime.conversation_state(), resumed_state);
    let compacted = runtime
        .compact(CompactionRequest {
            identity: fixture.identity(30_004),
            binding: job_binding,
            workspace_root: fixture.root.clone(),
            next_instruction_epoch: 4,
            policy: super::fixtures::policy(Some(
                crate::provider_runtime::TASK_STATE_COMPILER_DEADLINE,
            )),
        })
        .await
        .expect("compact native Codex thread");
    let compacted_thread = thread_id(&compacted);
    assert_eq!(compacted_thread, first_thread);
    fixture
        .tools
        .begin_request("codex-live-cancel", &project_id)
        .expect("begin Codex cancellation");
    fixture
        .events
        .arm_cancellation(fixture.cancellation.clone());
    let mut cancel_binding = binding.clone();
    cancel_binding.conversation = compacted;
    let mut cancelled = fixture.foreground(cancel_binding.clone(), 30_005);
    cancelled.identity.request_id = "codex-live-cancel".to_string();
    cancelled.binding = cancel_binding;
    cancelled.turn.text = "Produce a long response with many numbered lines.".to_string();
    assert_eq!(
        runtime.run_foreground(cancelled).await,
        RunOutcome::Cancelled
    );
    fixture
        .tools
        .begin_request("codex-live-after-cancel", &project_id)
        .expect("begin post-cancel Codex run");
    let mut restart_binding = binding.clone();
    restart_binding.conversation = runtime.conversation_state();
    let mut restarted = fixture.foreground(restart_binding.clone(), 30_006);
    restarted.identity.request_id = "codex-live-after-cancel".to_string();
    restarted.identity.cancellation_generation = 1;
    restarted.binding = restart_binding;
    restarted.turn.text = "Reply briefly.".to_string();
    assert!(
        matches!(
            runtime.run_foreground(restarted).await,
            RunOutcome::Failed(_)
        ),
        "unsafe native continuation must require an explicit reset"
    );
    runtime
        .reset()
        .await
        .expect("deliberately reset unsafe native context");
    fixture
        .tools
        .begin_request("codex-live-explicit-reset", &project_id)
        .expect("begin explicitly reset Codex run");
    let mut fresh_binding = binding;
    fresh_binding.conversation = runtime.conversation_state();
    let mut fresh = fixture.foreground(fresh_binding, 30_007);
    fresh.identity.request_id = "codex-live-explicit-reset".to_string();
    fresh.identity.cancellation_generation = 1;
    fresh.turn.text = "Reply briefly.".to_string();
    let restarted_state = match runtime.run_foreground(fresh).await {
        RunOutcome::Completed { conversation, .. } => conversation,
        other => panic!("post-cancel Codex run failed: {other:?}"),
    };

    // Then: only the explicit reset permits a new native thread after cancellation.
    assert_ne!(thread_id(&restarted_state), compacted_thread);
}
