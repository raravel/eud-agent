use std::{path::PathBuf, sync::Arc, time::Duration};

use serde_json::Value;

use crate::{
    claude_client::ProductionClaudeCodeAdapter,
    codex_adapter::CodexAdapter,
    codex_client::CodexLaunchConfig,
    provider::{ModelCapabilities, ProviderConversationState, ProviderId},
    provider_runtime::{
        BindingSnapshot, ProviderAdapter, ProviderRuntime, ProviderRuntimeError, RunOutcome,
        RuntimeExecutor,
    },
    provider_tool_loop::{DurableToolCompletion, RunGate},
};

use super::fixtures::RuntimeFixture;

#[path = "native_mcp_events.rs"]
mod native_mcp_events;
use native_mcp_events::{AuthoritativeToolEvent, NativeEvents};

#[path = "native_compiler_workspace.rs"]
mod compiler_workspace;
#[path = "native_delegated.rs"]
mod delegated;
#[path = "native_gate_publication.rs"]
mod gate_publication;
#[path = "native_process_cases.rs"]
mod process_cases;
#[path = "native_receipt_reset_process.rs"]
mod receipt_reset_process;
#[path = "native_seeded_compaction.rs"]
mod seeded_compaction;
#[path = "native_structured_lifecycle.rs"]
mod structured_lifecycle;

fn native_adapter(
    provider: ProviderId,
    fixture: &RuntimeFixture,
) -> (BindingSnapshot, Box<dyn ProviderAdapter>) {
    let script = fixture.root.join("native-fixture.ps1");
    std::fs::write(&script, include_str!("native_mcp_fixture.ps1")).unwrap();
    let executable = which::which("powershell.exe").unwrap();
    let mut prefix = vec![
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        script.to_string_lossy().into_owned(),
    ];
    let (model, adapter): (&str, Box<dyn ProviderAdapter>) = match provider {
        ProviderId::Codex => {
            prefix.push("codex".to_string());
            (
                "fixture-model",
                Box::new(CodexAdapter::new(
                    fixture.root.clone(),
                    CodexLaunchConfig {
                        executable,
                        executable_args: prefix.into_iter().map(Into::into).collect(),
                        profile_dir: fixture.root.join("profile"),
                        large_context_models: Default::default(),
                    },
                )),
            )
        }
        ProviderId::ClaudeCode => {
            prefix.push("claude".to_string());
            (
                "provider-default",
                Box::new(
                    ProductionClaudeCodeAdapter::new(
                        "provider-default".to_string(),
                        executable,
                        fixture.root.join("profile"),
                    )
                    .unwrap()
                    .with_prefix_args(prefix),
                ),
            )
        }
        ProviderId::Antigravity | ProviderId::OpencodeGo | ProviderId::Ollama => {
            panic!("native fixture requires a native provider")
        }
    };
    let binding = BindingSnapshot {
        provider,
        model: model.to_string(),
        reasoning: None,
        base_url: None,
        capabilities: Some(ModelCapabilities {
            tool_calls: true,
            strict_structured_output: true,
            native_compaction: true,
            ..ModelCapabilities::default()
        }),
        conversation: ProviderConversationState::empty(provider),
    };
    (binding, adapter)
}

fn setup(
    provider: ProviderId,
    tag: &str,
) -> (
    RuntimeFixture,
    BindingSnapshot,
    ProviderRuntime,
    Arc<NativeEvents>,
) {
    let fixture = RuntimeFixture::new(tag);
    let (binding, adapter) = native_adapter(provider, &fixture);
    let events = Arc::new(NativeEvents::default());
    let runtime = ProviderRuntime::new(
        adapter,
        binding.clone(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        events.clone(),
    )
    .unwrap();
    (fixture, binding, runtime, events)
}

async fn wait_for_process_exit(root: &std::path::Path) {
    let markers = std::fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("process-"))
        .map(|entry| entry.path())
        .collect::<Vec<PathBuf>>();
    assert!(!markers.is_empty(), "the native CLI must actually spawn");
    for marker in markers {
        let pid = std::fs::read_to_string(marker)
            .unwrap()
            .parse::<u32>()
            .unwrap();
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
        .expect("native fixture process must exit before cleanup");
    }
}

async fn native_mcp_sequence(provider: ProviderId, run_id: u64) {
    // Given: a production native adapter with a real executable and the runtime's real MCP server.
    let (fixture, binding, mut runtime, events) = setup(provider, "native-mcp");
    let mut request = fixture.foreground(binding, run_id);
    request.policy.active_deadline = Some(Duration::from_secs(20));

    // When: the CLI consumes list_files, then chooses read_file from that result and reports both calls.
    let outcome = runtime.run_foreground(request).await;

    // Then: native observations do not dispatch the two HTTP tool calls a second time.
    assert!(
        matches!(outcome, RunOutcome::Completed { ref text, .. } if text.contains("onPluginStart")),
        "{outcome:?}"
    );
    let path = RunGate::new(
        fixture.identity(run_id),
        fixture.tools.clone(),
        crate::provider_runtime::WorkspaceAccess::Read,
        None,
    )
    .receipt_path()
    .unwrap();
    let receipt: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let completions: Vec<DurableToolCompletion> =
        serde_json::from_value(receipt["completions"].clone()).unwrap();
    assert_eq!(
        completions.len(),
        2,
        "observations must never reexecute HTTP MCP calls"
    );
    assert_eq!(
        completions
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>(),
        ["list_files", "read_file"]
    );
    for (index, completion) in completions.iter().enumerate() {
        assert!(!completion.is_error);
        assert_eq!(completion.run_id, fixture.identity(run_id).run_id);
        assert_eq!(completion.request_id, fixture.request_id);
        assert!(
            completion
                .call_id
                .as_deref()
                .unwrap()
                .ends_with(&format!("String(\"native-mcp-request-{}\")", index + 1)),
            "the scoped gate id must retain its exact native JSON-RPC source id"
        );
    }
    let first_id = completions[0].call_id.clone().unwrap();
    let second_id = completions[1].call_id.clone().unwrap();
    assert_ne!(
        first_id, second_id,
        "the two native calls need distinct scoped ids"
    );
    assert!(completions[1].result.to_string().contains("onPluginStart"));
    assert_eq!(
        *events.authoritative_tools.lock(),
        vec![
            AuthoritativeToolEvent::Call {
                id: first_id.clone(),
                name: "list_files".to_string(),
                arguments: serde_json::json!({}),
            },
            AuthoritativeToolEvent::Result {
                id: first_id,
                name: "list_files".to_string(),
                result: completions[0].result.clone(),
                is_error: false,
            },
            AuthoritativeToolEvent::Call {
                id: second_id.clone(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "src/main.eps"}),
            },
            AuthoritativeToolEvent::Result {
                id: second_id,
                name: "read_file".to_string(),
                result: completions[1].result.clone(),
                is_error: false,
            },
        ],
        "the run gate must publish each authoritative tool call/result exactly once in wire order",
    );
    assert!(
        events.observations.lock().is_empty(),
        "EUD MCP observations duplicate authoritative gate events"
    );
    assert_eq!(*events.finishes.lock(), vec![true]);
    runtime.acknowledge_persisted().await.unwrap();
    assert!(!path.exists());
    drop(runtime);
    wait_for_process_exit(&fixture.root).await;
}

#[tokio::test]
async fn c02_c13_codex_process_consumes_two_real_mcp_results_exactly_once() {
    native_mcp_sequence(ProviderId::Codex, 51_001).await;
}

#[tokio::test]
async fn c02_c13_claude_process_consumes_two_real_mcp_results_exactly_once() {
    native_mcp_sequence(ProviderId::ClaudeCode, 51_002).await;
}
