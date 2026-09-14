use std::time::Duration;

use serde_json::Value;

use crate::{
    provider::ProviderId,
    provider_runtime::{ProviderRuntimeError, RunOutcome, RuntimeExecutor},
    provider_tool_loop::{DurableToolCompletion, RunGate},
};

use super::{setup, wait_for_process_exit};

fn install_scenario(root: &std::path::Path, scenario: &str) {
    std::fs::write(
        root.join("native-fixture.ps1"),
        include_str!("native_gate_publication_fixture.ps1"),
    )
    .unwrap();
    std::fs::write(root.join("gate-scenario.txt"), scenario).unwrap();
}

#[tokio::test]
async fn native_unknown_mcp_call_cannot_finish_as_product_success() {
    let (fixture, binding, mut runtime, _events) = setup(ProviderId::Codex, "native-gate-unknown");
    install_scenario(&fixture.root, "unknown");
    let mut request = fixture.foreground(binding, 51_101);
    request.policy.active_deadline = Some(Duration::from_secs(20));

    let outcome = runtime.run_foreground(request).await;

    assert!(
        matches!(
            outcome,
            RunOutcome::Failed(ProviderRuntimeError::Protocol(ref error))
                if error.contains("unknown tool")
        ),
        "{outcome:?}"
    );
    drop(runtime);
    wait_for_process_exit(&fixture.root).await;
}

#[tokio::test]
async fn native_executed_tool_error_allows_normal_final_response() {
    let (fixture, binding, mut runtime, _events) =
        setup(ProviderId::Codex, "native-gate-tool-error");
    install_scenario(&fixture.root, "tool-error");
    let run_id = 51_102;
    let mut request = fixture.foreground(binding, run_id);
    request.policy.active_deadline = Some(Duration::from_secs(20));

    let outcome = runtime.run_foreground(request).await;

    assert!(
        matches!(
            outcome,
            RunOutcome::Completed { ref text, .. } if text == "fixture completed"
        ),
        "{outcome:?}"
    );
    let receipt_path = RunGate::new(
        fixture.identity(run_id),
        fixture.tools.clone(),
        crate::provider_runtime::WorkspaceAccess::Read,
        None,
    )
    .receipt_path()
    .unwrap();
    let receipt: Value = serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    let completions: Vec<DurableToolCompletion> =
        serde_json::from_value(receipt["completions"].clone()).unwrap();
    assert!(matches!(completions.as_slice(), [completion]
        if completion.name == "read_file" && completion.is_error));
    runtime.acknowledge_persisted().await.unwrap();
    assert!(!receipt_path.exists());
    drop(runtime);
    wait_for_process_exit(&fixture.root).await;
}
