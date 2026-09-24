use super::*;
use crate::{provider_runtime::RunId, session::SessionKind};

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("native-receipt-test-{}", uuid::Uuid::new_v4())))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        if self.0.exists() {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

fn identity() -> RunIdentity {
    RunIdentity {
        session_id: "session".to_string(),
        run_id: RunId::new(1),
        request_id: "request".to_string(),
        session_kind: SessionKind::Eps,
        cancellation_generation: 1,
    }
}

#[test]
fn identical_identity_after_restart_cannot_overwrite_receipt() {
    // Given: the exact process-local identity is reused after a restart.
    let root = TestRoot::new();
    let previous = RunReceiptStore::new(root.path().to_path_buf(), &identity());
    previous
        .begin_native(ProviderId::Codex, Some("old-thread"))
        .unwrap();
    let original = fs::read(previous.path()).unwrap();
    let restarted = RunReceiptStore::new(root.path().to_path_buf(), &identity());

    // When: the new store tries to start a native run with the same receipt key.
    let result = restarted.begin_native(ProviderId::Codex, Some("new-thread"));

    // Then: the original recovery evidence is preserved byte for byte.
    assert!(result.is_err());
    assert!(restarted.acknowledge().is_err());
    assert_eq!(fs::read(previous.path()).unwrap(), original);
}

#[test]
fn pending_receipt_survives_owner_drop_without_any_tool_execution() {
    // Given: native execution is marked before invoking the adapter.
    let root = TestRoot::new();
    let store = RunReceiptStore::new(root.path().to_path_buf(), &identity());
    store
        .begin_native(ProviderId::Codex, Some("thread"))
        .unwrap();

    // When: the execution owner disappears before reporting an outcome.
    drop(store);

    // Then: recovery still identifies the unsafe native continuation.
    let pending = unresolved_native_runs(root.path(), "session").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].state, NativeRunReceiptState::Pending);
    assert_eq!(pending[0].prior_native_id.as_deref(), Some("thread"));
    assert_eq!(pending[0].identity, identity());
}

#[test]
fn unknown_receipt_preserves_tool_completion_during_recovery() {
    // Given: a tool finished before native transport became uncertain.
    let root = TestRoot::new();
    let store = RunReceiptStore::new(root.path().to_path_buf(), &identity());
    store
        .begin_native(ProviderId::Codex, Some("thread"))
        .unwrap();
    let completion = completion();
    store.persist(std::slice::from_ref(&completion)).unwrap();

    // When: failure marks the native outcome unknown.
    store.mark_unknown().unwrap();

    // Then: both uncertainty and the actual result survive disk recovery.
    let pending = unresolved_native_runs(root.path(), "session").unwrap();
    assert_eq!(pending[0].state, NativeRunReceiptState::Unknown);
    assert_eq!(
        read_receipt(&store.path()).unwrap().completions,
        vec![completion]
    );
}

#[test]
fn completed_candidate_survives_until_explicit_checkpoint_acknowledgement() {
    // Given: the adapter reported a confirmed native boundary.
    let root = TestRoot::new();
    let store = RunReceiptStore::new(root.path().to_path_buf(), &identity());
    store.begin_native(ProviderId::ClaudeCode, None).unwrap();
    store.mark_completed(Some("confirmed-session")).unwrap();
    let pending = unresolved_native_runs(root.path(), "session").unwrap();
    assert_eq!(pending[0].state, NativeRunReceiptState::Completed);
    assert_eq!(
        pending[0].candidate_native_id.as_deref(),
        Some("confirmed-session")
    );

    // When: the engine acknowledges its persisted checkpoint.
    acknowledge_native_run(root.path(), &pending[0].identity).unwrap();

    // Then: recovery no longer offers that already adopted candidate.
    assert!(unresolved_native_runs(root.path(), "session")
        .unwrap()
        .is_empty());
}

#[test]
fn explicit_reset_clears_recovery_but_preserves_tool_audit() {
    // Given: an unresolved native run contains an actual tool result.
    let root = TestRoot::new();
    let store = RunReceiptStore::new(root.path().to_path_buf(), &identity());
    store
        .begin_native(ProviderId::Codex, Some("thread"))
        .unwrap();
    let completion = completion();
    store.persist(std::slice::from_ref(&completion)).unwrap();

    // When: the user explicitly resets that native continuation.
    clear_native_run_recovery(root.path(), "session", ProviderId::Codex, Some("thread")).unwrap();

    // Then: reset suppresses reuse while retaining audit evidence.
    assert!(unresolved_native_runs(root.path(), "session")
        .unwrap()
        .is_empty());
    let receipt = read_receipt(&store.path()).unwrap();
    assert_eq!(receipt.lifecycle, Some(NativeRunReceiptState::Cleared));
    assert_eq!(receipt.completions, vec![completion]);
}

#[test]
fn explicit_reset_archives_empty_claim_and_allows_fresh_owner() {
    // Given: process interruption left only the final-path ownership claim.
    let root = TestRoot::new();
    let interrupted = RunReceiptStore::new(root.path().to_path_buf(), &identity());
    let path = interrupted.path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::File::create(&path).unwrap();
    assert!(unresolved_native_runs(root.path(), "session").is_err());

    // When: the user explicitly resets this session's native recovery.
    clear_native_run_recovery(root.path(), "session", ProviderId::Codex, Some("thread")).unwrap();

    // Then: corrupt evidence is retained outside admission and the identity can be claimed anew.
    assert!(!path.exists());
    let archived = corrupt_receipt_audit_directory(root.path(), "session")
        .read_dir()
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(archived.len(), 1);
    let original_name = path.file_name().unwrap().to_string_lossy();
    assert!(archived[0]
        .file_name()
        .to_string_lossy()
        .starts_with(original_name.as_ref()));
    assert_eq!(fs::read(archived[0].path()).unwrap(), Vec::<u8>::new());
    assert!(unresolved_native_runs(root.path(), "session")
        .unwrap()
        .is_empty());

    let fresh = RunReceiptStore::new(root.path().to_path_buf(), &identity());
    fresh.begin_native(ProviderId::Codex, None).unwrap();
}

#[test]
fn explicit_reset_does_not_archive_another_sessions_corrupt_receipt() {
    let root = TestRoot::new();
    let mut other = identity();
    other.session_id = "other-session".to_string();
    let other_store = RunReceiptStore::new(root.path().to_path_buf(), &other);
    let other_path = other_store.path();
    fs::create_dir_all(other_path.parent().unwrap()).unwrap();
    fs::write(&other_path, b"truncated").unwrap();

    clear_native_run_recovery(root.path(), "session", ProviderId::Codex, None).unwrap();

    assert_eq!(fs::read(other_path).unwrap(), b"truncated");
}

fn completion() -> DurableToolCompletion {
    DurableToolCompletion {
        sequence: 1,
        run_id: identity().run_id,
        request_id: identity().request_id,
        call_id: Some("call".to_string()),
        name: "write_file".to_string(),
        result: serde_json::json!({"written": true}),
        is_error: false,
    }
}

#[test]
fn long_map_run_of_rendered_images_stays_within_the_receipt_bound() {
    // Given: a Map run renders the candidate dozens of times; each render is a
    // multi-megabyte MCP image block the model must see in full.
    let root = TestRoot::new();
    let store = RunReceiptStore::new(root.path().to_path_buf(), &identity());
    store
        .begin_native(ProviderId::ClaudeCode, Some("thread"))
        .unwrap();
    let data = "A".repeat(2 * 1024 * 1024);
    let completions: Vec<DurableToolCompletion> = (1..=40)
        .map(|sequence| {
            let call = crate::provider_tool_loop::DirectToolCall {
                id: format!("render-{sequence}"),
                name: "map_draft_render".to_string(),
                arguments: serde_json::json!({}),
            };
            let result = crate::provider_tool_loop::DirectToolResult {
                id: call.id.clone(),
                name: call.name.clone(),
                result: serde_json::json!({
                    "image": {
                        "mimeType": "image/png",
                        "width": 1024,
                        "height": 1024,
                        "data": data,
                    }
                }),
                is_error: false,
            };
            crate::provider_tool_loop::completion::durable_completion(
                sequence,
                &identity(),
                &call,
                &result,
            )
        })
        .collect();

    // When: every completion is recorded on the receipt (80 MiB of raw image data).
    store.persist(&completions).unwrap();

    // Then: the receipt keeps the image identity, not its bytes, and stays bounded.
    let receipt = read_receipt(&store.path()).unwrap();
    assert_eq!(receipt.completions.len(), 40);
    let image = &receipt.completions[39].result["image"];
    assert_eq!(image["mimeType"], "image/png");
    assert_eq!(image["width"], 1024);
    assert_eq!(image["height"], 1024);
    assert!(image.get("data").is_none());
    assert_eq!(image["dataBytes"], 2 * 1024 * 1024);
    assert_eq!(image["dataSha256"].as_str().unwrap().len(), 64);
    assert!(fs::metadata(store.path()).unwrap().len() < 1024 * 1024);
}

#[test]
fn oversized_text_result_is_digested_on_the_receipt_and_small_results_stay_exact() {
    let call = crate::provider_tool_loop::DirectToolCall {
        id: "call".to_string(),
        name: "file_read".to_string(),
        arguments: serde_json::json!({}),
    };
    let small = crate::provider_tool_loop::DirectToolResult {
        id: call.id.clone(),
        name: call.name.clone(),
        result: serde_json::json!({"content": "short"}),
        is_error: false,
    };
    assert_eq!(
        crate::provider_tool_loop::completion::durable_completion(1, &identity(), &call, &small)
            .result,
        serde_json::json!({"content": "short"})
    );

    let large = crate::provider_tool_loop::DirectToolResult {
        id: call.id.clone(),
        name: call.name.clone(),
        result: serde_json::json!({"content": "B".repeat(200 * 1024)}),
        is_error: false,
    };
    let bounded =
        crate::provider_tool_loop::completion::durable_completion(2, &identity(), &call, &large)
            .result;
    assert_eq!(bounded["receiptOmitted"], true);
    assert_eq!(bounded["sha256"].as_str().unwrap().len(), 64);
    assert!(bounded["bytes"].as_u64().unwrap() > 200 * 1024);
    assert!(serde_json::to_vec(&bounded).unwrap().len() < 4096);
}
