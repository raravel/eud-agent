use super::*;

#[test]
fn completed_tool_batch_checkpoint_survives_later_transport_failure() {
    let (base, store) = store("completed-batch");
    let checkpoint = TranscriptCheckpoint {
        branch: TranscriptBranch::legacy(),
        blocks: vec![
            TranscriptBlock::User {
                request_id: "request-1".to_string(),
                text: "inspect".to_string(),
                images: Vec::new(),
            },
            TranscriptBlock::ToolCall {
                response_id: "response-1".to_string(),
                batch_id: "batch-1".to_string(),
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path":"src/main.eps"}),
                audit_only: false,
                continuation: None,
            },
            TranscriptBlock::ToolCall {
                response_id: "response-1".to_string(),
                batch_id: "batch-1".to_string(),
                id: "call-2".to_string(),
                name: "project_status".to_string(),
                arguments: serde_json::json!({}),
                audit_only: false,
                continuation: None,
            },
            TranscriptBlock::ToolResult {
                response_id: "response-1".to_string(),
                batch_id: "batch-1".to_string(),
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                result: serde_json::json!({"text":"Trigger(...)"}),
                is_error: false,
            },
            TranscriptBlock::ToolResult {
                response_id: "response-1".to_string(),
                batch_id: "batch-1".to_string(),
                id: "call-2".to_string(),
                name: "project_status".to_string(),
                result: serde_json::json!({"ok":true}),
                is_error: false,
            },
        ],
        boundary: CheckpointBoundary::CompletedToolBatch {
            response_id: "response-1".to_string(),
            batch_id: "batch-1".to_string(),
        },
        continuation: None,
    };

    let published = store
        .publish_checkpoint(ProviderId::OpencodeGo, "session-a", 0, checkpoint.clone())
        .unwrap();
    let restored = store
        .restore(
            ProviderId::OpencodeGo,
            "session-a",
            0,
            &TranscriptBranch::legacy(),
        )
        .unwrap()
        .unwrap();

    assert_eq!(published.checkpoint, checkpoint);
    assert_eq!(restored.generation, published);
    assert!(restored.metadata_was_stale);
    fs::remove_dir_all(base).ok();
}

#[test]
fn checkpoint_rejects_incomplete_batch_without_replacing_valid_head() {
    let (base, store) = store("incomplete-batch");
    let complete = TranscriptCheckpoint {
        branch: TranscriptBranch::legacy(),
        blocks: vec![TranscriptBlock::AssistantText {
            response_id: "response-1".to_string(),
            text: "done".to_string(),
        }],
        boundary: CheckpointBoundary::ResponseCompleted {
            response_id: "response-1".to_string(),
        },
        continuation: None,
    };
    store
        .publish_checkpoint(ProviderId::Ollama, "session-a", 0, complete)
        .unwrap();
    let incomplete = TranscriptCheckpoint {
        branch: TranscriptBranch::legacy(),
        blocks: vec![TranscriptBlock::ToolCall {
            response_id: "response-2".to_string(),
            batch_id: "batch-2".to_string(),
            id: "call-1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({}),
            audit_only: false,
            continuation: None,
        }],
        boundary: CheckpointBoundary::CompletedToolBatch {
            response_id: "response-2".to_string(),
            batch_id: "batch-2".to_string(),
        },
        continuation: None,
    };

    assert!(store
        .publish_checkpoint(ProviderId::Ollama, "session-a", 1, incomplete)
        .is_err());
    assert_eq!(
        store
            .load_current(ProviderId::Ollama, "session-a")
            .unwrap()
            .revision,
        1
    );
    fs::remove_dir_all(base).ok();
}

#[test]
fn provisional_tool_completion_is_durable_but_not_resumable_after_drop() {
    let (base, store) = store("provisional-drop");
    let branch = TranscriptBranch {
        instruction_epoch: 3,
        task_leaf_id: Some("leaf-3".to_string()),
    };
    let writer = store
        .checkpoint_writer(
            ProviderId::OpencodeGo,
            "session-a",
            0,
            branch.clone(),
            Vec::new(),
        )
        .unwrap();
    writer.commit_context("request-1", "inspect", &[]).unwrap();
    writer
        .begin_tool_batch(
            "response-1",
            "batch-1",
            &[
                crate::provider_runtime::NormalizedBlock::Reasoning {
                    response_id: "response-1".to_string(),
                    text: "checking".to_string(),
                    continuation: None,
                },
                crate::provider_runtime::NormalizedBlock::ToolCall {
                    response_id: "response-1".to_string(),
                    batch_id: "batch-1".to_string(),
                    call: crate::provider_tool_loop::DirectToolCall {
                        id: "call-1".to_string(),
                        name: "read_file".to_string(),
                        arguments: serde_json::json!({"path":"src/main.eps"}),
                    },
                    continuation: None,
                },
                crate::provider_runtime::NormalizedBlock::ToolCall {
                    response_id: "response-1".to_string(),
                    batch_id: "batch-1".to_string(),
                    call: crate::provider_tool_loop::DirectToolCall {
                        id: "call-2".to_string(),
                        name: "project_status".to_string(),
                        arguments: serde_json::json!({}),
                    },
                    continuation: None,
                },
            ],
        )
        .unwrap();
    writer
        .commit_provisional_tool_result(
            &crate::provider_tool_loop::DirectToolCall {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path":"src/main.eps"}),
            },
            &crate::provider_tool_loop::DirectToolResult {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                result: serde_json::json!({"text":"Trigger(...)"}),
                is_error: false,
            },
        )
        .unwrap();
    drop(writer);

    assert!(store
        .restore(ProviderId::OpencodeGo, "session-a", 1, &branch)
        .is_err());
    let restored = store
        .load_current(ProviderId::OpencodeGo, "session-a")
        .unwrap();

    assert_eq!(restored.revision, 2);
    assert!(!restored.checkpoint.boundary.is_resumable());
    assert_eq!(
        restored
            .checkpoint
            .blocks
            .iter()
            .filter(|block| matches!(block, TranscriptBlock::ToolResult { .. }))
            .count(),
        1
    );
    assert!(restored
        .checkpoint
        .blocks
        .iter()
        .any(|block| matches!(block, TranscriptBlock::ToolCall { id, .. } if id == "call-2")));
    assert!(restored
        .checkpoint
        .blocks
        .iter()
        .any(|block| matches!(block, TranscriptBlock::AssistantReasoning { text, .. } if text == "checking")));
    fs::remove_dir_all(base).ok();
}

/// A message the user sent mid-turn is committed with the resumable batch
/// boundary it follows, in one checkpoint. A run restored from it (an
/// iteration boundary right after, or a restart) must still resume, with the
/// batch's provider continuation (signatures/opaque state the provider needs)
/// and the message after the batch's results.
#[test]
fn a_mid_turn_user_block_keeps_the_batch_boundary_and_continuation_for_restore() {
    let (base, store) = store("steer-continuation");
    let continuation = crate::provider_runtime::ProviderContinuation {
        provider: ProviderId::OpencodeGo,
        data: serde_json::json!({"wire": "responses", "opaque": {"encrypted": "sig"}}),
    };
    let writer = store
        .checkpoint_writer(
            ProviderId::OpencodeGo,
            "session-a",
            0,
            TranscriptBranch::legacy(),
            Vec::new(),
        )
        .unwrap();
    writer.commit_context("request-1", "inspect", &[]).unwrap();
    let call = crate::provider_tool_loop::DirectToolCall {
        id: "call-1".to_string(),
        name: "read_file".to_string(),
        arguments: serde_json::json!({"path": "src/main.eps"}),
    };
    writer
        .begin_tool_batch(
            "response-1",
            "batch-1",
            &[crate::provider_runtime::NormalizedBlock::ToolCall {
                response_id: "response-1".to_string(),
                batch_id: "batch-1".to_string(),
                call: call.clone(),
                continuation: None,
            }],
        )
        .unwrap();
    writer
        .commit_provisional_tool_result(
            &call,
            &crate::provider_tool_loop::DirectToolResult {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                result: serde_json::json!({"text": "Trigger(...)"}),
                is_error: false,
            },
        )
        .unwrap();
    let revision = writer
        .mark_batch_resumable_with_messages(
            "response-1",
            "batch-1",
            Some(continuation.clone()),
            "request-1",
            &[(
                "[user message during this turn]\nalso a timer".to_string(),
                Vec::new(),
            )],
        )
        .unwrap();

    let restored = store
        .restore(
            ProviderId::OpencodeGo,
            "session-a",
            revision,
            &TranscriptBranch::legacy(),
        )
        .unwrap()
        .expect("the steered batch boundary is resumable");
    assert_eq!(
        restored.generation.checkpoint.continuation,
        Some(continuation)
    );
    let shape = restored
        .generation
        .checkpoint
        .blocks
        .iter()
        .map(|block| match block {
            TranscriptBlock::User { text, .. } if text.ends_with("also a timer") => "steer",
            TranscriptBlock::User { .. } => "user",
            TranscriptBlock::ToolCall { .. } => "call",
            TranscriptBlock::ToolResult { .. } => "result",
            _ => "other",
        })
        .collect::<Vec<_>>();
    assert_eq!(shape, vec!["user", "call", "result", "steer"]);
    fs::remove_dir_all(base).ok();
}
