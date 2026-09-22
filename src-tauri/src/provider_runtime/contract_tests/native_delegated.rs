use std::{sync::Arc, time::Duration};

use serde_json::json;

use crate::{
    provider::ProviderId,
    provider_runtime::{
        DelegatedRunExecutor, DelegatedRunKind, DelegatedRunOutcome, DelegatedRunRequest,
        ProviderRuntimeError, RunPolicy,
    },
    provider_tool_loop::DelegatedToolProfile,
};

use super::{native_adapter, wait_for_process_exit, NativeEvents};
use crate::provider_runtime::contract_tests::fixtures::RuntimeFixture;

fn schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "files": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["summary", "files"],
        "additionalProperties": false
    })
}

async fn delegated(provider: ProviderId, tag: &str, run: u64, prompt: &str) -> DelegatedRunOutcome {
    delegated_within(provider, tag, run, prompt, Duration::from_secs(30)).await
}

async fn delegated_within(
    provider: ProviderId,
    tag: &str,
    run: u64,
    prompt: &str,
    active_deadline: Duration,
) -> DelegatedRunOutcome {
    let fixture = RuntimeFixture::new(tag);
    let (binding, adapter) = native_adapter(provider, &fixture);
    let events = Arc::new(NativeEvents::default());
    let mut executor = DelegatedRunExecutor::new(
        adapter,
        fixture.tools.clone(),
        events,
        fixture.cancellation.subscribe(),
    );
    let outcome = executor
        .run(DelegatedRunRequest {
            identity: fixture.identity(run),
            parent_run_id: None,
            binding,
            kind: DelegatedRunKind::Research,
            prompt: prompt.to_string(),
            workspace_root: fixture.root.join("project"),
            workspace_temp: Some(fixture.root.join("temp")),
            output_schema: schema(),
            profile: DelegatedToolProfile::new(["list_files", "read_file"], &schema()).unwrap(),
            allow_live_write_ticket: false,
            policy: RunPolicy {
                active_deadline: Some(active_deadline),
                shutdown_grace: Duration::from_secs(2),
                max_output_bytes: 64 * 1024,
                max_output_tokens: None,
                max_tool_rounds: 4,
                allow_resume: false,
            },
        })
        .await;
    // A delegated run never registers write intent or leaves a native receipt.
    assert!(!fixture.tools.owns_write_registration());
    assert!(!fixture.root.join("project/src/x.eps").exists());
    assert!(crate::provider_tool_loop::unresolved_native_runs(
        &fixture.dirs.journal_dir(),
        &fixture.session_id
    )
    .unwrap()
    .is_empty());
    drop(executor);
    wait_for_process_exit(&fixture.root).await;
    outcome
}

async fn native_submits_result(provider: ProviderId, run: u64) {
    let outcome = delegated(provider, "native-delegated-submit", run, "delegated-submit").await;
    assert_eq!(
        outcome,
        DelegatedRunOutcome::Result {
            value: json!({"summary": "one entry module", "files": ["src/main.eps"]}),
            completions: 3,
            usage: None,
        },
        "{provider:?}"
    );
}

async fn native_write_is_fatal(provider: ProviderId, run: u64) {
    let outcome = delegated(provider, "native-delegated-write", run, "delegated-write").await;
    assert!(
        matches!(
            &outcome,
            DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(message))
                if message.contains("unknown tool 'file_create'")
        ),
        "{provider:?}: {outcome:?}"
    );
}

/// A submission accepted before the deadline is the result even when the
/// native CLI then keeps its turn open until the deadline cuts it.
async fn native_submission_survives_the_deadline(provider: ProviderId, run: u64) {
    let outcome = delegated_within(
        provider,
        "native-delegated-submit-hang",
        run,
        "delegated-submit-hang",
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(
        outcome,
        DelegatedRunOutcome::Result {
            value: json!({"summary": "submitted early", "files": []}),
            completions: 1,
            usage: None,
        },
        "{provider:?}"
    );
}

async fn native_prose_fails(provider: ProviderId, run: u64) {
    let outcome = delegated(provider, "native-delegated-prose", run, "delegated-prose").await;
    assert_eq!(
        outcome,
        DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
            "delegated run ended without submit_result".into()
        )),
        "{provider:?}"
    );
}

#[tokio::test]
async fn codex_delegated_run_submits_over_its_run_mcp_endpoint() {
    native_submits_result(ProviderId::Codex, 61_001).await;
}

#[tokio::test]
async fn claude_delegated_run_submits_over_its_run_mcp_endpoint() {
    native_submits_result(ProviderId::ClaudeCode, 61_002).await;
}

#[tokio::test]
async fn codex_delegated_write_is_a_fatal_unknown_tool() {
    native_write_is_fatal(ProviderId::Codex, 61_003).await;
}

#[tokio::test]
async fn claude_delegated_write_is_a_fatal_unknown_tool() {
    native_write_is_fatal(ProviderId::ClaudeCode, 61_004).await;
}

#[tokio::test]
async fn codex_delegated_submission_survives_a_deadline_on_the_open_turn() {
    native_submission_survives_the_deadline(ProviderId::Codex, 61_007).await;
}

#[tokio::test]
async fn claude_delegated_submission_survives_a_deadline_on_the_open_turn() {
    native_submission_survives_the_deadline(ProviderId::ClaudeCode, 61_008).await;
}

#[tokio::test]
async fn codex_delegated_prose_without_submission_fails() {
    native_prose_fails(ProviderId::Codex, 61_005).await;
}

#[tokio::test]
async fn claude_delegated_prose_without_submission_fails() {
    native_prose_fails(ProviderId::ClaudeCode, 61_006).await;
}
