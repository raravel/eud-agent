use serde_json::{json, Value};

use super::*;

#[tokio::test]
async fn c04_codex_partial_process_exit_never_commits_a_finished_native_turn() {
    // Given: an actual app-server process that emits partial text and exits with status 9.
    let (fixture, binding, mut runtime, events) = setup(ProviderId::Codex, "native-partial");
    let mut request = fixture.foreground(binding, 52_001);
    request.turn.text = "partial-exit".to_string();
    request.policy.active_deadline = Some(Duration::from_secs(10));

    // When: the common runtime consumes that process's partial stream and failure.
    let outcome = runtime.run_foreground(request).await;

    // Then: partial text stays observable, but no successful boundary or trusted native ID is adopted.
    assert!(
        matches!(
            outcome,
            RunOutcome::Failed(ProviderRuntimeError::Transport(_))
        ),
        "{outcome:?}"
    );
    assert_eq!(*events.text.lock(), "partial-source-result");
    assert!(!events.finishes.lock().contains(&true));
    assert_eq!(
        runtime.conversation_state(),
        ProviderConversationState::Codex { thread_id: None }
    );
    runtime.reset().await.unwrap();
    drop(runtime);
    wait_for_process_exit(&fixture.root).await;
}

async fn structured_process(provider: ProviderId, prompt: &str) -> RunOutcome {
    let (fixture, binding, runtime, events) = setup(provider, "native-structured");
    let (_, adapter) = native_adapter(provider, &fixture);
    let mut executor = crate::provider_runtime::StructuredJobExecutor::new(
        adapter,
        fixture.cancellation.subscribe(),
    );
    let schema = json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false});
    let mut request = fixture.structured(binding, 53_001, schema.clone());
    std::fs::create_dir_all(&request.workspace_root).unwrap();
    request.prompt = prompt.to_string();
    request.policy.active_deadline = Some(Duration::from_secs(10));
    let outcome = executor.run(request).await;
    assert_structured_isolation(provider, &fixture, &schema);
    assert_eq!(
        runtime.conversation_state(),
        ProviderConversationState::empty(provider)
    );
    assert!(
        events.observations.lock().is_empty(),
        "structured jobs must not publish native tool observations"
    );
    drop(runtime);
    drop(executor);
    wait_for_process_exit(&fixture.root).await;
    outcome
}

pub(super) fn assert_structured_isolation(
    provider: ProviderId,
    fixture: &RuntimeFixture,
    schema: &Value,
) {
    match provider {
        ProviderId::Codex => {
            let args: Vec<String> = serde_json::from_slice(
                &std::fs::read(fixture.root.join("codex-args.json")).unwrap(),
            )
            .unwrap();
            assert!(args.iter().any(|arg| arg == "web_search=\"disabled\""));
            assert!(!args.iter().any(|arg| arg.starts_with("mcp_servers.")));
            let turn: Value = serde_json::from_slice(
                &std::fs::read(fixture.root.join("codex-turn.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(&turn["outputSchema"], schema);
        }
        ProviderId::ClaudeCode => {
            let args: Vec<String> = serde_json::from_slice(
                &std::fs::read(fixture.root.join("claude-args.json")).unwrap(),
            )
            .unwrap();
            for required in ["--strict-mcp-config", "--no-session-persistence"] {
                assert!(args.iter().any(|arg| arg == required));
            }
            for forbidden in ["--mcp-config", "--resume"] {
                assert!(!args.iter().any(|arg| arg == forbidden));
            }
            let tools = args.iter().position(|arg| arg == "--tools").unwrap();
            assert_eq!(args[tools + 1], "");
            let output_schema = args.iter().position(|arg| arg == "--json-schema").unwrap();
            assert_eq!(
                &serde_json::from_str::<Value>(&args[output_schema + 1]).unwrap(),
                schema
            );
        }
        ProviderId::Antigravity | ProviderId::OpencodeGo | ProviderId::Ollama => {
            panic!("native fixture requires a native provider")
        }
    }
}

#[tokio::test]
async fn codex_structured_process_preserves_full_schema_without_native_tool_authority() {
    // Given/When: an actual isolated app-server process emits the requested JSON object.
    let outcome = structured_process(ProviderId::Codex, "structured-valid").await;
    // Then: the runtime returns the validated object.
    assert!(
        matches!(outcome, RunOutcome::Structured { ref value, .. } if value == &json!({"ok":true})),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn codex_structured_process_rejects_schema_incompatible_json() {
    // Given/When: the actual app-server exits normally with JSON whose field has the wrong type.
    let outcome = structured_process(ProviderId::Codex, "structured-wrong-schema").await;
    // Then: parseable JSON does not bypass the common schema validator.
    assert!(
        matches!(
            outcome,
            RunOutcome::Failed(ProviderRuntimeError::StructuredOutputInvalid)
        ),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn codex_structured_process_rejects_native_tool_notifications() {
    // Given/When: the actual isolated app-server attempts a native tool event.
    let outcome = structured_process(ProviderId::Codex, "structured-forbidden-tool").await;
    // Then: the native protocol violation fails without dispatching or publishing the tool.
    assert!(
        matches!(
            outcome,
            RunOutcome::Failed(ProviderRuntimeError::Protocol(_))
        ),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn native_structured_processes_reject_duplicate_and_truncated_outputs() {
    // Given/When: production native codecs receive two complete objects or an incomplete object.
    for provider in [ProviderId::Codex, ProviderId::ClaudeCode] {
        for prompt in ["structured-duplicate", "structured-truncated"] {
            let outcome = structured_process(provider, prompt).await;
            // Then: neither protocol accepts a partial or duplicate schema submission.
            assert_eq!(
                outcome,
                RunOutcome::Failed(ProviderRuntimeError::StructuredOutputInvalid),
                "{provider:?} {prompt}"
            );
        }
    }
}

#[tokio::test]
async fn claude_structured_process_rejects_native_tool_output() {
    // Given/When: a tools-empty CLI emits a forbidden tool event before its success envelope.
    let outcome = structured_process(ProviderId::ClaudeCode, "structured-forbidden-tool").await;
    // Then: it fails without publishing or executing the attempted native tool.
    assert_eq!(
        outcome,
        RunOutcome::Failed(ProviderRuntimeError::StructuredOutputInvalid)
    );
}

#[tokio::test]
async fn codex_structured_partial_exit_rejects_incomplete_result() {
    // Given/When: the isolated app-server emits partial JSON then exits with status 9.
    let outcome = structured_process(ProviderId::Codex, "structured-error").await;
    // Then: a transport failure cannot become a structured success.
    assert!(
        matches!(
            outcome,
            RunOutcome::Failed(ProviderRuntimeError::Transport(_))
        ),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn native_turn_accepts_current_generation_with_unseen_watch_notification() {
    // Given: a new request after an earlier cancellation, with its current generation already fixed.
    for provider in [ProviderId::Codex, ProviderId::ClaudeCode] {
        let (fixture, binding, mut runtime, _) = setup(provider, "native-current-generation");
        fixture.cancellation.send(1).unwrap();
        let mut request = fixture.foreground(binding, 56_001);
        request.identity.cancellation_generation = 1;
        request.policy.active_deadline = Some(Duration::from_secs(20));
        // When: the real native CLI starts from a receiver with an unseen earlier notification.
        let outcome = runtime.run_foreground(request).await;
        // Then: the matching generation completes real MCP calls without a spurious interrupt.
        assert!(
            matches!(outcome, RunOutcome::Completed { ref text, .. } if text.contains("onPluginStart")),
            "{provider:?}: {outcome:?}"
        );
        runtime.acknowledge_persisted().await.unwrap();
        drop(runtime);
        wait_for_process_exit(&fixture.root).await;
    }
}
