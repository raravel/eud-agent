use super::*;

#[test]
fn write_transition_replays_completed_prefix_and_keeps_unexecuted_audit_tail() {
    let (base, store) = store("write-transition");
    let branch = TranscriptBranch {
        instruction_epoch: 4,
        task_leaf_id: None,
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
    writer
        .commit_context("request-1", "change it", &[])
        .unwrap();
    let write_call = crate::provider_tool_loop::DirectToolCall {
        id: "call-write".to_string(),
        name: crate::tools::REQUEST_WRITE_WORKSPACE_TOOL.to_string(),
        arguments: serde_json::json!({"reason":"edit"}),
    };
    writer
        .begin_tool_batch(
            "response-1",
            "batch-1",
            &[
                crate::provider_runtime::NormalizedBlock::Text {
                    response_id: "response-1".to_string(),
                    text: "I will edit".to_string(),
                },
                crate::provider_runtime::NormalizedBlock::Reasoning {
                    response_id: "response-1".to_string(),
                    text: "the edit needs write access".to_string(),
                    continuation: Some(crate::provider_runtime::ProviderContinuation {
                        provider: ProviderId::OpencodeGo,
                        data: serde_json::json!({"reasoningSignature":"signed-reasoning"}),
                    }),
                },
                crate::provider_runtime::NormalizedBlock::ToolCall {
                    response_id: "response-1".to_string(),
                    batch_id: "batch-1".to_string(),
                    call: write_call.clone(),
                    continuation: Some(crate::provider_runtime::ProviderContinuation {
                        provider: ProviderId::OpencodeGo,
                        data: serde_json::json!({"callSignature":"signed-call"}),
                    }),
                },
                crate::provider_runtime::NormalizedBlock::ToolCall {
                    response_id: "response-1".to_string(),
                    batch_id: "batch-1".to_string(),
                    call: crate::provider_tool_loop::DirectToolCall {
                        id: "call-after".to_string(),
                        name: "read_file".to_string(),
                        arguments: serde_json::json!({"path":"src/main.eps"}),
                    },
                    continuation: None,
                },
            ],
        )
        .unwrap();
    writer
        .commit_provisional_tool_result(
            &write_call,
            &crate::provider_tool_loop::DirectToolResult {
                id: "call-write".to_string(),
                name: crate::tools::REQUEST_WRITE_WORKSPACE_TOOL.to_string(),
                result: serde_json::json!({"granted":true}),
                is_error: false,
            },
        )
        .unwrap();
    writer
        .mark_write_transition_resumable(
            "response-1",
            "batch-1",
            &["call-write".to_string()],
            Some(crate::provider_runtime::ProviderContinuation {
                provider: ProviderId::OpencodeGo,
                data: serde_json::json!({"responseId":"remote-1"}),
            }),
        )
        .unwrap();

    let restored = store
        .restore(
            ProviderId::OpencodeGo,
            "session-a",
            writer.revision(),
            &branch,
        )
        .unwrap()
        .unwrap();

    assert!(restored.generation.checkpoint.boundary.is_resumable());
    assert!(restored
        .generation
        .checkpoint
        .blocks
        .iter()
        .any(|block| matches!(block, TranscriptBlock::ToolCall { id, .. } if id == "call-after")));
    assert!(restored.generation.checkpoint.continuation.is_none());
    let history = writer.history();
    assert_eq!(history.len(), 5);
    assert!(matches!(
        &history[0],
        crate::provider_runtime::ConversationItem::User { text, .. } if text == "change it"
    ));
    assert!(matches!(
        &history[1],
        crate::provider_runtime::ConversationItem::Assistant(
            crate::provider_runtime::NormalizedBlock::Text { text, .. }
        ) if text == "I will edit"
    ));
    assert!(matches!(
        &history[2],
        crate::provider_runtime::ConversationItem::Assistant(
            crate::provider_runtime::NormalizedBlock::Reasoning {
                text,
                continuation: Some(continuation),
                ..
            }
        ) if text == "the edit needs write access"
            && continuation.data["reasoningSignature"] == "signed-reasoning"
    ));
    assert!(matches!(
        &history[3],
        crate::provider_runtime::ConversationItem::Assistant(
            crate::provider_runtime::NormalizedBlock::ToolCall {
                call,
                continuation: Some(continuation),
                ..
            }
        ) if call.id == "call-write"
            && continuation.data["callSignature"] == "signed-call"
    ));
    assert!(matches!(
        &history[4],
        crate::provider_runtime::ConversationItem::Assistant(
            crate::provider_runtime::NormalizedBlock::ToolResult { result, .. }
        ) if result.id == "call-write"
    ));
    assert!(!history.iter().any(|item| matches!(
        item,
        crate::provider_runtime::ConversationItem::Assistant(
            crate::provider_runtime::NormalizedBlock::ToolCall { call, .. }
        ) if call.id == "call-after"
    )));
    fs::remove_dir_all(base).ok();
}
