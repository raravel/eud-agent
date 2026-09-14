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
