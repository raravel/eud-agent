use std::sync::Arc;

use crate::{
    claude_client::ProductionClaudeCodeAdapter,
    provider::{ModelCapabilities, ProviderConversationState, ProviderId},
    provider_runtime::{
        BindingSnapshot, ProviderRuntime, ProviderRuntimeError, RunOutcome, RuntimeEventSink,
        RuntimeExecutor, StructuredJobExecutor,
    },
};

use super::fixtures::RuntimeFixture;

const POLICY_BYTES: usize = 64 * 1024;

struct SilentEvents;

impl RuntimeEventSink for SilentEvents {
    fn emit(
        &self,
        _event: &crate::provider_runtime::AdapterEventKind,
    ) -> Result<(), ProviderRuntimeError> {
        Ok(())
    }
}

fn binding() -> BindingSnapshot {
    BindingSnapshot {
        provider: ProviderId::ClaudeCode,
        model: "provider-default".to_string(),
        reasoning: None,
        base_url: None,
        capabilities: Some(ModelCapabilities {
            strict_structured_output: true,
            ..ModelCapabilities::default()
        }),
        conversation: ProviderConversationState::ClaudeCode { session_id: None },
    }
}

fn adapter(fixture: &RuntimeFixture) -> ProductionClaudeCodeAdapter {
    let script = fixture.root.join("claude-size-fixture.ps1");
    std::fs::write(&script, include_str!("claude_size_fixture.ps1"))
        .expect("write Claude size fixture");
    ProductionClaudeCodeAdapter::new(
        "provider-default".to_string(),
        which::which("powershell.exe").expect("PowerShell fixture executable"),
        fixture.root.join("profile"),
    )
    .expect("construct production Claude adapter")
    .with_prefix_args(vec![
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        script.to_string_lossy().into_owned(),
    ])
}

fn runtime(fixture: &RuntimeFixture, binding: &BindingSnapshot) -> ProviderRuntime {
    ProviderRuntime::new(
        Box::new(adapter(fixture)),
        binding.clone(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        Arc::new(SilentEvents),
    )
    .expect("construct production runtime with Claude adapter")
}

fn structured_witness(fixture: &RuntimeFixture) -> serde_json::Value {
    serde_json::from_slice(
        &std::fs::read(fixture.root.join("structured-size-witness.json"))
            .expect("Claude fixture must record its structured wire size"),
    )
    .expect("structured size witness must be valid JSON")
}

fn foreground_witness(fixture: &RuntimeFixture) -> serde_json::Value {
    serde_json::from_slice(
        &std::fs::read(fixture.root.join("foreground-size-witness.json"))
            .expect("Claude fixture must record its foreground wire size"),
    )
    .expect("foreground size witness must be valid JSON")
}

#[tokio::test]
async fn c01_claude_foreground_raw_jsonl_overhead_does_not_consume_semantic_output_budget() {
    let fixture = RuntimeFixture::new("claude-foreground-raw-size");
    let binding = binding();
    let mut runtime = runtime(&fixture, &binding);
    let mut request = fixture.foreground(binding, 58_001);
    request.policy.max_output_bytes = POLICY_BYTES;
    request.turn.text = "foreground-raw-overhead".to_string();

    let outcome = runtime.run_foreground(request).await;
    let witness = foreground_witness(&fixture);

    assert_eq!(witness["scenario"], 1);
    assert!(witness["rawBytes"].as_u64().unwrap() > 65_536);
    assert!(witness["semanticBytes"].as_u64().unwrap() < 65_536);
    assert!(
        matches!(outcome, RunOutcome::Completed { ref text, .. } if text == "tiny foreground result"),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn c01_claude_foreground_rejects_normalized_output_over_policy_limit() {
    let fixture = RuntimeFixture::new("claude-foreground-semantic-size");
    let binding = binding();
    let mut runtime = runtime(&fixture, &binding);
    let mut request = fixture.foreground(binding, 58_002);
    request.policy.max_output_bytes = POLICY_BYTES;
    request.turn.text = "foreground-semantic-over-limit".to_string();

    let outcome = runtime.run_foreground(request).await;
    let witness = foreground_witness(&fixture);

    assert_eq!(witness["scenario"], 2);
    assert!(witness["rawBytes"].as_u64().unwrap() > 65_536);
    assert!(witness["semanticBytes"].as_u64().unwrap() > 65_536);
    assert!(
        matches!(outcome, RunOutcome::Failed(ProviderRuntimeError::Protocol(ref message)) if message == "provider output exceeded its byte limit"),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn c07_claude_structured_raw_envelope_does_not_consume_validated_value_budget() {
    let fixture = RuntimeFixture::new("claude-structured-raw-size");
    let binding = binding();
    let mut executor = StructuredJobExecutor::new(
        Box::new(adapter(&fixture)),
        fixture.cancellation.subscribe(),
    );
    let schema = serde_json::json!({
        "type": "object",
        "properties": {"ok": {"type": "boolean"}},
        "required": ["ok"],
        "additionalProperties": false
    });
    let mut request = fixture.structured(binding, 58_003, schema);
    request.policy.max_output_bytes = POLICY_BYTES;
    std::fs::create_dir_all(&request.workspace_root).expect("create structured workspace");
    request.prompt = "structured-raw-overhead".to_string();

    let outcome = executor.run(request).await;
    let witness = structured_witness(&fixture);

    assert_eq!(witness["scenario"], 1);
    assert_eq!(witness["paddingChars"], 70_000);
    let policy_bytes = u64::try_from(POLICY_BYTES).expect("policy bytes fit u64");
    assert!(witness["rawBytes"].as_u64().unwrap() > policy_bytes);
    assert!(witness["structuredBytes"].as_u64().unwrap() < policy_bytes);
    assert!(
        matches!(outcome, RunOutcome::Structured { ref value, .. } if value == &serde_json::json!({"ok": true})),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn c07_claude_structured_rejects_validated_value_over_policy_limit() {
    let fixture = RuntimeFixture::new("claude-structured-semantic-size");
    let binding = binding();
    let mut executor = StructuredJobExecutor::new(
        Box::new(adapter(&fixture)),
        fixture.cancellation.subscribe(),
    );
    let schema = serde_json::json!({
        "type": "object",
        "properties": {"value": {"type": "string"}},
        "required": ["value"],
        "additionalProperties": false
    });
    let mut request = fixture.structured(binding, 58_004, schema);
    request.policy.max_output_bytes = POLICY_BYTES;
    std::fs::create_dir_all(&request.workspace_root).expect("create structured workspace");
    request.prompt = "structured-semantic-over-limit".to_string();

    let outcome = executor.run(request).await;
    let witness = structured_witness(&fixture);

    assert_eq!(witness["scenario"], 2);
    assert_eq!(witness["paddingChars"], 0);
    let policy_bytes = u64::try_from(POLICY_BYTES).expect("policy bytes fit u64");
    assert!(witness["rawBytes"].as_u64().unwrap() > policy_bytes);
    assert!(witness["structuredBytes"].as_u64().unwrap() > policy_bytes);
    assert!(
        matches!(outcome, RunOutcome::Failed(ProviderRuntimeError::Protocol(ref message)) if message == "provider output exceeded its byte limit"),
        "{outcome:?}"
    );
}
