use std::{fs, time::Duration};

use crate::{
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{ProviderRuntimeError, RunOutcome, RuntimeExecutor},
    provider_tool_loop::{DurableToolCompletion, RunGate},
};

use super::{setup, wait_for_process_exit};

#[tokio::test]
async fn c14_production_codex_corrupt_receipt_reset_starts_fresh_mcp_process() {
    // Given: a claimed current-session receipt interrupted before its JSON publication.
    let (fixture, binding, mut runtime, events) =
        setup(ProviderId::Codex, "native-receipt-reset-process");
    let corrupt_identity = fixture.identity(56_001);
    let corrupt_path = RunGate::new(
        corrupt_identity,
        fixture.tools.clone(),
        crate::provider_runtime::WorkspaceAccess::Read,
        None,
    )
    .receipt_path()
    .expect("derive canonical corrupt receipt path");
    fs::create_dir_all(corrupt_path.parent().expect("corrupt receipt parent"))
        .expect("create corrupt receipt parent");
    fs::File::create(&corrupt_path).expect("create zero-byte interrupted receipt claim");

    let mut other_identity = fixture.identity(56_002);
    other_identity.session_id = "other-native-receipt-session".to_string();
    let other_path = RunGate::new(
        other_identity,
        fixture.tools.clone(),
        crate::provider_runtime::WorkspaceAccess::Read,
        None,
    )
    .receipt_path()
    .expect("derive other-session receipt path");
    fs::create_dir_all(other_path.parent().expect("other receipt parent"))
        .expect("create other receipt parent");
    let other_bytes = b"other-session-receipt";
    fs::write(&other_path, other_bytes).expect("write other-session receipt");

    let persisted_old_thread = ProviderConversationState::Codex {
        thread_id: Some("interrupted-old-codex-thread".to_string()),
    };

    // When: reconstruction encounters the zero-byte receipt, then the user explicitly resets it.
    let seed = runtime.seed(persisted_old_thread.clone()).await;
    assert!(
        matches!(&seed, Err(ProviderRuntimeError::Transport(_))),
        "corrupt receipt must refuse automatic native reconstruction: {seed:?}"
    );
    assert_eq!(
        runtime.conversation_state(),
        binding.conversation,
        "a rejected reconstruction must retain the pre-seed runtime conversation"
    );
    assert_eq!(
        fs::read(&corrupt_path).expect("read unreset corrupt receipt"),
        Vec::<u8>::new(),
        "automatic recovery must preserve the interrupted claim for explicit review"
    );

    runtime
        .reset()
        .await
        .expect("explicit reset must archive a corrupt current-session receipt");

    // Then: reset retains only audit evidence and a real fresh Codex process completes MCP tools once.
    assert!(
        !corrupt_path.exists(),
        "the corrupt receipt must leave the active session directory after reset"
    );
    assert_eq!(
        fs::read(&other_path).expect("read untouched other-session receipt"),
        other_bytes,
        "reset must not inspect or alter another session's receipt"
    );
    let audit_root = fixture
        .dirs
        .journal_dir()
        .join("provider-run-receipt-audit");
    let audit_sessions = fs::read_dir(&audit_root)
        .expect("read corrupt receipt audit root")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect corrupt receipt audit sessions");
    assert_eq!(
        audit_sessions.len(),
        1,
        "one session audit directory is required"
    );
    let archived = fs::read_dir(audit_sessions[0].path())
        .expect("read current session corrupt receipt audit")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect archived corrupt receipt");
    assert_eq!(archived.len(), 1, "the exact claim must be archived once");
    assert!(
        archived[0].file_name().to_string_lossy().starts_with(
            corrupt_path
                .file_name()
                .expect("corrupt receipt name")
                .to_string_lossy()
                .as_ref()
        ),
        "audit entry must retain the canonical claimed receipt name"
    );
    assert_eq!(
        fs::read(archived[0].path()).expect("read archived zero-byte receipt"),
        Vec::<u8>::new(),
        "the audit entry must retain the exact interrupted bytes"
    );

    let fresh_run_id = 56_003;
    let mut fresh_binding = binding;
    fresh_binding.conversation = runtime.conversation_state();
    let mut fresh = fixture.foreground(fresh_binding, fresh_run_id);
    fresh.policy.active_deadline = Some(Duration::from_secs(20));
    let outcome = runtime.run_foreground(fresh).await;
    let fresh_thread = match outcome {
        RunOutcome::Completed {
            text,
            conversation:
                ProviderConversationState::Codex {
                    thread_id: Some(thread_id),
                },
            ..
        } => {
            assert!(text.contains("onPluginStart"), "{text}");
            thread_id
        }
        other => panic!("fresh production Codex foreground run failed: {other:?}"),
    };
    assert_ne!(
        fresh_thread,
        persisted_old_thread
            .conversation_key()
            .expect("persisted old Codex thread"),
        "the post-reset process must start a new native thread rather than resume the refused one"
    );

    let fresh_path = RunGate::new(
        fixture.identity(fresh_run_id),
        fixture.tools.clone(),
        crate::provider_runtime::WorkspaceAccess::Read,
        None,
    )
    .receipt_path()
    .expect("derive fresh receipt path");
    let fresh_receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&fresh_path).expect("read fresh receipt"))
            .expect("parse fresh receipt");
    let completions: Vec<DurableToolCompletion> =
        serde_json::from_value(fresh_receipt["completions"].clone())
            .expect("parse fresh durable completions");
    assert_eq!(
        completions
            .iter()
            .map(|completion| completion.name.as_str())
            .collect::<Vec<_>>(),
        ["list_files", "read_file"],
        "the fresh process must record each real MCP tool exactly once without old-run replay"
    );
    assert!(
        completions.iter().all(|completion| !completion.is_error),
        "the fresh MCP calls must both succeed"
    );
    assert!(
        events.observations.lock().is_empty(),
        "native tool observations must not replay the authoritative MCP completions"
    );

    runtime
        .acknowledge_persisted()
        .await
        .expect("acknowledge fresh native receipt after persistence");
    assert!(
        !fresh_path.exists(),
        "only the acknowledged fresh receipt may be retired; the corrupt audit stays retained"
    );
    assert_eq!(
        fs::read(archived[0].path())
            .expect("read retained corrupt audit after fresh acknowledgement"),
        Vec::<u8>::new(),
        "fresh receipt acknowledgement must not remove corrupt audit evidence"
    );
    drop(runtime);
    wait_for_process_exit(&fixture.root).await;
}
