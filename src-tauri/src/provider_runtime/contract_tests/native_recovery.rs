use super::*;
use crate::provider_tool_loop::{unresolved_native_runs, NativeRunReceiptState, RunGate};

struct NativeAdapter {
    started: Option<tokio::sync::oneshot::Sender<()>>,
    observed: Arc<parking_lot::Mutex<Vec<ProviderConversationState>>>,
    /// The resumable session a real native CLI publishes before any output.
    /// `None` reproduces an adapter that never learned one.
    session: Option<String>,
}

impl ProviderAdapter for NativeAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Codex
    }
    fn loop_kind(&self) -> AdapterLoopKind {
        AdapterLoopKind::NativeSession
    }
    fn run_step<'a>(
        &'a mut self,
        request: AdapterStepRequest,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<AdapterStepOutcome, ProviderRuntimeError>> {
        Box::pin(async move {
            self.observed.lock().push(request.binding.conversation);
            if let Some(started) = self.started.take() {
                started.send(()).expect("signal native transport entry");
                return pending().await;
            }
            for kind in [
                AdapterEventKind::ResponseStarted {
                    response_id: "native-response".to_string(),
                },
                AdapterEventKind::ResponseFinished {
                    response_id: "native-response".to_string(),
                    finish_reason: Some("stop".to_string()),
                    complete: true,
                },
            ] {
                events
                    .send(AdapterEvent {
                        identity: request.identity.clone(),
                        kind,
                    })
                    .await
                    .map_err(|_| ProviderRuntimeError::Transport("event receiver closed".into()))?;
            }
            Ok(AdapterStepOutcome::Completed {
                output: AdapterOutput::Text("native answer".to_string()),
                continuation: None,
                native_conversation: Some(ProviderConversationState::Codex {
                    thread_id: Some("confirmed-new-thread".to_string()),
                }),
            })
        })
    }
    fn interrupt(
        &mut self,
        _identity: &crate::provider_runtime::RunIdentity,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }
    fn reset(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }
    fn compact<'a>(
        &'a mut self,
        _identity: crate::provider_runtime::RunIdentity,
        _cancellation: tokio::sync::watch::Receiver<u64>,
        _events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<ProviderConversationState, ProviderRuntimeError>> {
        Box::pin(async {
            Err(ProviderRuntimeError::Protocol(
                "fixture does not compact".to_string(),
            ))
        })
    }
    fn seed(
        &mut self,
        _state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }
    fn observed_conversation(&self) -> Option<ProviderConversationState> {
        self.session
            .clone()
            .map(|thread_id| ProviderConversationState::Codex {
                thread_id: Some(thread_id),
            })
    }
}

fn binding(fixture: &RuntimeFixture) -> crate::provider_runtime::BindingSnapshot {
    let mut binding = fixture.binding("http://127.0.0.1:9/v1");
    binding.provider = ProviderId::Codex;
    binding.base_url = None;
    binding.conversation = ProviderConversationState::Codex {
        thread_id: Some("persisted-old-thread".to_string()),
    };
    binding
}

fn native_runtime(fixture: &RuntimeFixture, adapter: NativeAdapter) -> ProviderRuntime {
    ProviderRuntime::new(
        Box::new(adapter),
        binding(fixture),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        fixture.events.clone(),
    )
    .expect("construct controlled native runtime")
}

async fn unsafe_run_blocks_reconstruction_until_explicit_reset(drop_future: bool) {
    let fixture = RuntimeFixture::new("native-unsafe-reconstruction");
    let observed = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let (started, running) = tokio::sync::oneshot::channel();
    let mut first = native_runtime(
        &fixture,
        NativeAdapter {
            started: Some(started),
            observed: observed.clone(),
            session: None,
        },
    );
    let request = fixture.foreground(binding(&fixture), 40_001);
    if drop_future {
        let mut future = first.run_foreground(request);
        tokio::select! {
            _ = running => {}
            result = &mut future => panic!("native request completed before drop: {result:?}"),
        }
        drop(future);
    } else {
        let (outcome, ()) = tokio::join!(first.run_foreground(request), async {
            running.await.expect("native request started");
            fixture.cancellation.send(1).expect("cancel native request");
        });
        assert_eq!(outcome, RunOutcome::Cancelled);
    }
    drop(first);
    let receipts = unresolved_native_runs(&fixture.dirs.journal_dir(), &fixture.session_id)
        .expect("recover interrupted native receipt");
    assert_eq!(receipts.len(), 1);
    assert!(matches!(
        receipts[0].state,
        NativeRunReceiptState::Pending | NativeRunReceiptState::Unknown
    ));
    assert_eq!(
        receipts[0].prior_native_id.as_deref(),
        Some("persisted-old-thread")
    );
    let project_id = fixture.root.join("project").to_string_lossy().into_owned();
    fixture
        .tools
        .begin_request("native-after-restart", &project_id)
        .expect("begin recovered request");
    let mut recovered = native_runtime(
        &fixture,
        NativeAdapter {
            started: None,
            observed: observed.clone(),
            session: None,
        },
    );
    assert!(matches!(
        recovered.seed(binding(&fixture).conversation).await,
        Err(ProviderRuntimeError::Protocol(_))
    ));
    let mut retry = fixture.foreground(binding(&fixture), 40_002);
    retry.identity.request_id = "native-after-restart".to_string();
    retry.identity.cancellation_generation = *fixture.cancellation.borrow();
    assert!(matches!(
        recovered.run_foreground(retry).await,
        RunOutcome::Failed(ProviderRuntimeError::Protocol(_))
    ));
    assert_eq!(
        observed.lock().len(),
        1,
        "unsafe old thread reached reconstructed transport"
    );
    assert_eq!(
        recovered.conversation_state(),
        binding(&fixture).conversation
    );
    recovered
        .reset()
        .await
        .expect("explicitly reset unsafe conversation");
    fixture
        .tools
        .begin_request("native-after-explicit-reset", &project_id)
        .expect("begin fresh request");
    let mut fresh_binding = binding(&fixture);
    fresh_binding.conversation = recovered.conversation_state();
    let mut fresh = fixture.foreground(fresh_binding, 40_003);
    fresh.identity.request_id = "native-after-explicit-reset".to_string();
    fresh.identity.cancellation_generation = *fixture.cancellation.borrow();
    assert!(matches!(
        recovered.run_foreground(fresh).await,
        RunOutcome::Completed { .. }
    ));
    assert_eq!(
        observed.lock()[1],
        ProviderConversationState::Codex { thread_id: None }
    );
}

#[tokio::test]
async fn an_interrupted_run_that_named_its_session_is_resumed_not_refused() {
    // A native CLI publishes its resumable session before any output. A run
    // that is cancelled (or dies with the process) after that point did not
    // finish its turn, but the session it named is still the boundary the next
    // run continues from — losing it costs the whole conversation.
    let fixture = RuntimeFixture::new("native-interrupted-resume");
    let observed = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let (started, running) = tokio::sync::oneshot::channel();
    let mut first = native_runtime(
        &fixture,
        NativeAdapter {
            started: Some(started),
            observed: observed.clone(),
            session: Some("persisted-old-thread".to_string()),
        },
    );
    let request = fixture.foreground(binding(&fixture), 41_001);
    let (outcome, ()) = tokio::join!(first.run_foreground(request), async {
        running.await.expect("native request started");
        fixture.cancellation.send(1).expect("cancel native request");
    });
    assert_eq!(outcome, RunOutcome::Cancelled);
    drop(first);

    let receipts = unresolved_native_runs(&fixture.dirs.journal_dir(), &fixture.session_id)
        .expect("read the interrupted native receipt");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].state, NativeRunReceiptState::Interrupted);
    assert_eq!(
        receipts[0].candidate_native_id.as_deref(),
        Some("persisted-old-thread")
    );

    let project_id = fixture.root.join("project").to_string_lossy().into_owned();
    fixture
        .tools
        .begin_request("native-after-interrupt", &project_id)
        .expect("begin the request after the interruption");
    let mut recovered = native_runtime(
        &fixture,
        NativeAdapter {
            started: None,
            observed: observed.clone(),
            session: Some("persisted-old-thread".to_string()),
        },
    );
    let mut retry = fixture.foreground(binding(&fixture), 41_002);
    retry.identity.request_id = "native-after-interrupt".to_string();
    retry.identity.cancellation_generation = *fixture.cancellation.borrow();
    assert!(matches!(
        recovered.run_foreground(retry).await,
        RunOutcome::Completed { .. }
    ));
    assert_eq!(
        observed.lock()[1],
        ProviderConversationState::Codex {
            thread_id: Some("persisted-old-thread".to_string())
        },
        "the interrupted session must be the one the next run continues"
    );
}

#[tokio::test]
async fn c05_c14_cancelled_native_run_rejects_persisted_old_id_after_reconstruction() {
    unsafe_run_blocks_reconstruction_until_explicit_reset(false).await;
}

#[tokio::test]
async fn c14_dropped_native_run_rejects_persisted_old_id_after_reconstruction() {
    unsafe_run_blocks_reconstruction_until_explicit_reset(true).await;
}

#[tokio::test]
async fn c14_confirmed_native_checkpoint_is_recovered_and_retained_until_metadata_ack() {
    let fixture = RuntimeFixture::new("native-completed-reconstruction");
    let gate = RunGate::new(
        fixture.identity(40_011),
        fixture.tools.clone(),
        crate::provider_runtime::WorkspaceAccess::Read,
        None,
    );
    gate.begin_native_run(ProviderId::Codex, Some("persisted-old-thread"))
        .expect("persist native start");
    gate.mark_native_run_completed(Some("completed-before-crash"))
        .expect("persist confirmed native boundary");
    let original_path = gate.receipt_path().expect("native receipt path");
    let project_id = fixture.root.join("project").to_string_lossy().into_owned();
    fixture
        .tools
        .begin_request("native-recover-completed", &project_id)
        .expect("begin fresh request after crash");
    let observed = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let mut recovered = native_runtime(
        &fixture,
        NativeAdapter {
            started: None,
            observed: observed.clone(),
            session: None,
        },
    );
    let mut next = fixture.foreground(binding(&fixture), 40_012);
    next.identity.request_id = "native-recover-completed".to_string();
    assert!(matches!(
        recovered.run_foreground(next).await,
        RunOutcome::Completed { .. }
    ));
    assert_eq!(
        observed.lock()[0],
        ProviderConversationState::Codex {
            thread_id: Some("completed-before-crash".to_string())
        }
    );
    assert!(
        original_path.is_file(),
        "receipt removed before metadata acknowledgement"
    );
    recovered
        .acknowledge_persisted()
        .await
        .expect("acknowledge persisted native metadata");
    assert!(!original_path.exists());
    assert!(
        unresolved_native_runs(&fixture.dirs.journal_dir(), &fixture.session_id)
            .expect("read acknowledged receipts")
            .is_empty()
    );
}

#[tokio::test]
async fn c14_explicit_reset_archives_corrupt_claim_before_fresh_native_run() {
    let fixture = RuntimeFixture::new("native-corrupt-claim-reset");
    let corrupt_path = RunGate::new(
        fixture.identity(40_021),
        fixture.tools.clone(),
        crate::provider_runtime::WorkspaceAccess::Read,
        None,
    )
    .receipt_path()
    .expect("native receipt path");
    std::fs::create_dir_all(corrupt_path.parent().expect("receipt directory"))
        .expect("create receipt directory");
    let corrupt_bytes = b"{interrupted";
    std::fs::write(&corrupt_path, corrupt_bytes).expect("stage interrupted receipt claim");

    let mut other_identity = fixture.identity(40_022);
    other_identity.session_id = "native-other-session-corrupt-claim".to_string();
    let other_path = RunGate::new(
        other_identity,
        fixture.tools.clone(),
        crate::provider_runtime::WorkspaceAccess::Read,
        None,
    )
    .receipt_path()
    .expect("other native receipt path");
    std::fs::create_dir_all(other_path.parent().expect("other receipt directory"))
        .expect("create other receipt directory");
    std::fs::write(&other_path, b"other-session").expect("stage other session receipt");

    let observed = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let mut runtime = native_runtime(
        &fixture,
        NativeAdapter {
            started: None,
            observed: observed.clone(),
            session: None,
        },
    );
    assert!(matches!(
        runtime.seed(binding(&fixture).conversation).await,
        Err(ProviderRuntimeError::Transport(_))
    ));

    runtime
        .reset()
        .await
        .expect("explicitly reset corrupt native recovery");
    assert!(!corrupt_path.exists());
    assert_eq!(std::fs::read(&other_path).unwrap(), b"other-session");
    let audit_root = fixture
        .dirs
        .journal_dir()
        .join("provider-run-receipt-audit");
    let session_audit = std::fs::read_dir(audit_root)
        .expect("read receipt audit root")
        .next()
        .expect("session audit directory")
        .expect("read session audit entry")
        .path();
    let archived = std::fs::read_dir(session_audit)
        .expect("read session receipt audit")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect receipt audit");
    assert_eq!(archived.len(), 1);
    assert_eq!(std::fs::read(archived[0].path()).unwrap(), corrupt_bytes);

    let project_id = fixture.root.join("project").to_string_lossy().into_owned();
    fixture
        .tools
        .begin_request("native-after-corrupt-reset", &project_id)
        .expect("begin fresh request after corrupt reset");
    let mut fresh_binding = binding(&fixture);
    fresh_binding.conversation = runtime.conversation_state();
    let mut fresh = fixture.foreground(fresh_binding, 40_023);
    fresh.identity.request_id = "native-after-corrupt-reset".to_string();
    assert!(matches!(
        runtime.run_foreground(fresh).await,
        RunOutcome::Completed { .. }
    ));
    assert_eq!(
        observed.lock().as_slice(),
        [ProviderConversationState::Codex { thread_id: None }]
    );
}

#[tokio::test]
async fn native_round_exhaustion_fails_closed_without_synthesizing_a_checkpoint() {
    let fixture = RuntimeFixture::new("native-round-boundary");
    let observed = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let initial = binding(&fixture).conversation;
    let mut runtime = native_runtime(
        &fixture,
        NativeAdapter {
            started: None,
            observed: observed.clone(),
            session: None,
        },
    );
    let mut request = fixture.foreground(binding(&fixture), 40_024);
    request.policy.max_tool_rounds = 0;

    let outcome = runtime.run_foreground(request).await;

    assert_eq!(
        outcome,
        RunOutcome::Failed(ProviderRuntimeError::IterationBoundaryNotResumable)
    );
    assert_eq!(runtime.conversation_state(), initial);
    assert!(
        observed.lock().is_empty(),
        "native adapter must not run when no confirmed boundary can be obtained"
    );
}
