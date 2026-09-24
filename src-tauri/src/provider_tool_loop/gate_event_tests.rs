use crate::{
    provider_runtime::{RunId, RunIdentity},
    tool_exec::SessionToolRuntime,
};

use super::{GateEventKind, RunGate};

fn gate(runtime: SessionToolRuntime, run_id: u64, request_id: &str) -> RunGate {
    RunGate::new(
        RunIdentity {
            session_id: runtime.session_id().to_string(),
            run_id: RunId::new(run_id),
            request_id: request_id.to_string(),
            session_kind: runtime.kind(),
            cancellation_generation: 0,
        },
        runtime,
        crate::provider_runtime::WorkspaceAccess::Read,
        None,
    )
}

#[tokio::test]
async fn gate_publishes_one_start_and_one_durable_completion() {
    let runtime = SessionToolRuntime::for_tests();
    let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
    runtime.set_cancellation(cancellation);
    runtime.begin_request("event-request", "project").unwrap();
    let gate = gate(runtime, 81_001, "event-request");
    let mut events = gate.take_events().unwrap();

    let result = gate
        .dispatch_native(
            Some("event-call".to_string()),
            "list_files".to_string(),
            serde_json::json!({}),
        )
        .await
        .unwrap();

    let started = events.recv().await.unwrap();
    assert_eq!(started.identity, *gate.identity());
    assert!(matches!(
        started.kind,
        GateEventKind::Started(ref call) if call.id == "event-call" && call.name == "list_files"
    ));
    let completed = events.recv().await.unwrap();
    assert_eq!(completed.identity, *gate.identity());
    assert!(matches!(
        completed.kind,
        GateEventKind::Completed(ref event_result) if event_result == &result
    ));
    assert!(gate.receipt_path().unwrap().is_file());
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn native_protocol_failure_is_fatal_only_for_its_gate_generation() {
    let runtime = SessionToolRuntime::for_tests();
    let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
    runtime.set_cancellation(cancellation);
    runtime.begin_request("old-request", "project").unwrap();
    let old_gate = gate(runtime.clone(), 81_002, "old-request");
    let mut fatal = old_gate.subscribe_fatal();

    let error = old_gate
        .dispatch_native(
            Some("invalid-call".to_string()),
            "not_a_tool".to_string(),
            serde_json::json!({}),
        )
        .await
        .unwrap_err();

    assert!(error.contains("unknown tool"));
    fatal.changed().await.unwrap();
    assert_eq!(
        old_gate.fatal_admission_error().as_deref(),
        Some(error.as_str())
    );
    assert!(old_gate.completed().is_empty());
    let closed = old_gate
        .dispatch_native(
            Some("blocked-call".to_string()),
            "list_files".to_string(),
            serde_json::json!({}),
        )
        .await
        .unwrap_err();
    assert!(closed.contains("closed"));

    runtime.clear_current();
    runtime.begin_request("new-request", "project").unwrap();
    let new_gate = gate(runtime, 81_003, "new-request");
    assert!(new_gate.fatal_admission_error().is_none());
}

#[tokio::test]
async fn executed_tool_error_is_a_recoverable_completion() {
    let runtime = SessionToolRuntime::for_tests();
    let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
    runtime.set_cancellation(cancellation);
    runtime
        .begin_request("tool-error-request", "project")
        .unwrap();
    let gate = gate(runtime, 81_004, "tool-error-request");

    let result = gate
        .dispatch_native(
            Some("tool-error-call".to_string()),
            "read_file".to_string(),
            serde_json::json!({"path": "src/does-not-exist.eps"}),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(gate.fatal_admission_error().is_none());
    assert_eq!(gate.completed().len(), 1);
}

#[tokio::test]
async fn unanswered_native_ask_completes_as_a_durable_unanswered_result() {
    // A native CLI aborts a silent MCP call after 300s; the gate must complete
    // the ask itself, before that, with the text-handoff result.
    let runtime = SessionToolRuntime::for_tests();
    runtime.set_ask_wait_timeout(std::time::Duration::from_millis(50));
    let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
    runtime.set_cancellation(cancellation);
    runtime.set_ask_emitter(|_| Ok(()));
    runtime
        .begin_request("ask-expiry-request", "project")
        .unwrap();
    let gate = gate(runtime.clone(), 81_009, "ask-expiry-request");

    let result = gate
        .dispatch_native(
            Some("ask-expiry-call".to_string()),
            "ask".to_string(),
            serde_json::json!({
                "questions": [{
                    "id": "mode",
                    "question": "방식을 고르세요.",
                    "options": [{"label": "A"}, {"label": "B"}]
                }]
            }),
        )
        .await
        .unwrap();

    assert!(!result.is_error, "{result:?}");
    assert_eq!(result.result["status"], "unanswered");
    assert_eq!(result.result["questionIds"], serde_json::json!(["mode"]));
    assert!(runtime.pending_ask().is_none());
    assert!(runtime.ask_expired_for_request("ask-expiry-request"));
    assert!(gate.fatal_admission_error().is_none());
    assert_eq!(gate.completed().len(), 1);
    assert!(gate.receipt_path().unwrap().is_file());
}

#[tokio::test]
async fn native_schema_failures_are_recoverable_usage_completions() {
    for (run_id, request_id, arguments) in [
        (81_005, "schema-request", serde_json::json!({})),
        (
            81_006,
            "schema-type-request",
            serde_json::json!({"path": 7}),
        ),
    ] {
        let runtime = SessionToolRuntime::for_tests();
        let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
        runtime.set_cancellation(cancellation);
        runtime.begin_request(request_id, "project").unwrap();
        let gate = gate(runtime, run_id, request_id);

        let result = gate
            .dispatch_native(
                Some("schema-call".to_string()),
                "read_file".to_string(),
                arguments,
            )
            .await
            .unwrap();

        // The model receives actionable usage guidance, not a bare mismatch.
        assert!(result.is_error);
        let message = result.result.as_str().unwrap();
        assert!(message.contains("Usage: read_file(path)"), "{message}");
        assert!(
            message.contains("arguments do not match the documented input schema"),
            "{message}"
        );
        // No fatal error: admission stays open and the run continues.
        assert!(gate.fatal_admission_error().is_none());
        assert_eq!(gate.completed().len(), 1);
        assert!(gate.completed()[0].is_error);
    }
}

#[tokio::test]
async fn native_duplicate_failures_close_admission() {
    let runtime = SessionToolRuntime::for_tests();
    let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
    runtime.set_cancellation(cancellation);
    runtime
        .begin_request("duplicate-request", "project")
        .unwrap();
    let gate = gate(runtime, 81_007, "duplicate-request");
    gate.dispatch_native(
        Some("duplicate-call".to_string()),
        "list_files".to_string(),
        serde_json::json!({}),
    )
    .await
    .unwrap();

    let duplicate = gate
        .dispatch_native(
            Some("duplicate-call".to_string()),
            "list_files".to_string(),
            serde_json::json!({}),
        )
        .await
        .unwrap_err();
    assert!(duplicate.contains("duplicate"));
    assert_eq!(
        gate.fatal_admission_error().as_deref(),
        Some(duplicate.as_str())
    );
    assert_eq!(gate.completed().len(), 1);
}
