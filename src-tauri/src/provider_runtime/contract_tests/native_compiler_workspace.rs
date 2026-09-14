use serde_json::json;

use super::*;
use crate::provider_runtime::{CompilerInputWorkspace, StructuredJobExecutor};

#[derive(Clone, Copy)]
enum Completion {
    Success,
    Error,
    Cancel,
    Timeout,
    Drop,
}

async fn compiler_workspace_lifecycle(provider: ProviderId, completion: Completion) {
    // Given: real native transports and canonical source outside the private compiler cwd.
    let (fixture, binding, runtime, events) = setup(provider, "compiler-workspace");
    let original_path = fixture.root.join("project/src/main.eps");
    let original = std::fs::read(&original_path).unwrap();
    let (_, adapter) = native_adapter(provider, &fixture);
    let mut executor = StructuredJobExecutor::new(adapter, fixture.cancellation.subscribe());
    let workspace = CompilerInputWorkspace::prepare(&fixture.dirs).unwrap();
    let input_root = workspace.root().to_path_buf();
    let schema = json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false});
    let mut request = fixture.structured(binding, 56_001, schema.clone());
    request.workspace_root = input_root.clone();
    request.prompt = match completion {
        Completion::Success => "structured-valid",
        Completion::Error => "structured-wrong-schema",
        Completion::Cancel | Completion::Timeout | Completion::Drop => "structured-hang",
    }
    .into();
    let deadline = Duration::from_secs(60);
    request.policy.active_deadline = Some(deadline);
    assert_eq!(std::fs::read_dir(&input_root).unwrap().count(), 0);

    // When: the compiler's owner closes or drops the same scope as its native execution.
    let mut execution = Box::pin(async move {
        let outcome = executor.run(request).await;
        let cleanup = workspace.close();
        (outcome, cleanup)
    });
    match completion {
        Completion::Success | Completion::Error => {
            let (outcome, cleanup) = execution.await;
            cleanup.unwrap();
            match completion {
                Completion::Success => assert!(matches!(outcome, RunOutcome::Structured { .. })),
                Completion::Error => assert_eq!(
                    outcome,
                    RunOutcome::Failed(ProviderRuntimeError::StructuredOutputInvalid)
                ),
                Completion::Cancel | Completion::Timeout | Completion::Drop => unreachable!(),
            }
        }
        Completion::Cancel | Completion::Timeout | Completion::Drop => {
            let barrier = async {
                while !fixture.root.join("descendant.ready").exists() {
                    tokio::task::yield_now().await;
                }
            };
            tokio::select! {
                result = tokio::time::timeout(Duration::from_secs(10), barrier) => result.unwrap(),
                result = &mut execution => panic!("native compiler ended before interruption: {result:?}"),
            }
            match completion {
                Completion::Cancel => {
                    fixture.cancellation.send(1).unwrap();
                    let (outcome, cleanup) = execution.await;
                    assert_eq!(outcome, RunOutcome::Cancelled);
                    cleanup.unwrap();
                }
                Completion::Timeout => {
                    tokio::time::pause();
                    tokio::time::advance(deadline).await;
                    let (outcome, cleanup) = execution.await;
                    tokio::time::resume();
                    assert_eq!(outcome, RunOutcome::Failed(ProviderRuntimeError::TimedOut));
                    cleanup.unwrap();
                }
                Completion::Drop => drop(execution),
                Completion::Success | Completion::Error => unreachable!(),
            }
        }
    }

    // Then: the actual child cwd was private, its processes and cwd are gone, and source is exact.
    let captured_cwd = std::fs::read_to_string(fixture.root.join("provider-cwd.txt")).unwrap();
    assert_eq!(PathBuf::from(captured_cwd), input_root);
    process_cases::assert_structured_isolation(provider, &fixture, &schema);
    wait_for_process_exit(&fixture.root).await;
    assert!(
        !input_root.exists(),
        "compiler input cwd must be removed after process termination"
    );
    assert_eq!(std::fs::read(original_path).unwrap(), original);
    assert!(events.text.lock().is_empty());
    assert!(events.observations.lock().is_empty());
    assert!(events.finishes.lock().is_empty());
    assert_eq!(
        runtime.conversation_state(),
        ProviderConversationState::empty(provider)
    );
}

#[tokio::test]
async fn compiler_native_cwd_is_private_and_cleaned_after_success() {
    for provider in [ProviderId::Codex, ProviderId::ClaudeCode] {
        compiler_workspace_lifecycle(provider, Completion::Success).await;
    }
}

#[tokio::test]
async fn compiler_native_cwd_is_private_and_cleaned_after_error() {
    for provider in [ProviderId::Codex, ProviderId::ClaudeCode] {
        compiler_workspace_lifecycle(provider, Completion::Error).await;
    }
}

#[tokio::test]
async fn compiler_native_cwd_is_private_and_cleaned_after_cancel() {
    for provider in [ProviderId::Codex, ProviderId::ClaudeCode] {
        compiler_workspace_lifecycle(provider, Completion::Cancel).await;
    }
}

#[tokio::test]
async fn compiler_native_cwd_is_private_and_cleaned_after_timeout() {
    for provider in [ProviderId::Codex, ProviderId::ClaudeCode] {
        compiler_workspace_lifecycle(provider, Completion::Timeout).await;
    }
}

#[tokio::test]
async fn compiler_native_cwd_is_private_and_cleaned_after_future_drop() {
    for provider in [ProviderId::Codex, ProviderId::ClaudeCode] {
        compiler_workspace_lifecycle(provider, Completion::Drop).await;
    }
}
