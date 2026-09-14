use serde_json::json;

use super::*;

#[derive(Clone, Copy)]
enum Interruption {
    Timeout,
    Drop,
}

async fn native_structured_interruption(provider: ProviderId, interruption: Interruption) {
    // Given: an isolated production native process with a real, recorded hanging descendant.
    let (fixture, binding, runtime, events) = setup(provider, "native-structured-lifecycle");
    let (_, adapter) = native_adapter(provider, &fixture);
    let mut executor = crate::provider_runtime::StructuredJobExecutor::new(
        adapter,
        fixture.cancellation.subscribe(),
    );
    let schema = json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false});
    let mut request = fixture.structured(binding, 54_001, schema.clone());
    std::fs::create_dir_all(&request.workspace_root).unwrap();
    request.prompt = "structured-hang".to_string();
    let deadline = Duration::from_secs(60);
    request.policy.active_deadline = Some(deadline);
    let mut execution = Box::pin(executor.run(request));
    let ready = fixture.root.join("descendant.ready");
    let barrier = async {
        while !ready.exists() {
            tokio::task::yield_now().await;
        }
    };
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(10), barrier) => result.expect("native structured process must publish its descendant barrier"),
        outcome = &mut execution => panic!("structured process ended before interruption: {outcome:?}"),
    }
    assert_structured_isolation(provider, &fixture, &schema);

    // When: the controlled clock expires the active deadline, or the owning Future is dropped.
    match interruption {
        Interruption::Timeout => {
            tokio::time::pause();
            tokio::time::advance(deadline).await;
            let outcome = execution.await;
            tokio::time::resume();
            assert_eq!(outcome, RunOutcome::Failed(ProviderRuntimeError::TimedOut));
        }
        Interruption::Drop => drop(execution),
    }

    // Then: both PIDs have terminated before fixture cleanup, with no main-session publication.
    wait_for_process_exit(&fixture.root).await;
    assert_eq!(
        runtime.conversation_state(),
        ProviderConversationState::empty(provider)
    );
    assert!(events.text.lock().is_empty());
    assert!(events.observations.lock().is_empty());
    assert!(events.finishes.lock().is_empty());
    drop(executor);
    drop(runtime);
}

use super::process_cases::assert_structured_isolation;

#[tokio::test]
async fn codex_structured_timeout_terminates_parent_and_descendant() {
    native_structured_interruption(ProviderId::Codex, Interruption::Timeout).await;
}

#[tokio::test]
async fn codex_structured_drop_terminates_parent_and_descendant() {
    native_structured_interruption(ProviderId::Codex, Interruption::Drop).await;
}

#[tokio::test]
async fn claude_structured_timeout_terminates_parent_and_descendant() {
    native_structured_interruption(ProviderId::ClaudeCode, Interruption::Timeout).await;
}

#[tokio::test]
async fn claude_structured_drop_terminates_parent_and_descendant() {
    native_structured_interruption(ProviderId::ClaudeCode, Interruption::Drop).await;
}
