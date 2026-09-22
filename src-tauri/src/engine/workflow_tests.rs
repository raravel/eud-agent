//! Engine-level contracts of the staged workflow: triage routing, clarify,
//! research → plan → critic → plan review, approval → execute → verify loop,
//! feedback revision, and restore/resume.

use std::fs;

use serde_json::json;

use super::tests::{
    native_workspace_snapshot, record_file_write_in_memory, test_engine, CapturingEventSink,
    FakeCodexDriver,
};
use super::{AgentTurnResult, EngineEvent, Phase};
use crate::{
    ipc,
    provider_runtime::{DelegatedRunKind, DelegatedRunOutcome},
    workflow::{WorkflowRoute, WorkflowStage},
    workspace::WorkspaceManager,
};

fn result(value: serde_json::Value) -> DelegatedRunOutcome {
    DelegatedRunOutcome::Result {
        value,
        completions: 1,
        usage: None,
    }
}

fn triage(route: &str) -> DelegatedRunOutcome {
    result(json!({
        "route": route,
        "goal": "웨이브 시스템",
        "acceptanceCriteria": ["1분마다 웨이브가 오른다", "빌드가 성공한다"],
        "rationale": "several modules"
    }))
}

fn research() -> DelegatedRunOutcome {
    result(json!({
        "summary": "spawn.eps가 주기 스폰을 담당한다",
        "relevantFiles": [{"path": "src/spawn.eps", "why": "spawn timer", "symbols": ["spawnUpdate"]}],
        "docEvidence": [{"title": "CreateUnit", "url": "https://docs/createunit", "claim": "CreateUnit places units"}],
        "constraints": ["loops need an exit"],
        "risks": []
    }))
}

fn plan(title: &str) -> DelegatedRunOutcome {
    result(json!({
        "title": title,
        "goal": "웨이브 시스템",
        "acceptanceCriteria": ["1분마다 웨이브가 오른다"],
        "steps": [
            {"id": "S1", "title": "wave module", "files": ["src/wave.eps"], "change": "add", "verification": "build"},
            {"id": "S2", "title": "import", "files": ["src/main.eps"], "change": "import wave", "verification": "build", "dependsOn": ["S1"]}
        ],
        "buildRequired": true,
        "risks": [],
        "outOfScope": []
    }))
}

fn critic(verdict: &str) -> DelegatedRunOutcome {
    result(json!({
        "verdict": verdict,
        "issues": if verdict == "revise" { json!([{"severity": "major", "stepId": "S2", "text": "main.eps import missing"}]) } else { json!([]) },
        "summary": if verdict == "revise" { "import 누락" } else { "문제 없음" }
    }))
}

fn verdict(value: &str) -> DelegatedRunOutcome {
    result(json!({
        "verdict": value,
        "criteria": [{"text": "1분마다 웨이브가 오른다", "status": if value == "pass" { "met" } else { "unmet" }, "evidence": "wave.eps"}],
        "stepStatus": [{"id": "S1", "status": "done"}, {"id": "S2", "status": if value == "pass" { "done" } else { "missing" }, "note": "no import"}],
        "build": {"ok": true, "revision": "r1"},
        "summary": if value == "pass" { "충족" } else { "S2 누락" }
    }))
}

fn chat(text: &str) -> ipc::ChatRequest {
    ipc::ChatRequest {
        client_turn_id: ipc::new_client_turn_id(),
        text: text.to_string(),
        attachments: Vec::new(),
        mentions: Vec::new(),
        execution_mode: Default::default(),
        autonomous_policy: None,
    }
}

fn stages(sink: &CapturingEventSink) -> Vec<WorkflowStage> {
    sink.events()
        .into_iter()
        .filter_map(|event| match event {
            EngineEvent::Workflow(event) => Some(event.stage),
            _ => None,
        })
        .collect()
}

fn plan_events(sink: &CapturingEventSink) -> Vec<u32> {
    sink.events()
        .into_iter()
        .filter_map(|event| match event {
            EngineEvent::Plan(plan) => Some(plan.revision),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn pipeline_route_researches_plans_critiques_and_waits_for_approval() {
    let driver = FakeCodexDriver::scripted([]);
    let handle = driver.clone();
    handle.script_delegated([
        triage("pipeline"),
        research(),
        plan("웨이브"),
        critic("approve"),
    ]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    let workspace = WorkspaceManager::new(dirs.clone())
        .prepare_snapshot(&native_workspace_snapshot(&dirs, "ExampleProject"))
        .unwrap();
    handle.set_workspace(workspace.clone());

    engine.chat(chat("웨이브 시스템 만들어줘")).await.unwrap();

    // No foreground turn ran: the pipeline consumed the request.
    assert!(handle.prompts().is_empty());
    let kinds = handle
        .delegated_requests()
        .into_iter()
        .map(|(kind, _)| kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        [
            DelegatedRunKind::Triage,
            DelegatedRunKind::Research,
            DelegatedRunKind::Planner,
            DelegatedRunKind::Critic
        ]
    );
    assert_eq!(
        stages(&sink),
        [
            WorkflowStage::Triage,
            WorkflowStage::Triage,
            WorkflowStage::Triage,
            WorkflowStage::Research,
            WorkflowStage::Research,
            WorkflowStage::Planning,
            WorkflowStage::Planning,
            WorkflowStage::Critique,
            WorkflowStage::Critique,
            WorkflowStage::PlanReview,
        ]
    );
    assert_eq!(plan_events(&sink), [1]);
    assert_eq!(engine.phase, Phase::PlanReview);
    let request_id = engine.current_request_id.clone().unwrap();
    let research_file = fs::read_to_string(
        workspace
            .workspace_root
            .join(format!("research/{request_id}.md")),
    )
    .unwrap();
    assert!(research_file.contains("`src/spawn.eps` — spawn timer"));
    let plan_file = fs::read_to_string(
        workspace
            .workspace_root
            .join(format!("plans/{request_id}.md")),
    )
    .unwrap();
    assert!(plan_file.starts_with("# 웨이브\n"));
    assert_eq!(
        engine.current_plan_markdown.as_deref(),
        Some(plan_file.as_str())
    );
    let record = engine.session_store.load(&engine.session_id).unwrap();
    let state = record.workflow.expect("workflow persisted on the session");
    assert_eq!(state.stage, WorkflowStage::PlanReview);
    assert_eq!(state.route, Some(WorkflowRoute::Pipeline));
    assert_eq!(
        state.plan.as_ref().unwrap().critic_verdict.as_deref(),
        Some("approve")
    );
    assert_eq!(state.critique_rounds, 1);
    // The planner saw the rendered research and the critic saw the plan.
    let requests = handle.delegated_requests();
    assert!(requests[2].1.contains("spawn timer"));
    assert!(requests[3].1.contains("# 웨이브"));
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn critic_revise_produces_a_second_plan_revision_before_review() {
    let driver = FakeCodexDriver::scripted([]);
    let handle = driver.clone();
    handle.script_delegated([
        triage("pipeline"),
        research(),
        plan("v1"),
        critic("revise"),
        plan("v2"),
    ]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    let workspace = WorkspaceManager::new(dirs.clone())
        .prepare_snapshot(&native_workspace_snapshot(&dirs, "ExampleProject"))
        .unwrap();
    handle.set_workspace(workspace.clone());

    engine.chat(chat("웨이브 시스템 만들어줘")).await.unwrap();

    assert_eq!(
        plan_events(&sink),
        [2],
        "only the revised plan reaches the user"
    );
    let requests = handle.delegated_requests();
    assert_eq!(requests.len(), 5);
    assert!(requests[4].1.contains("[critique]"));
    assert!(requests[4].1.contains("import 누락"));
    let state = engine
        .session_store
        .load(&engine.session_id)
        .unwrap()
        .workflow
        .unwrap();
    let plan = state.plan.as_ref().unwrap();
    assert_eq!(plan.revision, 2);
    // The revised plan itself was not re-reviewed; the history keeps round 1.
    assert_eq!(plan.critic_verdict, None);
    assert_eq!(plan.reviews.len(), 1);
    assert_eq!(plan.reviews[0].critic_verdict, "revise");
    assert_eq!(plan.reviews[0].revision, 1);
    assert_eq!(state.critique_rounds, 1);
    assert!(workspace
        .workspace_root
        .join(format!("verify/{}.plan.1.md", state.request_id))
        .exists());
    assert!(engine
        .current_plan_markdown
        .as_deref()
        .unwrap()
        .starts_with("# v2\n"));
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn answer_route_runs_the_ordinary_foreground_with_a_route_note() {
    let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
        text: "저글링은 spawn.eps에서 스폰됩니다.".to_string(),
    }]);
    let handle = driver.clone();
    handle.script_delegated([result(json!({
        "route": "answer", "goal": "설명", "acceptanceCriteria": [], "rationale": "question"
    }))]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    // No workspace is injected: the stage job prepares the session workspace
    // from the active native project itself.
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    native_workspace_snapshot(&dirs, "ExampleProject");

    engine.chat(chat("저글링은 어디서 스폰돼?")).await.unwrap();

    let prompts = handle.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("[route]\nTriage classified this request as answer-only"));
    assert!(prompts[0].contains("goal: 설명"));
    assert_eq!(stages(&sink).last(), Some(&WorkflowStage::Done));
    assert!(!stages(&sink).contains(&WorkflowStage::Executing));
    assert_eq!(engine.phase, Phase::Idle);
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn scoped_route_skips_research_and_planning_for_one_foreground_turn() {
    let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
        text: "게임 속도 테이블에 21을 썼습니다.".to_string(),
    }]);
    let handle = driver.clone();
    handle.script_delegated([result(json!({
        "route": "scoped",
        "goal": "맵 기본 속도를 2배속으로 만든다",
        "acceptanceCriteria": ["게임 속도 테이블이 2배속 값으로 설정된다"],
        "rationale": "one value once its address is known"
    }))]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    native_workspace_snapshot(&dirs, "ExampleProject");

    engine
        .chat(chat("이 맵의 기본 속도를 2배속으로 해줘."))
        .await
        .unwrap();

    // Triage is the only isolated run: no research, planner, or critic job.
    let kinds = handle
        .delegated_requests()
        .into_iter()
        .map(|(kind, _)| kind)
        .collect::<Vec<_>>();
    assert_eq!(kinds, [DelegatedRunKind::Triage]);
    let prompts = handle.prompts();
    assert_eq!(prompts.len(), 1, "one ordinary foreground turn");
    assert!(prompts[0].contains("Triage classified this request as one scoped change"));
    assert!(prompts[0].contains("smallest change that satisfies the request"));
    // The request executes on the foreground and never reaches plan review.
    let stages = stages(&sink);
    assert!(stages.contains(&WorkflowStage::Executing));
    assert!(!stages.contains(&WorkflowStage::Research));
    assert!(!stages.contains(&WorkflowStage::Planning));
    assert!(!stages.contains(&WorkflowStage::PlanReview));
    assert!(plan_events(&sink).is_empty());
    assert_eq!(engine.phase, Phase::Idle);
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn clarify_asks_the_user_then_retriages_with_the_answer() {
    let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
        text: "스폰 수를 줄였습니다.".to_string(),
    }]);
    let handle = driver.clone();
    handle.script_delegated([
        result(json!({
            "route": "clarify", "goal": "밸런스", "acceptanceCriteria": [], "rationale": "vague",
            "questions": [{"id": "what", "question": "무엇을 조정할까요?", "options": [{"label": "스폰 수"}, {"label": "체력"}]}]
        })),
        result(json!({
            "route": "direct", "goal": "스폰 수 감소", "acceptanceCriteria": ["CreateUnit count decreases"], "rationale": "single site"
        })),
    ]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    native_workspace_snapshot(&dirs, "ExampleProject");
    let runtime = engine.runtime.clone();
    let asks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = asks.clone();
    let answer_runtime = runtime.clone();
    runtime.set_ask_emitter(move |event: ipc::AskEvent| {
        seen.lock().unwrap().push(event.clone());
        let answer_runtime = answer_runtime.clone();
        let mut answers = std::collections::BTreeMap::new();
        answers.insert(
            "what".to_string(),
            ipc::AskAnswer {
                answers: vec!["스폰 수".to_string()],
            },
        );
        tokio::spawn(async move {
            answer_runtime
                .answer_ask(&event.request_id, answers)
                .unwrap();
        });
        Ok(())
    });

    engine.chat(chat("밸런스 좀 맞춰줘")).await.unwrap();

    assert_eq!(asks.lock().unwrap().len(), 1);
    assert_eq!(
        asks.lock().unwrap()[0].questions[0].question,
        "무엇을 조정할까요?"
    );
    let requests = handle.delegated_requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1]
        .1
        .contains("[clarification]\nround 1: 무엇을 조정할까요? → 스폰 수"));
    let prompts = handle.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("round 1: 무엇을 조정할까요? → 스폰 수"));
    assert!(stages(&sink).contains(&WorkflowStage::Clarify));
    let state = engine
        .session_store
        .load(&engine.session_id)
        .unwrap()
        .workflow
        .unwrap();
    assert_eq!(state.route, Some(WorkflowRoute::Direct));
    assert_eq!(state.clarifications.len(), 1);
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn unanswered_clarify_ask_hands_off_as_text_and_the_next_message_answers_it() {
    let driver = FakeCodexDriver::scripted([AgentTurnResult::Answer {
        text: "스폰 수를 줄였습니다.".to_string(),
    }]);
    let handle = driver.clone();
    handle.script_delegated([
        result(json!({
            "route": "clarify", "goal": "밸런스", "acceptanceCriteria": [], "rationale": "vague",
            "questions": [{"id": "what", "question": "무엇을 조정할까요?", "options": [{"label": "스폰 수"}, {"label": "체력", "description": "유닛 HP"}]}]
        })),
        result(json!({
            "route": "direct", "goal": "스폰 수 감소", "acceptanceCriteria": ["CreateUnit count decreases"], "rationale": "single site"
        })),
    ]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    native_workspace_snapshot(&dirs, "ExampleProject");
    let runtime = engine.runtime.clone();
    let asks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = asks.clone();
    // Nobody answers the card: the bounded wait elapses.
    runtime.set_ask_emitter(move |event: ipc::AskEvent| {
        seen.lock().unwrap().push(event);
        Ok(())
    });
    runtime.set_ask_wait_timeout(std::time::Duration::from_millis(30));

    engine
        .chat(chat("밸런스 좀 맞춰줘"))
        .await
        .expect("an unanswered clarify ask is a text handoff, not a failure");

    let statuses = asks
        .lock()
        .unwrap()
        .iter()
        .map(|event| event.status)
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        vec![ipc::AskEventStatus::Pending, ipc::AskEventStatus::Expired]
    );
    let answer = sink
        .events()
        .into_iter()
        .find_map(|event| match event {
            EngineEvent::Answer(answer) => Some(answer.text),
            _ => None,
        })
        .expect("the turn ends with the questions as its answer");
    assert!(answer.contains("1. 무엇을 조정할까요?"), "{answer}");
    assert!(answer.contains("- 스폰 수"), "{answer}");
    assert!(answer.contains("- 체력 — 유닛 HP"), "{answer}");
    assert_eq!(stages(&sink).last(), Some(&WorkflowStage::Done));
    assert!(!stages(&sink).contains(&WorkflowStage::Failed));
    assert_eq!(engine.phase, Phase::Idle);
    let state = engine
        .session_store
        .load(&engine.session_id)
        .unwrap()
        .workflow
        .unwrap();
    let pending = state
        .pending_clarification
        .expect("the handed-off questions persist for the next message");
    assert_eq!(pending.questions[0].id, "what");
    assert_eq!(pending.rounds, 0);
    assert_eq!(handle.delegated_requests().len(), 1);
    assert!(handle.prompts().is_empty());

    // The next message is the reply: triage continues the original request
    // with it, and the direct route reaches the foreground turn.
    engine.chat(chat("스폰 수")).await.unwrap();

    let requests = handle.delegated_requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].1.contains("[user message]\n밸런스 좀 맞춰줘"),
        "{}",
        requests[1].1
    );
    assert!(
        requests[1]
            .1
            .contains("[clarification]\nround 1: 무엇을 조정할까요? → 스폰 수"),
        "{}",
        requests[1].1
    );
    assert!(
        requests[1].1.contains("1 clarify round(s) remain"),
        "{}",
        requests[1].1
    );
    let prompts = handle.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains(
            "[clarification]\noriginal request: 밸런스 좀 맞춰줘\nround 1: 무엇을 조정할까요? → 스폰 수"
        ),
        "{}",
        prompts[0]
    );
    let state = engine
        .session_store
        .load(&engine.session_id)
        .unwrap()
        .workflow
        .unwrap();
    assert_eq!(state.route, Some(WorkflowRoute::Direct));
    assert_eq!(state.user_text, "밸런스 좀 맞춰줘");
    assert_eq!(state.clarifications.len(), 1);
    assert!(state.pending_clarification.is_none());
    assert_eq!(
        asks.lock().unwrap().len(),
        2,
        "the reply never reopens an ask card"
    );
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn approval_executes_verifies_fixes_once_and_reaches_review() {
    let driver = FakeCodexDriver::scripted([
        AgentTurnResult::Answer {
            text: "S1 — done; S2 — skipped".to_string(),
        },
        AgentTurnResult::Answer {
            text: "S1 — done; S2 — done".to_string(),
        },
    ]);
    let handle = driver.clone();
    handle.script_delegated([
        triage("pipeline"),
        research(),
        plan("웨이브"),
        critic("approve"),
        verdict("fail"),
        verdict("pass"),
    ]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    engine.executor.plan_runtime = Some(engine.runtime.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    // The pipeline runs before any foreground turn, so nothing has prepared
    // the session workspace: approval and the stages must do it themselves.
    let workspace = WorkspaceManager::new(dirs.clone())
        .prepare_snapshot(&native_workspace_snapshot(&dirs, "ExampleProject"))
        .unwrap();

    engine.chat(chat("웨이브 시스템 만들어줘")).await.unwrap();
    let request_id = engine.current_request_id.clone().unwrap();
    engine.plan_approve().await.unwrap();
    // The executing turn journals one change before verification judges it.
    record_file_write_in_memory(
        &engine.journal_store,
        &request_id,
        "wave",
        1,
        "src/wave.eps",
    );
    engine.continue_pending_write().await.unwrap();

    let prompts = handle.prompts();
    assert_eq!(
        prompts.len(),
        2,
        "one execute turn and one verification fix turn"
    );
    assert!(prompts[0].contains(&format!("plans/{request_id}.md")));
    assert!(prompts[0].contains("Acceptance criteria the verifier will check"));
    // Direct providers cannot read workspace files: the plan and research
    // travel inline with the instruction.
    assert!(prompts[0].contains("[approved plan]\n# 웨이브"));
    assert!(prompts[0].contains("[research]\n# 조사"));
    assert!(prompts[1].contains("[verification]"));
    assert!(prompts[1].contains("step S2 is missing: no import"));
    let kinds = handle
        .delegated_requests()
        .into_iter()
        .map(|(kind, _)| kind)
        .collect::<Vec<_>>();
    assert_eq!(
        &kinds[4..],
        [DelegatedRunKind::Verifier, DelegatedRunKind::Verifier]
    );
    let verifier_prompt = &handle.delegated_requests()[4].1;
    assert!(verifier_prompt.contains("`src/wave.eps`"));
    assert!(verifier_prompt.contains("S1 — done; S2 — skipped"));
    assert_eq!(engine.phase, Phase::ChangesetReview);
    let state = engine
        .session_store
        .load(&engine.session_id)
        .unwrap()
        .workflow
        .unwrap();
    assert_eq!(state.stage, WorkflowStage::ChangesetReview);
    assert_eq!(state.verify_attempts, 2);
    assert_eq!(state.verdict.as_ref().unwrap().verdict, "pass");
    assert!(state.plan.as_ref().unwrap().approved_sha256.is_some());
    for attempt in [1, 2] {
        assert!(workspace
            .workspace_root
            .join(format!("verify/{request_id}.{attempt}.md"))
            .exists());
    }
    // Review starts only after verification: the changeset event follows
    // the last verifying projection.
    let events = sink.events();
    let last_verifying = events
        .iter()
        .rposition(|event| {
            matches!(event, EngineEvent::Workflow(event) if event.stage == WorkflowStage::Verifying)
        })
        .expect("a verifying projection");
    let changeset_index = events
        .iter()
        .position(|event| matches!(event, EngineEvent::Changeset(_)))
        .expect("a changeset event");
    assert!(changeset_index > last_verifying);

    engine
        .changeset_decision(ipc::ChangesetDecisionRequest {
            decision: ipc::Decision::Accept,
            ids: ipc::DecisionIds::All(ipc::AllLiteral),
        })
        .await
        .unwrap();
    assert_eq!(stages(&sink).last(), Some(&WorkflowStage::Done));
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn two_failed_verifications_still_reach_review_with_the_verdict() {
    let driver = FakeCodexDriver::scripted([
        AgentTurnResult::Answer {
            text: "S1 — done; S2 — skipped".to_string(),
        },
        AgentTurnResult::Answer {
            text: "S1 — done; S2 — still skipped".to_string(),
        },
    ]);
    let handle = driver.clone();
    handle.script_delegated([
        triage("pipeline"),
        research(),
        plan("웨이브"),
        critic("approve"),
        verdict("fail"),
        verdict("fail"),
    ]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    engine.executor.plan_runtime = Some(engine.runtime.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    let workspace = WorkspaceManager::new(dirs.clone())
        .prepare_snapshot(&native_workspace_snapshot(&dirs, "ExampleProject"))
        .unwrap();
    handle.set_workspace(workspace);

    engine.chat(chat("웨이브 시스템 만들어줘")).await.unwrap();
    let request_id = engine.current_request_id.clone().unwrap();
    engine.plan_approve().await.unwrap();
    record_file_write_in_memory(
        &engine.journal_store,
        &request_id,
        "wave",
        1,
        "src/wave.eps",
    );
    engine.continue_pending_write().await.unwrap();

    assert_eq!(
        handle.prompts().len(),
        2,
        "one execute turn, one fix turn, no third"
    );
    assert_eq!(engine.phase, Phase::ChangesetReview);
    let state = engine
        .session_store
        .load(&engine.session_id)
        .unwrap()
        .workflow
        .unwrap();
    assert_eq!(state.stage, WorkflowStage::ChangesetReview);
    assert_eq!(state.verify_attempts, 2);
    assert_eq!(state.verdict.as_ref().unwrap().verdict, "fail");
    assert_eq!(
        state.verdict.as_ref().unwrap().unmet,
        [
            "1분마다 웨이브가 오른다 (unmet: wave.eps)",
            "step S2 is missing: no import"
        ]
    );
    // The accepted changeset hands the rendered verdict to the harness job.
    let job = engine
        .changeset_decision(ipc::ChangesetDecisionRequest {
            decision: ipc::Decision::Accept,
            ids: ipc::DecisionIds::All(ipc::AllLiteral),
        })
        .await
        .unwrap()
        .expect("accepted changes schedule a harness job");
    assert!(job
        .verify_verdict
        .as_deref()
        .unwrap()
        .contains("판정: **fail**"));
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn interrupted_research_resumes_and_restart_clears_the_request() {
    let driver = FakeCodexDriver::scripted([]);
    let handle = driver.clone();
    handle.script_delegated([
        triage("pipeline"),
        research(),
        plan("웨이브"),
        critic("approve"),
    ]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    let workspace = WorkspaceManager::new(dirs.clone())
        .prepare_snapshot(&native_workspace_snapshot(&dirs, "ExampleProject"))
        .unwrap();
    handle.set_workspace(workspace.clone());
    engine.chat(chat("웨이브 시스템 만들어줘")).await.unwrap();
    let session_id = engine.session_id.clone();
    let sessions = engine.session_store.clone();

    // Simulate a shutdown during research and the startup recovery.
    let mut record = sessions.load(&session_id).unwrap();
    let workflow = record.workflow.as_mut().unwrap();
    workflow.stage = WorkflowStage::Research;
    workflow.research = None;
    workflow.plan = None;
    sessions.save(&record).unwrap();
    assert_eq!(sessions.recover_interrupted_workflows().unwrap(), 1);

    let record = sessions.load(&session_id).unwrap();
    let runtime = crate::tool_exec::ToolServices::new(
        dirs.clone(),
        crate::map_candidate::CandidateStore::new(
            dirs.clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        ),
        crate::write_coordinator::ProjectWriteCoordinator::silent(),
    )
    .session("test-session");
    let driver = FakeCodexDriver::scripted([]);
    let handle = driver.clone();
    handle.script_delegated([research(), plan("재개된 웨이브"), critic("approve")]);
    let sink = CapturingEventSink::default();
    let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
    let mut restored = super::AgentEngine::new(
        driver,
        sink.clone(),
        super::AgentEngineConfig::for_tests("[project state]\nproject=Sample", None, Vec::new()),
        runtime,
        sessions.clone(),
        super::tests::attachment_store_at(dirs.app_data()),
        record,
        cancellation,
    );
    restored.journal_store = engine.journal_store.clone();
    restored.hydrate().await.unwrap();
    assert_eq!(stages(&sink), [WorkflowStage::Interrupted]);

    // Resume restarts research from its persisted inputs; nothing is replayed.
    restored.workflow_resume().await.unwrap();
    let kinds = handle
        .delegated_requests()
        .into_iter()
        .map(|(kind, _)| kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        [
            DelegatedRunKind::Research,
            DelegatedRunKind::Planner,
            DelegatedRunKind::Critic
        ]
    );
    assert_eq!(restored.phase, Phase::PlanReview);
    assert_eq!(plan_events(&sink), [1]);
    assert!(restored
        .current_plan_markdown
        .as_deref()
        .unwrap()
        .starts_with("# 재개된 웨이브"));

    // Restart is refused while the plan is under review, and allowed once
    // the request is interrupted again.
    assert!(restored.workflow_restart().is_err());
    let mut record = sessions.load(&session_id).unwrap();
    record.workflow.as_mut().unwrap().stage = WorkflowStage::Interrupted;
    sessions.save(&record).unwrap();
    restored.workflow = record.workflow.clone();
    restored.phase = Phase::Idle;
    let text = restored.workflow_restart().unwrap();
    assert_eq!(text, "웨이브 시스템 만들어줘");
    assert!(sessions.load(&session_id).unwrap().workflow.is_none());
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn a_later_message_sets_the_interrupted_request_aside_and_triage_is_told() {
    let driver = FakeCodexDriver::scripted([]);
    let handle = driver.clone();
    handle.script_delegated([
        triage("pipeline"),
        research(),
        plan("웨이브"),
        critic("approve"),
        // The follow-up message is triaged on its own.
        triage("answer"),
    ]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    let workspace = WorkspaceManager::new(dirs.clone())
        .prepare_snapshot(&native_workspace_snapshot(&dirs, "ExampleProject"))
        .unwrap();
    handle.set_workspace(workspace.clone());
    engine.chat(chat("웨이브 시스템 만들어줘")).await.unwrap();
    let session_id = engine.session_id.clone();
    let sessions = engine.session_store.clone();
    let interrupted_request_id = sessions
        .load(&session_id)
        .unwrap()
        .workflow
        .unwrap()
        .request_id;

    // A shutdown during research, then the startup recovery.
    let mut record = sessions.load(&session_id).unwrap();
    let workflow = record.workflow.as_mut().unwrap();
    workflow.stage = WorkflowStage::Research;
    workflow.research = None;
    workflow.plan = None;
    sessions.save(&record).unwrap();
    assert_eq!(sessions.recover_interrupted_workflows().unwrap(), 1);
    engine.workflow = sessions.load(&session_id).unwrap().workflow;

    // The next message does not discard the interrupted request: it is set
    // aside, projected to the panel, and named in the triage prompt.
    engine.chat(chat("continue")).await.unwrap();
    let stashed = sessions
        .load(&session_id)
        .unwrap()
        .interrupted_workflow
        .expect("the interrupted request is kept");
    assert_eq!(stashed.request_id, interrupted_request_id);
    assert_eq!(stashed.stage, WorkflowStage::Interrupted);
    assert_eq!(
        sessions
            .load(&session_id)
            .unwrap()
            .workflow
            .map(|live| live.user_text),
        Some("continue".to_string())
    );
    let triage_prompt = handle
        .delegated_requests()
        .into_iter()
        .filter(|(kind, _)| *kind == DelegatedRunKind::Triage)
        .map(|(_, prompt)| prompt)
        .next_back()
        .expect("the follow-up was triaged");
    assert!(triage_prompt.contains("[interrupted request]"));
    assert!(triage_prompt.contains("웨이브 시스템"));
    assert!(triage_prompt.contains("[continuity]"));
    assert_eq!(
        sink.events()
            .into_iter()
            .filter(|event| matches!(event, EngineEvent::InterruptedRequest(_)))
            .count(),
        1
    );

    // Resume works on the set-aside request, which then stops being pending.
    handle.script_delegated([research(), plan("재개된 웨이브"), critic("approve")]);
    engine.phase = Phase::Idle;
    engine.workflow_resume().await.unwrap();
    assert_eq!(engine.phase, Phase::PlanReview);
    let after = sessions.load(&session_id).unwrap();
    assert!(after.interrupted_workflow.is_none());
    assert_eq!(
        after.workflow.map(|live| live.request_id),
        Some(interrupted_request_id)
    );
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn plan_feedback_revises_through_the_planner_not_the_foreground() {
    let driver = FakeCodexDriver::scripted([]);
    let handle = driver.clone();
    handle.script_delegated([
        triage("pipeline"),
        research(),
        plan("v1"),
        critic("approve"),
        plan("v2 with feedback"),
    ]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    let workspace = WorkspaceManager::new(dirs.clone())
        .prepare_snapshot(&native_workspace_snapshot(&dirs, "ExampleProject"))
        .unwrap();
    handle.set_workspace(workspace.clone());
    engine.chat(chat("웨이브 시스템 만들어줘")).await.unwrap();

    engine
        .plan_feedback(ipc::PlanFeedbackRequest {
            text: "히드라도 넣어줘".to_string(),
            client_turn_id: ipc::new_client_turn_id(),
            attachments: Vec::new(),
            mentions: Vec::new(),
        })
        .await
        .unwrap();

    assert!(handle.prompts().is_empty());
    let requests = handle.delegated_requests();
    assert_eq!(requests.len(), 5);
    assert!(requests[4].1.contains("[user feedback]"));
    assert!(requests[4].1.contains("히드라도 넣어줘"));
    assert!(requests[4].1.contains("[previous plan]\n# v1"));
    assert_eq!(plan_events(&sink), [1, 2]);
    assert_eq!(engine.phase, Phase::PlanReview);
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn stage_failure_marks_the_workflow_failed_and_returns_the_error() {
    let driver = FakeCodexDriver::scripted([]);
    let handle = driver.clone();
    handle.script_delegated([
        triage("pipeline"),
        DelegatedRunOutcome::Failed(crate::provider_runtime::ProviderRuntimeError::Protocol(
            "delegated run ended without submit_result".into(),
        )),
    ]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    let workspace = WorkspaceManager::new(dirs.clone())
        .prepare_snapshot(&native_workspace_snapshot(&dirs, "ExampleProject"))
        .unwrap();
    handle.set_workspace(workspace);

    let error = engine
        .chat(chat("웨이브 시스템 만들어줘"))
        .await
        .unwrap_err();
    assert!(error.message.contains("조사 단계를 완료하지 못했습니다"));
    let state = engine
        .session_store
        .load(&engine.session_id)
        .unwrap()
        .workflow
        .unwrap();
    assert_eq!(state.stage, WorkflowStage::Failed);
    assert!(state.error.as_deref().unwrap().contains("submit_result"));
    assert_eq!(engine.phase, Phase::Idle);
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn cancelled_stage_returns_to_idle_and_keeps_completed_artifacts() {
    let driver = FakeCodexDriver::scripted([]);
    let handle = driver.clone();
    handle.script_delegated([
        triage("pipeline"),
        research(),
        DelegatedRunOutcome::Cancelled,
    ]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    let workspace = WorkspaceManager::new(dirs.clone())
        .prepare_snapshot(&native_workspace_snapshot(&dirs, "ExampleProject"))
        .unwrap();
    handle.set_workspace(workspace.clone());

    engine.chat(chat("웨이브 시스템 만들어줘")).await.unwrap();

    let request_id = engine.current_request_id.clone().unwrap();
    assert!(workspace
        .workspace_root
        .join(format!("research/{request_id}.md"))
        .exists());
    assert_eq!(stages(&sink).last(), Some(&WorkflowStage::Cancelled));
    assert_eq!(engine.phase, Phase::Idle);
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[tokio::test]
async fn hydrate_restores_plan_review_and_startup_interrupts_in_flight_stages() {
    let driver = FakeCodexDriver::scripted([]);
    let handle = driver.clone();
    handle.script_delegated([
        triage("pipeline"),
        research(),
        plan("웨이브"),
        critic("approve"),
    ]);
    let sink = CapturingEventSink::default();
    let mut engine = test_engine(driver, sink.clone());
    let dirs = engine.runtime.data_dirs();
    dirs.ensure_dirs().unwrap();
    let workspace = WorkspaceManager::new(dirs.clone())
        .prepare_snapshot(&native_workspace_snapshot(&dirs, "ExampleProject"))
        .unwrap();
    handle.set_workspace(workspace.clone());
    engine.chat(chat("웨이브 시스템 만들어줘")).await.unwrap();
    let session_id = engine.session_id.clone();
    let sessions = engine.session_store.clone();

    // A fresh engine over the same session restores the plan card.
    let driver = FakeCodexDriver::scripted([]);
    let handle = driver.clone();
    handle.set_workspace(workspace.clone());
    let sink = CapturingEventSink::default();
    let record = sessions.load(&session_id).unwrap();
    let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
    // The restored runtime shares the app data dirs so workspace artifacts resolve.
    let runtime = crate::tool_exec::ToolServices::new(
        dirs.clone(),
        crate::map_candidate::CandidateStore::new(
            dirs.clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        ),
        crate::write_coordinator::ProjectWriteCoordinator::silent(),
    )
    .session("test-session");
    let mut restored = super::AgentEngine::new(
        driver,
        sink.clone(),
        super::AgentEngineConfig::for_tests("[project state]\nproject=Sample", None, Vec::new()),
        runtime,
        sessions.clone(),
        super::tests::attachment_store_at(dirs.app_data()),
        record,
        cancellation,
    );
    restored.journal_store = engine.journal_store.clone();
    restored.hydrate().await.unwrap();
    assert_eq!(restored.phase, Phase::PlanReview);
    assert_eq!(plan_events(&sink), [1]);
    assert_eq!(stages(&sink), [WorkflowStage::PlanReview]);
    assert!(restored
        .current_plan_markdown
        .as_deref()
        .unwrap()
        .starts_with("# 웨이브"));

    // An in-flight stage at startup becomes Interrupted; review stages do not.
    let mut record = sessions.load(&session_id).unwrap();
    record.workflow.as_mut().unwrap().stage = WorkflowStage::Research;
    sessions.save(&record).unwrap();
    assert_eq!(sessions.recover_interrupted_workflows().unwrap(), 1);
    let state = sessions.load(&session_id).unwrap().workflow.unwrap();
    assert_eq!(state.stage, WorkflowStage::Interrupted);
    assert_eq!(state.interrupted_stage, Some(WorkflowStage::Research));
    assert_eq!(sessions.recover_interrupted_workflows().unwrap(), 0);
    fs::remove_dir_all(dirs.app_data()).ok();
}

#[test]
fn project_map_lists_sources_and_is_delivered_as_a_delta_slot() {
    let dirs = crate::config::DataDirs::from_bases(
        &super::tests::unique_temp_dir("project-map"),
        &super::tests::unique_temp_dir("project-map-local"),
    );
    dirs.ensure_dirs().unwrap();
    native_workspace_snapshot(&dirs, "MapProject");
    let manager = crate::native_runtime::NativeProjectManager::new(dirs.clone());
    let map = manager.render_project_map().unwrap();
    assert!(map.starts_with("[project map]\nmainFile=src/main.eps\n"));
    assert!(map.contains("files (1):\n- src/main.eps ("));
    assert!(map.contains("dat overrides: standard=0, xdat=0, tbl=0, requirements=0, buttons=0"));

    // The context cursor hashes the map: unchanged on a follow-up, replaced when it changes.
    let baseline = super::static_prompt_baseline();
    let mut context = crate::context_state::SessionContextState::default();
    context.initialize_baseline(&baseline);
    let input = |map: Option<&'static str>| crate::context_state::ContextAssemblyInput {
        static_baseline: &baseline,
        project_state: "[project state]\nproject=Sample",
        project_memory: None,
        wiki_facts: None,
        project_map: map,
        reference_context: None,
        task_revision: 0,
        task_snapshot: "[active task state]\n{}",
        task_delta: None,
        replay_transcript: None,
        resolved_mentions: None,
        user_text: "hello",
        provider: crate::provider::ProviderId::Codex,
        current_conversation_key: Some("thread"),
        force_full: false,
    };
    let first =
        crate::context_state::assemble_context(&context, input(Some("[project map]\nv1"))).unwrap();
    assert!(first.text.contains("[project map]\nv1"));
    context.delivered = first.cursor.clone();
    let same =
        crate::context_state::assemble_context(&context, input(Some("[project map]\nv1"))).unwrap();
    assert!(!same.text.contains("[project map]"));
    let changed =
        crate::context_state::assemble_context(&context, input(Some("[project map]\nv2"))).unwrap();
    assert!(changed
        .text
        .contains("[project map delta instructionEpoch="));
    assert!(changed.text.contains("v2"));
    fs::remove_dir_all(dirs.app_data()).ok();
}
