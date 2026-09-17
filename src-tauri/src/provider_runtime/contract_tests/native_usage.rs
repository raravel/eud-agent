use std::sync::Arc;

use parking_lot::Mutex;

use crate::{
    codex_adapter::CodexAdapter,
    codex_client::CodexLaunchConfig,
    engine::{runtime_events::SessionRuntimeEventSink, AgentEngineError, EngineEvent, EventSink},
    provider::{ProviderBinding, ProviderConversationState, ProviderId},
    provider_runtime::{ProviderRuntime, RunOutcome, RuntimeExecutor},
    session::{SessionMeta, SessionRecord, SessionStore},
};

use super::fixtures::RuntimeFixture;

#[derive(Clone, Default)]
struct ProductionEventCapture {
    events: Arc<Mutex<Vec<EngineEvent>>>,
}

impl EventSink for ProductionEventCapture {
    fn emit(&self, event: EngineEvent) -> Result<(), AgentEngineError> {
        self.events.lock().push(event);
        Ok(())
    }
}

fn save_session(fixture: &RuntimeFixture, binding: &ProviderBinding) -> SessionStore {
    let store = SessionStore::new(&fixture.dirs);
    store
        .save(&SessionRecord {
            meta: SessionMeta {
                id: fixture.session_id.clone(),
                name: "native usage regression".to_string(),
                project: "Runtime Fixture".to_string(),
                kind: crate::session::SessionKind::Eps,
                provider: ProviderId::Codex,
                model: binding.model.clone(),
                created_at: 1,
                last_conversation_at: 1,
            },
            provider_binding: binding.clone(),
            pending_request_ids: Vec::new(),
            context_usage: None,
            panel_log: serde_json::Value::Null,
            context_state: Default::default(),
            task_state: Default::default(),
            autonomous_run: None,
        })
        .expect("save session consumed by production usage sink");
    store
}

#[tokio::test]
async fn c01_codex_resumed_turn_correlates_usage_before_persisting_to_the_session() {
    // Given: a production Codex process resumes a saved thread, publishes an old-turn
    // baseline before the new turn starts, and repeats stale usage after the new start.
    let fixture = RuntimeFixture::new("native-usage-correlation");
    let script = fixture.root.join("native-usage-fixture.ps1");
    std::fs::write(&script, include_str!("native_usage_fixture.ps1"))
        .expect("write native usage fixture");
    let mut binding = ProviderBinding::new(ProviderId::Codex, "fixture-model".to_string(), None)
        .expect("valid Codex binding");
    binding.conversation = ProviderConversationState::Codex {
        thread_id: Some("authoritative-native-thread".to_string()),
    };
    let snapshot = crate::provider_runtime::BindingSnapshot::from_binding(&binding, None)
        .expect("runtime binding snapshot");
    let store = save_session(&fixture, &binding);
    let capture = ProductionEventCapture::default();
    let session_sink = Arc::new(SessionRuntimeEventSink::new(
        capture.clone(),
        store.clone(),
        fixture.session_id.clone(),
    ));
    let adapter = CodexAdapter::new(
        fixture.root.clone(),
        CodexLaunchConfig {
            executable: which::which("powershell.exe").expect("PowerShell fixture executable"),
            executable_args: [
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ]
            .into_iter()
            .map(Into::into)
            .chain([script.into_os_string(), "codex".into()])
            .collect(),
            profile_dir: fixture.root.join("profile"),
            large_context_models: Default::default(),
        },
    );
    let mut runtime = ProviderRuntime::new(
        Box::new(adapter),
        snapshot.clone(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        session_sink,
    )
    .expect("construct production runtime with production session sink");
    runtime
        .seed(snapshot.conversation.clone())
        .await
        .expect("seed the persisted native conversation into the production adapter");
    let mut request = fixture.foreground(snapshot, 57_001);
    request.turn.text = "usage correlation".to_string();

    // When: the resumed native turn runs through the real adapter and session sink.
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        runtime.run_foreground(request),
    )
    .await
    .expect("native usage fixture must complete within its bounded deadline");

    // Then: stale usage cannot fail or mutate the current request; matching usage is
    // emitted and persisted exactly once under the current request identifier.
    assert!(
        matches!(outcome, RunOutcome::Completed { ref text, .. } if text == "usage-correlated answer"),
        "{outcome:?}"
    );
    let usages = capture
        .events
        .lock()
        .iter()
        .filter_map(|event| match event {
            EngineEvent::ContextUsage(usage) => Some(usage.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        usages.len(),
        1,
        "only usage owned by the active native turn is publishable"
    );
    assert_eq!(usages[0].turn_id, fixture.request_id);
    assert_eq!(usages[0].token_usage.total.total_tokens, 301);
    let persisted = store
        .load(&fixture.session_id)
        .expect("reload persisted session");
    assert_eq!(
        persisted
            .context_usage
            .expect("matching current usage must persist")
            .total
            .total_tokens,
        301
    );
}
