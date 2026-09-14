use crate::provider_runtime::CompactionRequest;

use super::*;

fn saved_state(provider: ProviderId) -> ProviderConversationState {
    match provider {
        ProviderId::Codex => ProviderConversationState::Codex {
            thread_id: Some("seeded-codex-thread".into()),
        },
        ProviderId::ClaudeCode => ProviderConversationState::ClaudeCode {
            session_id: Some("seeded-claude-thread".into()),
        },
        ProviderId::Antigravity | ProviderId::OpencodeGo | ProviderId::Ollama => {
            panic!("native fixture required")
        }
    }
}

async fn seeded_compaction(provider: ProviderId, invalid_workspace: bool) {
    // Given: a restored native checkpoint whose adapter has never run a foreground turn.
    let (fixture, mut binding, mut runtime, _) = setup(provider, "native-seeded-compaction");
    let saved = saved_state(provider);
    runtime.seed(saved.clone()).await.unwrap();
    binding.conversation = saved.clone();
    let compact_identity = fixture.identity(55_001);
    let workspace_root = if invalid_workspace {
        fixture.root.join("absent-workspace")
    } else {
        fixture.root.clone()
    };
    let request = CompactionRequest {
        identity: compact_identity.clone(),
        binding: binding.clone(),
        workspace_root,
        next_instruction_epoch: 4,
        policy: super::super::fixtures::policy(Some(Duration::from_secs(20))),
    };

    // When: the first operation after restore is native compaction.
    let compacted = runtime.compact(request).await;

    // Then: confirmed native state resumes, and a local preparation failure cannot poison it.
    if invalid_workspace {
        assert!(
            compacted.is_err(),
            "absent workspace must fail before native execution"
        );
        let path = RunGate::new(
            compact_identity,
            fixture.tools.clone(),
            crate::provider_runtime::WorkspaceAccess::Read,
            None,
        )
        .receipt_path()
        .unwrap();
        assert!(
            !path.exists(),
            "local preparation failure must not create an unknown native receipt"
        );
    } else {
        assert_eq!(compacted.unwrap(), saved);
        let call: Value = serde_json::from_slice(
            &std::fs::read(fixture.root.join("native-compact.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            call["nativeId"].as_str(),
            saved.conversation_key().as_deref()
        );
        assert_eq!(
            call["foregroundTurns"], 0,
            "compaction must not synthesize a foreground turn"
        );
        runtime.acknowledge_persisted().await.unwrap();
    }
    assert_eq!(runtime.conversation_state(), saved);
    let mut foreground = fixture.foreground(binding, 55_002);
    foreground.policy.active_deadline = Some(Duration::from_secs(20));
    let outcome = runtime.run_foreground(foreground).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { ref text, ref conversation, .. } if text.contains("onPluginStart") && conversation == &saved),
        "{outcome:?}"
    );
    runtime.acknowledge_persisted().await.unwrap();
    drop(runtime);
    wait_for_process_exit(&fixture.root).await;
}

#[tokio::test]
async fn codex_saved_checkpoint_compacts_before_foreground_and_resumes() {
    seeded_compaction(ProviderId::Codex, false).await;
}

#[tokio::test]
async fn claude_saved_checkpoint_compacts_before_foreground_and_resumes() {
    seeded_compaction(ProviderId::ClaudeCode, false).await;
}

#[tokio::test]
async fn codex_prelaunch_compaction_failure_preserves_saved_continuation() {
    seeded_compaction(ProviderId::Codex, true).await;
}

#[tokio::test]
async fn claude_prelaunch_compaction_failure_preserves_saved_continuation() {
    seeded_compaction(ProviderId::ClaudeCode, true).await;
}
