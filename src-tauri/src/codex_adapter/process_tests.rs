use std::time::Duration;

use super::process_fixture::{
    assert_process_stopped, cleanup_fixture, descendant_pid, fixture_pids, launch_config,
    process_is_running, step_request,
};
use super::*;
use crate::{provider::ProviderConversationState, provider_runtime::AdapterOutput};

#[tokio::test]
async fn production_adapter_process_preserves_resume_and_native_compaction() {
    let (launch, root) = launch_config();
    let mut adapter = CodexAdapter::new(root.clone(), launch);
    let (_cancel, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (events, mut received) = tokio::sync::mpsc::channel(16);
    let first = adapter
        .run_step(
            step_request("first", "http://127.0.0.1:1/mcp", cancel_rx.clone()),
            events.clone(),
        )
        .await
        .unwrap();
    let AdapterStepOutcome::Completed {
        output: AdapterOutput::Text(first),
        native_conversation: Some(ProviderConversationState::Codex { thread_id }),
        ..
    } = first
    else {
        panic!("expected completed first native turn");
    };
    assert_eq!(first, "fixture answer");
    assert_eq!(thread_id.as_deref(), Some("fixture-thread"));
    let mut saw_reasoning = false;
    let mut tool_observations = Vec::new();
    while let Ok(event) = received.try_recv() {
        match event.kind {
            crate::provider_runtime::AdapterEventKind::Block(
                crate::provider_runtime::NormalizedBlock::Reasoning { text, .. },
            ) => saw_reasoning = text == "fixture reasoning",
            crate::provider_runtime::AdapterEventKind::NativeToolObservation {
                call_id,
                name,
                arguments,
                result,
                status,
                ..
            } => tool_observations.push((call_id, name, arguments, result, status)),
            _ => {}
        }
    }
    assert!(saw_reasoning);
    assert_eq!(
        tool_observations,
        vec![
            (
                Some("fixture-call".to_string()),
                "web_search".to_string(),
                Some(serde_json::json!("fixture")),
                None,
                Some("started".to_string()),
            ),
            (
                Some("fixture-call".to_string()),
                "web_search".to_string(),
                None,
                None,
                Some("completed".to_string()),
            ),
        ],
        "a completed webSearch item without status or result must remain one visible completed observation",
    );
    let second = adapter
        .run_step(
            step_request("second", "http://127.0.0.1:2/mcp", cancel_rx.clone()),
            events.clone(),
        )
        .await
        .unwrap();
    assert!(matches!(
        second,
        AdapterStepOutcome::Completed { output: AdapterOutput::Text(ref text), .. }
            if text == "resumed answer"
    ));
    let compact_request = step_request("compact", "http://127.0.0.1:2/mcp", cancel_rx);
    assert_eq!(
        adapter
            .compact(
                compact_request.identity,
                compact_request.cancellation,
                events
            )
            .await
            .unwrap(),
        ProviderConversationState::Codex {
            thread_id: Some("fixture-thread".to_string())
        }
    );
    drop(adapter);
    cleanup_fixture(root).await;
}

#[tokio::test]
async fn production_adapter_process_interrupt_discards_unknown_native_state() {
    let (launch, root) = launch_config();
    let mut adapter = CodexAdapter::new(root.clone(), launch);
    let (cancel, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (events, _received) = tokio::sync::mpsc::channel(16);
    let profile = root.join("profile");
    let mut run = adapter.run_step(
        step_request("cancel", "http://127.0.0.1:1/mcp", cancel_rx),
        events,
    );
    let descendant = tokio::select! {
        result = &mut run => panic!("native turn ended before cancellation barrier: {result:?}"),
        result = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(pid) = descendant_pid(&profile) { break pid; }
                tokio::task::yield_now().await;
            }
        }) => result.expect("the native descendant must start before cancellation"),
    };
    assert!(process_is_running(descendant));
    cancel.send_replace(1);
    let result = run.await;
    assert_eq!(result, Ok(AdapterStepOutcome::Cancelled));
    assert_eq!(
        std::fs::read_dir(root.join("profile"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with("interrupt-"))
            .count(),
        1,
        "the concrete app-server process must observe turn/interrupt"
    );
    assert!(adapter.committed_thread_id.is_none());
    assert!(adapter.client.is_none());
    assert_process_stopped(descendant).await;
    drop(adapter);
    cleanup_fixture(root).await;
}

#[tokio::test]
async fn dropped_turn_poisoning_kills_process_and_forces_fresh_thread() {
    let (launch, root) = launch_config();
    let profile = launch.profile_dir.clone();
    let mut adapter = CodexAdapter::new(root.clone(), launch);
    let (_cancel, cancel_rx) = tokio::sync::watch::channel(0_u64);
    let (events, _received) = tokio::sync::mpsc::channel(16);
    let mut run = adapter.run_step(
        step_request("drop", "http://127.0.0.1:1/mcp", cancel_rx.clone()),
        events.clone(),
    );
    let descendant = tokio::select! {
        result = &mut run => panic!("native turn ended before descendant barrier: {result:?}"),
        result = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(pid) = descendant_pid(&profile) { break pid; }
                tokio::task::yield_now().await;
            }
        }) => result.expect("the native descendant must start"),
    };
    assert!(process_is_running(descendant));
    drop(run);
    let first_pid = fixture_pids(&profile)
        .into_iter()
        .next()
        .expect("the first concrete app-server process must start");
    assert_process_stopped(first_pid).await;
    assert_process_stopped(descendant).await;

    let outcome = adapter
        .run_step(
            step_request("after-drop", "http://127.0.0.1:1/mcp", cancel_rx),
            events,
        )
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        AdapterStepOutcome::Completed { output: AdapterOutput::Text(ref text), .. }
            if text == "fixture answer"
    ));
    assert_eq!(fixture_pids(&profile).len(), 2);
    drop(adapter);
    cleanup_fixture(root).await;
}
