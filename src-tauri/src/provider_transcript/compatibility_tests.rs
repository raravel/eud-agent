use super::*;

#[test]
fn schema_v1_fixture_reads_losslessly_without_rewriting_original_bytes() {
    let (base, store) = store("legacy-v1");
    let generation_bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "schemaVersion": 1,
        "provider": "antigravity",
        "sessionId": "session-a",
        "revision": 1,
        "entries": [
            {"type":"user","text":"inspect","images":[]},
            {"type":"assistant-reasoning","text":"checking"},
            {"type":"tool-call","id":"call-1","name":"read_file","arguments":{"path":"src/main.eps"},"thought_signature":"signed-state"},
            {"type":"tool-result","id":"call-1","name":"read_file","result":{"text":"Trigger(...)"},"is_error":false},
            {"type":"assistant-text","text":"done"}
        ]
    }))
    .unwrap();
    let pointer_bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "schemaVersion": 1,
        "provider": "antigravity",
        "sessionId": "session-a",
        "revision": 1,
        "sha256": hex_sha256(&generation_bytes)
    }))
    .unwrap();
    let session_dir = store.session_dir("session-a").unwrap();
    fs::create_dir_all(session_dir.join("generations")).unwrap();
    fs::write(session_dir.join("generations/1.json"), &generation_bytes).unwrap();
    fs::write(session_dir.join("current.json"), &pointer_bytes).unwrap();

    let loaded = store
        .load_current(ProviderId::Antigravity, "session-a")
        .unwrap();

    assert_eq!(loaded.schema_version, TRANSCRIPT_SCHEMA_VERSION);
    assert!(matches!(
        &loaded.checkpoint.blocks[2],
        TranscriptBlock::ToolCall {
            continuation: Some(continuation),
            ..
        } if continuation.data["thoughtSignature"] == "signed-state"
    ));
    assert_eq!(
        fs::read(session_dir.join("generations/1.json")).unwrap(),
        generation_bytes
    );
    assert_eq!(
        fs::read(session_dir.join("current.json")).unwrap(),
        pointer_bytes
    );
    fs::remove_dir_all(base).ok();
}

#[test]
fn stale_metadata_adopts_only_matching_branch_and_ahead_metadata_fails_closed() {
    let (base, store) = store("metadata-branch");
    let branch = TranscriptBranch {
        instruction_epoch: 7,
        task_leaf_id: Some("event-7".to_string()),
    };
    store
        .publish_checkpoint(
            ProviderId::Ollama,
            "session-a",
            0,
            TranscriptCheckpoint {
                branch: branch.clone(),
                blocks: vec![TranscriptBlock::AssistantText {
                    response_id: "response-1".to_string(),
                    text: "done".to_string(),
                }],
                boundary: CheckpointBoundary::ResponseCompleted {
                    response_id: "response-1".to_string(),
                },
                continuation: None,
            },
        )
        .unwrap();

    let stale = store
        .restore(ProviderId::Ollama, "session-a", 0, &branch)
        .unwrap()
        .unwrap();
    assert!(stale.metadata_was_stale);
    assert!(store
        .restore(
            ProviderId::Ollama,
            "session-a",
            1,
            &TranscriptBranch {
                instruction_epoch: 7,
                task_leaf_id: Some("compiler-event-8".to_string()),
            },
        )
        .is_ok());
    assert!(store
        .restore(
            ProviderId::Ollama,
            "session-a",
            1,
            &TranscriptBranch {
                instruction_epoch: 8,
                task_leaf_id: Some("event-8".to_string()),
            },
        )
        .is_err());
    assert!(store
        .restore(ProviderId::Ollama, "session-a", 2, &branch)
        .is_err());
    fs::remove_dir_all(base).ok();
}

#[test]
fn conversation_images_snapshot_bytes_with_stable_identity() {
    let (base, _store) = store("image-snapshot");
    fs::create_dir_all(&base).unwrap();
    let path = base.join("image.png");
    let bytes = b"\x89PNG\r\n\x1a\nfixture";
    fs::write(&path, bytes).unwrap();

    let images = conversation_images(&[path]).unwrap();

    assert_eq!(images.len(), 1);
    assert_eq!(images[0].mime, "image/png");
    assert_eq!(
        images[0].data_base64,
        base64::engine::general_purpose::STANDARD.encode(bytes)
    );
    assert_eq!(images[0].id.len(), 32);
    fs::remove_dir_all(base).ok();
}

#[test]
fn checkpoint_rejects_cross_provider_or_credential_continuation() {
    let (base, store) = store("continuation-authority");
    let checkpoint = |continuation| TranscriptCheckpoint {
        branch: TranscriptBranch {
            instruction_epoch: 1,
            task_leaf_id: None,
        },
        blocks: vec![TranscriptBlock::AssistantReasoning {
            response_id: "response-1".to_string(),
            text: "reasoning".to_string(),
            continuation: Some(continuation),
        }],
        boundary: CheckpointBoundary::ResponseCompleted {
            response_id: "response-1".to_string(),
        },
        continuation: None,
    };

    assert!(store
        .publish_checkpoint(
            ProviderId::Ollama,
            "session-a",
            0,
            checkpoint(crate::provider_runtime::ProviderContinuation {
                provider: ProviderId::Antigravity,
                data: serde_json::json!({"thoughtSignature":"signature"}),
            }),
        )
        .is_err());
    assert!(store
        .publish_checkpoint(
            ProviderId::Ollama,
            "session-a",
            0,
            checkpoint(crate::provider_runtime::ProviderContinuation {
                provider: ProviderId::Ollama,
                data: serde_json::json!({"authorization":"secret"}),
            }),
        )
        .is_err());
    assert!(!store.session_dir("session-a").unwrap().exists());
    fs::remove_dir_all(base).ok();
}
