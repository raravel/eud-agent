use super::{fixture::*, *};
use crate::provider_runtime::{
    JobBase, ProviderRuntime, RunOutcome, RuntimeEventSink, StructuredJobExecutor,
    StructuredJobKind, StructuredJobRequest,
};

#[tokio::test]
async fn all_wires_reject_error_eof_and_truncated_responses() {
    for wire in WIRES {
        for failure in ["error", "eof", "truncated"] {
            let tail = match failure {
                "error" => frame(Some("error"), json!({"error":{"message":"fixture-error"}})),
                "eof" => String::new(),
                "truncated" => terminal(wire, false, true),
                _ => unreachable!(),
            };
            let mut fixture = serve(wire, text_frame(wire, "partial") + &tail, None).await;
            let (_cancel, cancellation) = tokio::sync::watch::channel(0);
            let (events, mut received) = tokio::sync::mpsc::channel(16);
            let result = fixture
                .adapter
                .run_step(
                    fixture_request("fixture-wire", Vec::new(), cancellation),
                    events,
                )
                .await;
            assert!(result.is_err(), "{wire:?} {failure}: {result:?}");
            let kinds: Vec<_> = std::iter::from_fn(|| received.try_recv().ok())
                .map(|event| event.kind)
                .collect();
            assert!(kinds.iter().any(|kind| matches!(kind, AdapterEventKind::Block(NormalizedBlock::Text { text, .. }) if text == "partial")));
            assert!(!kinds.iter().any(|kind| matches!(
                kind,
                AdapterEventKind::ResponseFinished { complete: true, .. }
            )));
        }
    }
}

#[tokio::test]
async fn all_wires_reject_invalid_structured_jobs_through_the_runtime() {
    for wire in WIRES {
        for failure in ["malformed", "schema", "duplicate", "forbidden", "truncated"] {
            let name = if failure == "forbidden" {
                "read_file"
            } else {
                STRUCTURED_TOOL
            };
            let args = match failure {
                "malformed" => "{",
                "schema" => r#"{"ok":"wrong-type"}"#,
                _ => r#"{"ok":true}"#,
            };
            let mut stream = call_frame(wire, 0, name, args);
            if failure == "duplicate" {
                stream.push_str(&call_frame(wire, 1, name, args));
            }
            stream.push_str(&terminal(wire, true, failure == "truncated"));
            let fixture = serve(wire, stream, None).await;
            let (_cancel, cancellation) = tokio::sync::watch::channel(0);
            let step = fixture_request("fixture-wire", Vec::new(), cancellation.clone());
            let request = StructuredJobRequest {
                identity: step.identity,
                binding: step.binding,
                kind: StructuredJobKind::TaskStateCompiler,
                prompt: "compile".into(),
                workspace_root: std::env::temp_dir(),
                output_schema: json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}),
                base: JobBase {
                    revision: 0,
                    instruction_epoch: 0,
                    branch: None,
                },
                policy: step.policy,
            };
            let mut runtime = StructuredJobExecutor::new(Box::new(fixture.adapter), cancellation);
            let outcome = runtime.run(request).await;
            assert!(
                matches!(outcome, RunOutcome::Failed(_)),
                "{wire:?} {failure}: {outcome:?}"
            );
        }
    }
}

#[tokio::test]
async fn all_wires_cancel_active_streams_before_late_frames() {
    for wire in WIRES {
        let (release, wait) = tokio::sync::oneshot::channel();
        let mut fixture = serve(
            wire,
            text_frame(wire, "partial"),
            Some((
                text_frame(wire, "late") + &terminal(wire, false, false),
                wait,
            )),
        )
        .await;
        let (cancel, cancellation) = tokio::sync::watch::channel(0);
        let (events, mut received) = tokio::sync::mpsc::channel(16);
        let mut run = Box::pin(fixture.adapter.run_step(
            fixture_request("fixture-wire", Vec::new(), cancellation),
            events,
        ));
        loop {
            tokio::select! {
                result = &mut run => panic!("{wire:?} ended before cancellation: {result:?}"),
                event = received.recv() => {
                    if matches!(event.unwrap().kind, AdapterEventKind::Block(NormalizedBlock::Text { ref text, .. }) if text == "partial") { break; }
                }
            }
        }
        cancel.send(1).unwrap();
        let outcome = run.await;
        let _ = release.send(());
        assert!(
            matches!(
                outcome,
                Err(ProviderRuntimeError::Cancelled) | Ok(AdapterStepOutcome::Cancelled)
            ),
            "{wire:?}: {:?}",
            outcome.as_ref().err()
        );
        while let Some(event) = received.recv().await {
            assert!(
                !matches!(event.kind, AdapterEventKind::Block(NormalizedBlock::Text { ref text, .. }) if text == "late")
            );
            assert!(!matches!(
                event.kind,
                AdapterEventKind::ResponseFinished { complete: true, .. }
            ));
        }
    }
}

#[derive(Default)]
struct TextSink {
    text: parking_lot::Mutex<String>,
    successful_finishes: std::sync::atomic::AtomicUsize,
    tool_blocks: std::sync::atomic::AtomicUsize,
    ready: tokio::sync::Notify,
}
impl RuntimeEventSink for TextSink {
    fn emit(&self, event: &AdapterEventKind) -> Result<(), ProviderRuntimeError> {
        match event {
            AdapterEventKind::NativeSessionStarted { .. } => {}
            AdapterEventKind::Block(NormalizedBlock::Text { text, .. }) => {
                self.text.lock().push_str(text);
                self.ready.notify_one();
            }
            AdapterEventKind::Block(NormalizedBlock::ToolCall { .. }) => {
                self.tool_blocks
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            AdapterEventKind::ResponseFinished { complete: true, .. } => {
                self.successful_finishes
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            AdapterEventKind::ResponseStarted { .. }
            | AdapterEventKind::Block(
                NormalizedBlock::Reasoning { .. } | NormalizedBlock::ToolResult { .. },
            )
            | AdapterEventKind::ResponseFinished {
                complete: false, ..
            }
            | AdapterEventKind::Usage(_)
            | AdapterEventKind::NativeToolObservation { .. }
            | AdapterEventKind::TransportClosed => {}
        }
        Ok(())
    }
}

#[tokio::test]
async fn runtime_preserves_protocol_failures_after_partial_tool_streams() {
    for wire in WIRES {
        for failure in ["malformed", "truncated"] {
            let tail = match failure {
                "malformed" => match wire {
                    OpenCodeGoWire::Responses => {
                        "event: response.output_text.delta\ndata: {\n\n".to_string()
                    }
                    OpenCodeGoWire::ChatCompletions => "data: {\n\n".to_string(),
                    OpenCodeGoWire::AnthropicMessages => {
                        "event: content_block_delta\ndata: {\n\n".to_string()
                    }
                },
                "truncated" => terminal(wire, true, true),
                _ => unreachable!(),
            };
            let stream = text_frame(wire, "partial")
                + &call_frame(wire, 0, "read_file", r#"{"path":"src/main.eps"}"#)
                + &tail;
            let mut fixture = serve(wire, stream, None).await;
            let (_cancel, cancellation) = tokio::sync::watch::channel(0);
            let (_ask, ask_waiting) = tokio::sync::watch::channel(false);
            let sink = TextSink::default();

            let error = match ProviderRuntime::run_adapter_step(
                &mut fixture.adapter,
                fixture_request("fixture-wire", Vec::new(), cancellation),
                None,
                Some(&sink),
                ask_waiting,
            )
            .await
            {
                Err(error) => error,
                Ok(_) => panic!("{wire:?} {failure} unexpectedly succeeded"),
            };

            let expected = match failure {
                "malformed" => ProviderRuntimeError::Protocol("provider_protocol_changed".into()),
                "truncated" => {
                    ProviderRuntimeError::Protocol("provider response was incomplete".into())
                }
                _ => unreachable!(),
            };
            assert_eq!(error, expected, "{wire:?} {failure}");
            assert_eq!(&*sink.text.lock(), "partial", "{wire:?} {failure}");
            assert_eq!(
                sink.successful_finishes
                    .load(std::sync::atomic::Ordering::SeqCst),
                0,
                "{wire:?} {failure}"
            );
            assert_eq!(
                sink.tool_blocks.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "{wire:?} {failure}"
            );
        }
    }
}

#[tokio::test]
async fn all_wires_obey_runtime_deadlines_after_stream_admission() {
    for wire in WIRES {
        let (release, wait) = tokio::sync::oneshot::channel();
        let mut fixture = serve(
            wire,
            text_frame(wire, "partial"),
            Some((
                text_frame(wire, "late") + &terminal(wire, false, false),
                wait,
            )),
        )
        .await;
        let (_cancel, cancellation) = tokio::sync::watch::channel(0);
        let (_ask, ask_waiting) = tokio::sync::watch::channel(false);
        let mut request = fixture_request("fixture-wire", Vec::new(), cancellation);
        request.policy.active_deadline = Some(std::time::Duration::from_secs(30));
        let sink = TextSink::default();
        let mut run = Box::pin(ProviderRuntime::run_adapter_step(
            &mut fixture.adapter,
            request,
            None,
            Some(&sink),
            ask_waiting,
        ));
        tokio::select! {
            result = &mut run => panic!("{wire:?} ended before stream admission: {:?}", result.map(|step| step.outcome)),
            _ = sink.ready.notified() => {}
        }
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(31)).await;
        let outcome = run.await;
        tokio::time::resume();
        let _ = release.send(());
        assert!(
            matches!(outcome, Err(ProviderRuntimeError::TimedOut)),
            "{wire:?}: {:?}",
            outcome.as_ref().err()
        );
        assert_eq!(&*sink.text.lock(), "partial");
    }
}
#[test]
fn anthropic_history_preserves_one_signed_block_across_fragments() {
    let continuation = Some(ProviderContinuation {
        provider: ProviderId::OpencodeGo,
        data: json!({"wire":"anthropic_messages","opaque":{"signature":"signed"}}),
    });
    let history: Vec<_> = ["th", "ink"]
        .into_iter()
        .map(|text| WireHistory::Reasoning {
            response_id: "response".into(),
            text: text.into(),
            continuation: continuation.clone(),
        })
        .collect();
    let messages = anthropic_runtime_history(&history);
    assert_eq!(
        messages[0]["content"],
        json!([{"type":"thinking","thinking":"think","signature":"signed"}])
    );
}
