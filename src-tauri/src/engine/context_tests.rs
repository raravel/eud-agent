use super::{tests::*, AgentTurnResult};
use crate::worker::{WorkerProfile, WorkerRecord, WorkerStatus};

fn request(text: &str) -> crate::ipc::ChatRequest {
    crate::ipc::ChatRequest {
        client_turn_id: crate::ipc::new_client_turn_id(),
        text: text.into(),
        attachments: Vec::new(),
        mentions: Vec::new(),
        execution_mode: Default::default(),
        autonomous_policy: None,
        clarify: false,
        skill_selections: Default::default(),
    }
}

#[tokio::test]
async fn worker_continuation_persists_known_results_without_semantic_compiler() {
    let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
        text: "reviewed".into(),
    }]);
    driver.script_compilers([Ok(Some(
        serde_json::json!({"baseRevision": 0, "operations": []}).to_string(),
    ))]);
    let handle = driver.clone();
    let mut engine = test_engine(driver, CapturingEventSink::default());
    engine
        .chat(request(crate::worker::WORKER_CONTINUE_TEXT))
        .await
        .unwrap();
    assert!(handle.compiler_prompts().is_empty());
}

#[tokio::test]
async fn scoped_workflow_survives_worker_continuation_and_reset() {
    let _shown = crate::skills::show_hidden_skills();
    let driver = FakeCodexDriver::scripted([]);
    let handle = driver.clone();
    let mut engine = test_engine(driver, CapturingEventSink::default());
    let turn = "11111111-1111-4111-8111-111111111111";
    let selected = engine
        .session_store
        .set_task_skill_selection(&engine.session_id, turn, crate::skills::LUCK_DEFENSE, true)
        .unwrap();
    let retry = engine
        .session_store
        .set_task_skill_selection(&engine.session_id, turn, crate::skills::LUCK_DEFENSE, true)
        .unwrap();
    assert_eq!(selected.projection.revision, retry.projection.revision);
    engine
        .session_store
        .replace_task_workflow_progress(
            &engine.session_id,
            crate::task_state::WorkflowProgressUpdate {
                expected_task_revision: retry.projection.revision,
                client_turn_id: Some(turn.to_string()),
                request_id: Some("request-one".to_string()),
                progress: crate::task_state::WorkflowProgress {
                    mode: crate::task_state::WorkflowMode::ContinueExisting,
                    phase: crate::task_state::WorkflowPhase::CoreLoop,
                    last_completed: Some(crate::task_state::WorkflowPhase::Scope),
                    prerequisites: vec!["map baseline".to_string()],
                    next_action: "finish the wave loop".to_string(),
                    pending_playtests: vec!["two player balance".to_string()],
                },
                completed_evidence: Vec::new(),
                provenance: crate::task_state::WorkflowProvenance::local(),
            },
        )
        .unwrap();

    let first = engine
        .prepare_eps_context("continue", None, None, false)
        .await
        .unwrap();
    assert!(first.contains("\n[active skill]\n"), "{first}");
    engine.reset_conversation().await.unwrap();
    let resumed = engine
        .prepare_eps_context(crate::worker::WORKER_CONTINUE_TEXT, None, None, false)
        .await
        .unwrap();
    assert!(resumed.contains("\n[active skill]\n"), "{resumed}");
    let persisted = engine.session_store.load(&engine.session_id).unwrap();
    println!(
        "PERSISTED_EPS_WORKFLOW={}",
        serde_json::to_string(&persisted.task_state.projection.workflow).unwrap()
    );
    assert_eq!(
        persisted.task_state.projection.workflow.mode,
        Some(crate::task_state::WorkflowMode::ContinueExisting)
    );
    assert_eq!(
        persisted.task_state.projection.workflow.next_action,
        "finish the wave loop"
    );
    assert!(handle.compiler_prompts().is_empty());

    let off_turn = "22222222-2222-4222-8222-222222222222";
    engine
        .session_store
        .set_task_skill_selection(
            &engine.session_id,
            off_turn,
            crate::skills::LUCK_DEFENSE,
            false,
        )
        .unwrap();
    engine.reset_conversation().await.unwrap();
    let disabled = engine
        .prepare_eps_context("운빨 디펜스 만들어줘", None, None, false)
        .await
        .unwrap();
    assert!(!disabled.contains("\n[active skill]\n"), "{disabled}");
    assert!(handle.compiler_prompts().is_empty());
}

#[tokio::test]
async fn manual_skills_combine_in_one_active_skill_section() {
    let _shown = crate::skills::show_hidden_skills();
    let mut engine = test_engine(FakeCodexDriver::scripted([]), CapturingEventSink::default());
    let unselected = engine
        .prepare_eps_context(
            "맵 용량 최적화 스킬 켜줘. map-capacity block table 90% 경고",
            None,
            None,
            false,
        )
        .await
        .unwrap();
    assert!(
        !unselected.contains("[skill: map-capacity]"),
        "{unselected}"
    );
    assert!(!unselected.contains("\n[active skill]\n"), "{unselected}");

    let turn = "33333333-3333-4333-8333-333333333333";
    for skill in ["map-capacity", crate::skills::LUCK_DEFENSE] {
        engine
            .session_store
            .set_task_skill_selection(&engine.session_id, turn, skill, true)
            .unwrap();
    }
    let both = engine
        .prepare_eps_context("continue", None, None, false)
        .await
        .unwrap();
    assert_eq!(
        both.lines()
            .filter(|line| *line == "[active skill]")
            .count(),
        1,
        "{both}"
    );
    let luck = both.find("[skill: luck-defense]").unwrap();
    let capacity = both
        .find("[skill: map-capacity]\n# 맵 용량 최적화")
        .unwrap();
    assert!(luck < capacity, "{both}");
    assert!(
        both.contains("<!-- luck-defense:section:router -->"),
        "{both}"
    );

    let off_turn = "44444444-4444-4444-8444-444444444444";
    engine
        .session_store
        .set_task_skill_selection(&engine.session_id, off_turn, "map-capacity", false)
        .unwrap();
    let luck_only = engine
        .prepare_eps_context("continue", None, None, false)
        .await
        .unwrap();
    assert!(!luck_only.contains("[skill: map-capacity]"), "{luck_only}");
    assert!(luck_only.contains("[skill: luck-defense]"), "{luck_only}");
}

#[tokio::test]
async fn native_checkpoint_is_saved_and_delivered_once_then_on_reset() {
    use crate::native_project::{NativeProject, ProjectManifest, PROJECT_SCHEMA_VERSION};
    let driver = FakeCodexDriver::scripted(
        ["first", "second", "third", "fourth"]
            .map(|text| AgentTurnResult::Answer { text: text.into() }),
    );
    let handle = driver.clone();
    let mut engine = test_engine(driver, CapturingEventSink::default());
    let root = engine
        .runtime
        .data_dirs()
        .app_data()
        .join("checkpoint-project");
    std::fs::create_dir_all(root.join("maps")).unwrap();
    std::fs::write(root.join("maps/source.scx"), b"map fixture").unwrap();
    let project = NativeProject::create(
        &root,
        ProjectManifest {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: "Checkpoint".into(),
            source_map: "maps/source.scx".into(),
            output_map: "build/output.scx".into(),
            main_file: "src/main.eps".into(),
            settings: Default::default(),
            plugins: Vec::new(),
            python_entrypoints: Vec::new(),
            python_dependencies: Vec::new(),
            python_lock: None,
            editor_compatibility: None,
            e2_import: None,
        },
    )
    .unwrap();
    engine.runtime.bind_for_tests(&project);
    engine
        .chat(request("keep fixed races and verify gameplay"))
        .await
        .unwrap();
    let state_path = root
        .join(".eud-agent/state/tasks")
        .join(format!("{}.json", engine.session_id));
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state_path).unwrap()).unwrap();
    assert_eq!(saved["requestGoal"], "keep fixed races and verify gameplay");
    let mut record = engine.session_store.load(&engine.session_id).unwrap();
    let mut worker = WorkerRecord::new(
        "interrupted-child".into(),
        "request".into(),
        "pending task".into(),
        "goal".into(),
    );
    worker.status = WorkerStatus::Running;
    record.workers.push(worker);
    engine.session_store.save(&record).unwrap();
    engine.persist_work_checkpoint(None);
    record.workers[0].status = WorkerStatus::Interrupted;
    engine.session_store.save(&record).unwrap();
    engine.chat(request("continue")).await.unwrap();
    engine.chat(request("continue")).await.unwrap();
    engine.reset_conversation().await.unwrap();
    engine.chat(request("continue")).await.unwrap();
    let prompts = handle.prompts();
    assert!(prompts[1].contains("[project work checkpoint]"));
    assert!(prompts[1].contains("activeWorkers=0"));
    assert!(!prompts[2].contains("[project work checkpoint]"));
    assert!(prompts[3].contains("[project work checkpoint]"));
    assert!(prompts[3].contains("check acceptance separately"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn reported_summary_is_omitted_on_the_next_parent_turn() {
    let driver = FakeCodexDriver::scripted(
        ["first", "second"].map(|text| AgentTurnResult::Answer { text: text.into() }),
    );
    let handle = driver.clone();
    let mut engine = test_engine(driver, CapturingEventSink::default());
    let mut record = engine.session_store.load(&engine.session_id).unwrap();
    let mut worker = WorkerRecord::new("child".into(), "req".into(), "task".into(), "goal".into());
    worker.status = WorkerStatus::Settled;
    worker.summary = Some("result-unique-123".into());
    record.workers.push(worker);
    engine.session_store.save(&record).unwrap();
    engine.chat(request("review results")).await.unwrap();

    engine.chat(request("continue")).await.unwrap();

    let prompts = handle.prompts();
    assert_eq!(prompts[0].matches("result-unique-123").count(), 1);
    assert_eq!(prompts[1].matches("result-unique-123").count(), 0);
}

#[tokio::test]
async fn first_worker_goal_is_delivered_once_after_panel_hydration() {
    let goal = "goal-unique-456\nread src/main.eps";
    let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
        text: "done".into(),
    }]);
    let handle = driver.clone();
    let mut engine = test_engine(driver, CapturingEventSink::default());
    engine.worker_origin = Some(("parent".into(), "task".into()));
    engine.runtime.set_worker_profile(WorkerProfile::Research);
    let mut record = engine.session_store.load(&engine.session_id).unwrap();
    record.panel_log = serde_json::json!({"log": [{"kind": "you", "text": goal}]});
    engine.session_store.save(&record).unwrap();
    engine.hydrate().await.unwrap();

    engine.chat(request(goal)).await.unwrap();

    assert_eq!(handle.prompts()[0].matches(goal).count(), 1);
}

#[tokio::test]
async fn investigation_goal_does_not_implicitly_select_a_game_skill() {
    let mut engine = test_engine(FakeCodexDriver::scripted([]), CapturingEventSink::default());
    engine.worker_origin = Some(("parent".into(), "task".into()));
    engine.runtime.set_worker_profile(WorkerProfile::Research);

    engine
        .prepare_eps_context("운빨 디펜스 구현 근거 조사", None, None, false)
        .await
        .unwrap();

    assert!(engine
        .pending_context_delivery
        .as_ref()
        .unwrap()
        .active_skill_sha256
        .is_none());
}

#[tokio::test]
async fn explicit_luck_defense_worker_marker_has_one_active_skill_section() {
    let _shown = crate::skills::show_hidden_skills();
    let mut engine = test_engine(FakeCodexDriver::scripted([]), CapturingEventSink::default());
    engine.worker_origin = Some(("parent".into(), "task".into()));
    engine.runtime.set_worker_profile(WorkerProfile::Research);

    let prompt = engine
        .prepare_eps_context("[$luck-defense]\ninspect existing design", None, None, false)
        .await
        .unwrap();

    assert_eq!(
        prompt
            .lines()
            .filter(|line| *line == "[active skill]")
            .count(),
        1,
        "{prompt}"
    );
    assert_eq!(
        prompt
            .lines()
            .filter(|line| *line == "[luck-defense worker]")
            .count(),
        1,
        "{prompt}"
    );
    for main_section in [
        "<!-- luck-defense:section:router -->",
        "<!-- luck-defense:section:new -->",
        "<!-- luck-defense:section:continue -->",
        "<!-- luck-defense:section:local -->",
    ] {
        assert!(!prompt.lines().any(|line| line == main_section), "{prompt}");
    }
}

#[tokio::test]
async fn investigation_baseline_is_smaller_and_advertises_only_its_tools() {
    let mut engine = test_engine(FakeCodexDriver::scripted([]), CapturingEventSink::default());
    let parent = engine
        .prepare_eps_context("inspect", None, None, false)
        .await
        .unwrap();
    engine.worker_origin = Some(("parent".into(), "task".into()));
    engine.runtime.set_worker_profile(WorkerProfile::Research);

    let worker = engine
        .prepare_eps_context("inspect", None, None, false)
        .await
        .unwrap();

    assert!(worker.chars().count() * 4 < parent.chars().count() * 3);
    let catalog = worker.split("[first principles]").next().unwrap();
    assert!(catalog.contains("source_search"));
    assert!(!catalog.contains("worker_start"));
    assert!(!catalog.contains("dat_patch"));
    assert_eq!(worker.matches("[first principles]").count(), 1);
}

#[tokio::test]
async fn reset_parent_context_restores_reported_worker_results() {
    let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
        text: "done".into(),
    }]);
    let handle = driver.clone();
    let mut engine = test_engine(driver, CapturingEventSink::default());
    let mut record = engine.session_store.load(&engine.session_id).unwrap();
    let mut worker = WorkerRecord::new("child".into(), "req".into(), "task".into(), "goal".into());
    worker.status = WorkerStatus::Settled;
    worker.summary = Some("result-restore-789".into());
    worker.reported = true;
    record.workers.push(worker);
    engine.session_store.save(&record).unwrap();
    engine.reset_conversation().await.unwrap();

    engine.chat(request("continue")).await.unwrap();

    assert_eq!(handle.prompts()[0].matches("result-restore-789").count(), 1);
}

#[tokio::test]
async fn cancelled_parent_turn_keeps_worker_results_unreported() {
    let driver = FakeCodexDriver::scripted([AgentTurnResult::Cancelled]);
    let mut engine = test_engine(driver, CapturingEventSink::default());
    let mut record = engine.session_store.load(&engine.session_id).unwrap();
    let mut worker = WorkerRecord::new("child".into(), "req".into(), "task".into(), "goal".into());
    worker.status = WorkerStatus::Settled;
    worker.summary = Some("result-retry-123".into());
    record.workers.push(worker);
    engine.session_store.save(&record).unwrap();

    engine.chat(request("review")).await.unwrap();

    assert!(
        !engine
            .session_store
            .load(&engine.session_id)
            .unwrap()
            .workers[0]
            .reported
    );
}

#[tokio::test]
async fn worker_recovery_preserves_real_previous_conversation() {
    let mut engine = test_engine(FakeCodexDriver::scripted([]), CapturingEventSink::default());
    engine.worker_origin = Some(("parent".into(), "task".into()));
    engine.runtime.set_worker_profile(WorkerProfile::Research);
    let transcript = "[prior conversation]\nuser: goal-before-123\nassistant: evidence-before-456";

    let prompt = engine
        .prepare_eps_context("continue", None, Some(transcript), true)
        .await
        .unwrap();

    assert!(prompt.contains(transcript));
}
