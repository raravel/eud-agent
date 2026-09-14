use serde_json::{json, Value};

use crate::{
    engine::harness_execution_contract,
    harness::{
        self, HarnessDelta, HarnessJob, HarnessJobStatus, HarnessJobStore, HarnessProviderBinding,
    },
    provider::{ProviderId, ReasoningSelection},
    provider_runtime::{production_adapter, RunOutcome, RuntimeExecutor, StructuredJobExecutor},
    workspace::WorkspaceManager,
};

use super::{
    fixtures::{request_body, sse, transcript_root, HttpFixture, RuntimeFixture},
    runtime,
};

fn answer(text: &str) -> String {
    let frame = json!({"id":"harness-fixture","choices":[{"delta":{"content":text},"finish_reason":"stop"}]});
    sse(&format!("data: {frame}\n\ndata: [DONE]\n\n"))
}

async fn generate(fixture: &RuntimeFixture, job: &mut HarnessJob) -> Result<HarnessDelta, String> {
    let prompt = harness::generation_prompt(job, &fixture.dirs)?;
    job.status = HarnessJobStatus::Running;
    job.attempts += 1;
    HarnessJobStore::new(fixture.dirs.clone())
        .save(job)
        .expect("persist running attempt");
    let workspace = WorkspaceManager::new(fixture.dirs.clone())
        .prepare_document_session(&job.workspace_id, &job.project, &job.workspace_session_id)
        .expect("prepare actual document workspace");
    let (binding, request) = harness_execution_contract(job, prompt, workspace.root.clone(), 0)
        .expect("construct production harness request");
    let base = request.base.clone();
    let adapter = production_adapter(&binding, &fixture.dirs, workspace.root)
        .expect("construct production adapter from persisted binding");
    let mut executor = StructuredJobExecutor::new(adapter, fixture.cancellation.subscribe());
    match executor.run(request).await {
        RunOutcome::Structured {
            value,
            base: returned,
        } => {
            assert_eq!(returned, base);
            harness::parse_delta(&value.to_string())
        }
        RunOutcome::Failed(error) => Err(error.to_string()),
        other => panic!("unexpected harness outcome: {other:?}"),
    }
}

fn assert_wire(request: &str) -> Value {
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    let body = request_body(request);
    assert_eq!(body["model"], "fixture-model");
    assert_eq!(body["reasoning_effort"], "high");
    assert!(body.get("tools").is_none());
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert!(!body.to_string().contains("main-kept"));
    body
}

#[tokio::test]
async fn c16_actual_harness_retry_pins_binding_and_only_stages_validated_delta() {
    // Given: a durable main checkpoint and a harness attempt failing through production HTTP.
    let delta = json!({
        "summary":"Document accepted behavior",
        "documents":[{"path":"specs/game.md","create":null,"edits":[{
            "old_text":"Old behavior.","new_text":"Accepted behavior."
        }]}],
        "memoryUpdates":[],"promotedFactIds":[]
    });
    let mut unapproved = delta.clone();
    unapproved["promotedFactIds"] = json!(["unapproved-fact"]);
    let http = HttpFixture::scripted([
        answer("main-kept"),
        sse("data: {\"error\":{\"message\":\"controlled harness failure\"}}\n\n"),
        answer(&delta.to_string()),
        answer(&unapproved.to_string()),
    ]);
    let fixture = RuntimeFixture::new("c16-harness-retry");
    let binding = fixture.binding(&http.base_url);
    let mut main = runtime(&fixture, binding.clone());
    assert!(
        matches!(main.run_foreground(fixture.foreground(binding.clone(), 60_001)).await,
        RunOutcome::Completed { ref text, .. } if text == "main-kept")
    );
    let _ = http.requests.recv().expect("main HTTP request");
    let manager = WorkspaceManager::new(fixture.dirs.clone());
    let canonical = manager
        .prepare_current()
        .expect("prepare canonical documents");
    let document = canonical.root.join("specs/game.md");
    std::fs::write(&document, "# Gameplay\n\nOld behavior.\n").expect("seed canonical document");
    let original_document = std::fs::read(&document).expect("read canonical document");
    let source_path = fixture.root.join("project/src/main.eps");
    let source = std::fs::read(&source_path).expect("read source");
    let pointer = transcript_root(&fixture.dirs, &fixture.session_id).join("current.json");
    let checkpoint = std::fs::read(&pointer).expect("read main checkpoint");
    let conversation = main.conversation_state();
    let workspace = main.current_workspace();
    let project = fixture.tools.current_project_id();
    let events = fixture.events.snapshot();
    let assert_main = || {
        assert_eq!(main.conversation_state(), conversation);
        assert_eq!(main.current_workspace(), workspace);
        assert_eq!(fixture.tools.current_project_id(), project);
        assert_eq!(fixture.events.snapshot(), events);
        assert_eq!(
            std::fs::read(&pointer).expect("main pointer remains"),
            checkpoint
        );
        assert_eq!(std::fs::read(&source_path).expect("source remains"), source);
        assert_eq!(
            std::fs::read(&document).expect("canonical document remains"),
            original_document
        );
    };
    let mut job = HarnessJob::new_with_provider(
        fixture.session_id.clone(),
        HarnessProviderBinding {
            provider: binding.provider,
            model: binding.model,
            reasoning: binding.reasoning,
            base_url: binding.base_url,
        },
        canonical.project.clone(),
        canonical.id.clone(),
        "req-c16-source".to_string(),
        "Document the accepted change".to_string(),
        None,
        "Change accepted".to_string(),
        Vec::new(),
        None,
    );
    let store = HarnessJobStore::new(fixture.dirs.clone());
    store.create(&job).expect("persist new job");
    let error = generate(&fixture, &mut job)
        .await
        .expect_err("initial harness attempt fails");
    assert!(error.contains("provider_protocol_changed"));
    job.fail(error);
    store.save(&job).expect("persist failed harness attempt");
    assert_wire(&http.requests.recv().expect("initial harness request"));
    assert_main();

    // When: persisted defaults change before restart/retry, the saved job still drives production I/O.
    let mut config = fixture.dirs.load_config().expect("load global settings");
    config.default_provider = Some(ProviderId::ClaudeCode);
    config.providers.claude_code.default_model = Some("unavailable-global-model".to_string());
    config.providers.ollama.default_model = Some("unavailable-global-model".to_string());
    config.providers.ollama.default_reasoning = Some(ReasoningSelection {
        level: "low".to_string(),
    });
    config.providers.ollama.base_url = "http://127.0.0.1:1/unavailable".to_string();
    fixture
        .dirs
        .save_config(&config)
        .expect("persist changed global defaults");
    let restarted = HarnessJobStore::new(fixture.dirs.clone());
    let mut retried = restarted
        .load(&job.id)
        .expect("reload failed job after restart");
    assert_eq!(retried.status, HarnessJobStatus::Failed);
    retried.retry().expect("explicit retry accepted");
    restarted.save(&retried).expect("persist pending retry");
    let mut retried = restarted.load(&job.id).expect("reload pending retry");
    let delta = generate(&fixture, &mut retried)
        .await
        .expect("retry uses original endpoint");
    let retry_wire = assert_wire(&http.requests.recv().expect("retried harness request"));
    assert!(retry_wire.to_string().contains("provider_protocol_changed"));
    let journal = fixture.tools.journal().clone();
    assert_eq!(
        harness::stage_delta(&fixture.dirs, journal.clone(), &mut retried, delta)
            .expect("stage validated documentation"),
        2
    );
    retried.status = HarnessJobStatus::Review;
    restarted.save(&retried).expect("persist review state");
    let reviewed = restarted.load(&job.id).expect("reload staged review");
    assert_eq!(reviewed.attempts, 2);
    assert_eq!(reviewed.status, HarnessJobStatus::Review);
    assert_eq!(
        crate::journal::JournalStore::new(fixture.dirs.app_data())
            .changeset(
                reviewed
                    .harness_request_id
                    .as_deref()
                    .expect("review request")
            )
            .expect("durable harness changeset")
            .items
            .len(),
        2
    );
    let staged_root = fixture
        .dirs
        .session_workspaces_dir()
        .join(&canonical.id)
        .join(&reviewed.workspace_session_id);
    assert!(std::fs::read_to_string(staged_root.join("specs/game.md"))
        .expect("read staged documentation")
        .contains("Accepted behavior."));
    assert!(staged_root.join("worklog/req-c16-source.md").is_file());
    assert!(!transcript_root(&fixture.dirs, &format!("{}-generator", job.id)).exists());
    assert_main();

    // Then: schema-valid but unapproved promotion never reaches document staging or a journal.
    let mut rejected = HarnessJob::new_with_provider(
        fixture.session_id.clone(),
        reviewed.provider_binding.clone(),
        canonical.project,
        canonical.id.clone(),
        "req-c16-unapproved".to_string(),
        "Reject unapproved fact".to_string(),
        None,
        "Change accepted".to_string(),
        Vec::new(),
        None,
    );
    restarted
        .create(&rejected)
        .expect("persist promotion test job");
    let unapproved = generate(&fixture, &mut rejected)
        .await
        .expect("schema-valid promotion response");
    assert_wire(&http.requests.recv().expect("unapproved harness request"));
    let rejection = harness::stage_delta(&fixture.dirs, journal.clone(), &mut rejected, unapproved)
        .expect_err("unapproved fact must fail before staging");
    assert!(rejection.contains("unapproved task-state fact"));
    assert!(rejected.harness_request_id.is_none());
    assert!(rejected.delta.is_none());
    assert!(journal
        .changeset(&format!("req-{}-{}", rejected.id, rejected.attempts))
        .is_err());
    let rejected_root = fixture
        .dirs
        .session_workspaces_dir()
        .join(canonical.id)
        .join(&rejected.workspace_session_id);
    assert_eq!(
        std::fs::read(rejected_root.join("specs/game.md")).expect("rejected document baseline"),
        original_document
    );
    assert!(!rejected_root.join("worklog/req-c16-unapproved.md").exists());
    assert_main();
    http.join();
}
