//! Provider-neutral sequential tool dispatch for direct HTTP providers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod completion;
mod execution;
#[cfg(test)]
mod gate_event_tests;
mod gate_events;
mod gate_state;
mod receipt;
mod run_gate;
mod validation;

pub use completion::DurableToolCompletion;
pub use gate_events::{GateEvent, GateEventKind};
pub use receipt::{
    acknowledge_native_run, clear_native_run_recovery, unresolved_native_runs,
    NativeRunReceiptState, UnresolvedNativeRun,
};
pub use run_gate::RunGate;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectToolResult {
    pub id: String,
    pub name: String,
    pub result: Value,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NormalizedAssistantStep {
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<DirectToolCall>,
    pub usage: Option<crate::ipc::ContextUsage>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DirectDispatchBatch {
    pub results: Vec<DirectToolResult>,
    pub stop_for_write_transition: bool,
}

pub fn validate_structured_output(schema: &Value, value: &Value) -> Result<(), String> {
    let validator = jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(schema)
        .map_err(|_| "provider_structured_output_invalid".to_string())?;
    if let Err(errors) = validator.validate(value) {
        let mut details = errors
            .take(3)
            .map(|error| error.instance_path.to_string())
            .collect::<Vec<_>>();
        details.sort();
        details.dedup();
        return Err(if details.is_empty() {
            "provider_structured_output_invalid".to_string()
        } else {
            format!("provider_structured_output_invalid: {}", details.join(", "))
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_exec::SessionToolRuntime;

    #[test]
    fn structured_output_uses_schema_validation_not_substring_extraction() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "ok": { "type": "boolean" } },
            "required": ["ok"],
            "additionalProperties": false
        });
        assert!(validate_structured_output(&schema, &serde_json::json!({"ok": true})).is_ok());
        assert!(
            validate_structured_output(&schema, &Value::String("{\"ok\":true}".to_string()))
                .is_err()
        );
        assert!(
            validate_structured_output(&schema, &serde_json::json!({"ok": true, "extra": 1}))
                .is_err()
        );
    }

    #[tokio::test]
    async fn duplicate_tool_call_ids_fail_before_dispatch() {
        let runtime = SessionToolRuntime::for_tests();
        let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
        runtime.set_cancellation(cancellation);
        runtime.begin_request("request", "project").unwrap();
        let gate = RunGate::new(
            crate::provider_runtime::RunIdentity {
                session_id: runtime.session_id().to_string(),
                run_id: crate::provider_runtime::RunId::new(10),
                request_id: "request".to_string(),
                session_kind: runtime.kind(),
                cancellation_generation: 0,
            },
            runtime,
            crate::provider_runtime::WorkspaceAccess::Read,
            None,
        );
        let call = DirectToolCall {
            id: "call-1".to_string(),
            name: "list_files".to_string(),
            arguments: serde_json::json!({}),
        };
        let first = gate.dispatch_batch(vec![call.clone()], false).await;
        assert!(first.is_ok());
        assert!(gate.dispatch_batch(vec![call], false).await.is_err());
    }

    #[tokio::test]
    async fn batch_rejection_happens_before_any_tool_is_admitted() {
        // Given: one valid call precedes an unknown call in the same model batch.
        let runtime = SessionToolRuntime::for_tests();
        let (_cancel, cancellation) = tokio::sync::watch::channel(7_u64);
        runtime.set_cancellation(cancellation);
        runtime.begin_request("request-a", "project").unwrap();
        let gate = RunGate::new(
            crate::provider_runtime::RunIdentity {
                session_id: runtime.session_id().to_string(),
                run_id: crate::provider_runtime::RunId::new(11),
                request_id: "request-a".to_string(),
                session_kind: runtime.kind(),
                cancellation_generation: 7,
            },
            runtime,
            crate::provider_runtime::WorkspaceAccess::Read,
            None,
        );

        // When: the complete batch is submitted.
        let error = gate
            .dispatch_batch(
                vec![
                    DirectToolCall {
                        id: "call-1".to_string(),
                        name: "list_files".to_string(),
                        arguments: serde_json::json!({}),
                    },
                    DirectToolCall {
                        id: "call-2".to_string(),
                        name: "not_a_tool".to_string(),
                        arguments: serde_json::json!({}),
                    },
                ],
                false,
            )
            .await
            .unwrap_err();

        // Then: validation rejects the batch without recording a partial execution.
        assert!(error.contains("unknown tool"));
        assert!(gate.completed().is_empty());
    }

    #[tokio::test]
    async fn calls_after_write_transition_are_validated_but_not_executed() {
        // Given: a write transition followed by another valid tool call.
        let runtime = SessionToolRuntime::for_tests();
        let (_cancel, cancellation) = tokio::sync::watch::channel(8_u64);
        runtime.set_cancellation(cancellation);
        runtime.begin_request("request-write", "project").unwrap();
        let gate = RunGate::new(
            crate::provider_runtime::RunIdentity {
                session_id: runtime.session_id().to_string(),
                run_id: crate::provider_runtime::RunId::new(13),
                request_id: "request-write".to_string(),
                session_kind: runtime.kind(),
                cancellation_generation: 8,
            },
            runtime,
            crate::provider_runtime::WorkspaceAccess::Read,
            None,
        );

        // When: the complete batch passes shape validation and dispatch starts.
        let batch = gate
            .dispatch_batch(
                vec![
                    DirectToolCall {
                        id: "call-write".to_string(),
                        name: crate::tools::REQUEST_WRITE_WORKSPACE_TOOL.to_string(),
                        arguments: serde_json::json!({"reason": "test change"}),
                    },
                    DirectToolCall {
                        id: "call-after".to_string(),
                        name: "list_files".to_string(),
                        arguments: serde_json::json!({}),
                    },
                ],
                false,
            )
            .await
            .unwrap();

        // Then: the transition is explicit and the remainder has no completion record.
        assert!(batch.stop_for_write_transition);
        assert_eq!(batch.results.len(), 1);
        assert_eq!(gate.completed().len(), 1);
        assert_eq!(gate.completed()[0].call_id.as_deref(), Some("call-write"));
    }

    #[tokio::test]
    async fn completed_tool_is_checkpointed_before_batch_returns() {
        // Given: a direct run with a transcript checkpoint writer and active batch.
        let runtime = SessionToolRuntime::for_tests();
        let dirs = runtime.data_dirs();
        let (_cancel, cancellation) = tokio::sync::watch::channel(9_u64);
        runtime.set_cancellation(cancellation);
        runtime
            .begin_request("request-checkpoint", "project")
            .unwrap();
        let store = crate::provider_transcript::ProviderTranscriptStore::new(&dirs);
        let branch = crate::provider_transcript::TranscriptBranch::legacy();
        let writer = std::sync::Arc::new(
            store
                .checkpoint_writer(
                    crate::provider::ProviderId::OpencodeGo,
                    runtime.session_id(),
                    0,
                    branch,
                    Vec::new(),
                )
                .unwrap(),
        );
        writer
            .commit_context("request-checkpoint", "inspect", &[])
            .unwrap();
        writer
            .begin_tool_batch(
                "response-1",
                "batch-1",
                &[crate::provider_runtime::NormalizedBlock::ToolCall {
                    response_id: "response-1".to_string(),
                    batch_id: "batch-1".to_string(),
                    call: DirectToolCall {
                        id: "call-1".to_string(),
                        name: "list_files".to_string(),
                        arguments: serde_json::json!({}),
                    },
                    continuation: None,
                }],
            )
            .unwrap();
        let gate = RunGate::new(
            crate::provider_runtime::RunIdentity {
                session_id: runtime.session_id().to_string(),
                run_id: crate::provider_runtime::RunId::new(14),
                request_id: "request-checkpoint".to_string(),
                session_kind: runtime.kind(),
                cancellation_generation: 9,
            },
            runtime,
            crate::provider_runtime::WorkspaceAccess::Read,
            Some(writer),
        );

        // When: a real SessionToolRuntime call finishes through the gate.
        gate.dispatch_batch(
            vec![DirectToolCall {
                id: "call-1".to_string(),
                name: "list_files".to_string(),
                arguments: serde_json::json!({}),
            }],
            false,
        )
        .await
        .unwrap();

        // Then: the atomic transcript head already contains the actual tool result.
        let generation = store
            .load_current(
                crate::provider::ProviderId::OpencodeGo,
                gate.identity().session_id.as_str(),
            )
            .unwrap();
        assert!(matches!(
            generation.checkpoint.boundary,
            crate::provider_transcript::CheckpointBoundary::ToolResultsCommitted { .. }
        ));
        assert!(matches!(
            generation.checkpoint.blocks.last(),
            Some(crate::provider_transcript::TranscriptBlock::ToolResult {
                response_id,
                batch_id,
                id,
                name,
                result,
                is_error,
            }) if response_id == "response-1"
                && batch_id == "batch-1"
                && id == "call-1"
                && name == "list_files"
                && result == &gate.completed()[0].result
                && *is_error == gate.completed()[0].is_error
        ));
    }

    #[tokio::test]
    async fn stale_gate_cannot_execute_against_a_new_request() {
        // Given: a gate bound to the request that created it.
        let runtime = SessionToolRuntime::for_tests();
        let (_cancel, cancellation) = tokio::sync::watch::channel(3_u64);
        runtime.set_cancellation(cancellation);
        runtime.begin_request("request-old", "project").unwrap();
        let gate = RunGate::new(
            crate::provider_runtime::RunIdentity {
                session_id: runtime.session_id().to_string(),
                run_id: crate::provider_runtime::RunId::new(12),
                request_id: "request-old".to_string(),
                session_kind: runtime.kind(),
                cancellation_generation: 3,
            },
            runtime.clone(),
            crate::provider_runtime::WorkspaceAccess::Read,
            None,
        );
        runtime.clear_current();
        runtime.begin_request("request-new", "project").unwrap();

        // When: the old native handler submits a tool after the next request starts.
        let error = gate
            .dispatch_native(
                Some("native-1".to_string()),
                "list_files".to_string(),
                serde_json::json!({}),
            )
            .await
            .unwrap_err();

        // Then: the old run has no authority and no completion is fabricated.
        assert!(error.contains("stale"));
        assert!(gate.completed().is_empty());
    }

    #[tokio::test]
    async fn cancellation_generation_is_rechecked_inside_the_execution_lock() {
        // Given: a tool is admitted for generation four while an earlier worker owns the lock.
        let runtime = SessionToolRuntime::for_tests();
        let (cancel, cancellation) = tokio::sync::watch::channel(4_u64);
        runtime.set_cancellation(cancellation);
        runtime
            .begin_request("request-generation", "project")
            .unwrap();
        let gate = RunGate::new(
            crate::provider_runtime::RunIdentity {
                session_id: runtime.session_id().to_string(),
                run_id: crate::provider_runtime::RunId::new(16),
                request_id: "request-generation".to_string(),
                session_kind: runtime.kind(),
                cancellation_generation: 4,
            },
            runtime.clone(),
            crate::provider_runtime::WorkspaceAccess::Read,
            None,
        );
        let execution_lock = runtime.execution_lock_for_tests();
        let (locked, ready) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let blocker = std::thread::spawn(move || {
            let _guard = execution_lock.lock();
            locked.send(()).unwrap();
            released.recv().unwrap();
        });
        ready.recv().unwrap();
        let dispatch_gate = gate.clone();
        let dispatch = tokio::spawn(async move {
            dispatch_gate
                .dispatch_native(
                    Some("generation-call".to_string()),
                    "list_files".to_string(),
                    serde_json::json!({}),
                )
                .await
        });
        gate.wait_for_admission().await;

        // When: cancellation advances before the admitted worker acquires the execution lock.
        cancel.send_replace(5);
        release.send(()).unwrap();
        blocker.join().unwrap();

        // Then: the in-lock scope check rejects execution without a completion receipt.
        let error = dispatch.await.unwrap().unwrap_err();
        assert!(error.contains("stale provider run"), "got: {error}");
        gate.drain(std::time::Duration::from_secs(2)).await.unwrap();
        assert!(gate.completed().is_empty());
        assert!(!gate.receipt_path().unwrap().exists());
    }

    #[tokio::test]
    async fn closing_gate_cancels_pending_ask_without_a_completion_receipt() {
        // Given: a native run whose ASK request has reached the existing UI coordinator.
        let runtime = SessionToolRuntime::for_tests();
        let (_cancel, cancellation) = tokio::sync::watch::channel(4_u64);
        runtime.set_cancellation(cancellation);
        runtime.begin_request("request-ask", "project").unwrap();
        let (events, mut emitted) = tokio::sync::mpsc::unbounded_channel();
        runtime.set_ask_emitter(move |event| {
            events
                .send(event)
                .map_err(|_| "ask event receiver closed".to_string())
        });
        let gate = RunGate::new(
            crate::provider_runtime::RunIdentity {
                session_id: runtime.session_id().to_string(),
                run_id: crate::provider_runtime::RunId::new(15),
                request_id: "request-ask".to_string(),
                session_kind: runtime.kind(),
                cancellation_generation: 4,
            },
            runtime.clone(),
            crate::provider_runtime::WorkspaceAccess::Read,
            None,
        );
        let dispatch_gate = gate.clone();
        let dispatch = tokio::spawn(async move {
            dispatch_gate
                .dispatch_native(
                    Some("ask-call".to_string()),
                    crate::tools::ASK_TOOL.to_string(),
                    serde_json::json!({
                        "questions": [{
                            "id": "choice",
                            "question": "Choose one",
                            "options": [{"label": "A"}, {"label": "B"}]
                        }]
                    }),
                )
                .await
        });
        emitted
            .recv()
            .await
            .expect("ASK must be pending before close");

        // When: cancellation closes the run while the user response is pending.
        gate.cancel_and_drain(std::time::Duration::from_secs(2))
            .await
            .unwrap();

        // Then: the pending ASK ends, and no fabricated tool completion is durable.
        let error = dispatch.await.unwrap().unwrap_err();
        assert!(error.contains("gate closed"), "got: {error}");
        assert!(runtime.pending_ask().is_none());
        assert!(gate.completed().is_empty());
        assert!(!gate.receipt_path().unwrap().exists());
    }

    #[tokio::test]
    async fn native_completion_receipt_survives_until_checkpoint_acknowledgement() {
        // Given: a native run without a direct-provider transcript writer.
        let runtime = SessionToolRuntime::for_tests();
        let (_cancel, cancellation) = tokio::sync::watch::channel(10_u64);
        runtime.set_cancellation(cancellation);
        runtime.begin_request("request-native", "project").unwrap();
        let gate = RunGate::new(
            crate::provider_runtime::RunIdentity {
                session_id: runtime.session_id().to_string(),
                run_id: crate::provider_runtime::RunId::new(15),
                request_id: "request-native".to_string(),
                session_kind: runtime.kind(),
                cancellation_generation: 10,
            },
            runtime,
            crate::provider_runtime::WorkspaceAccess::Read,
            None,
        );

        // When: the production executor completes a native tool request.
        let result = gate
            .dispatch_native(
                Some("native-1".to_string()),
                "list_files".to_string(),
                serde_json::json!({}),
            )
            .await
            .unwrap();

        // Then: the exact result is atomic on disk until the runtime adopts it.
        let receipt = gate.receipt_path().unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&receipt).unwrap()).unwrap();
        assert_eq!(value["completions"][0]["callId"], "native-1");
        assert_eq!(value["completions"][0]["result"], result.result);
        assert!(receipt.is_file());
        gate.acknowledge_receipts().unwrap();
        assert!(!receipt.exists());
    }
}
