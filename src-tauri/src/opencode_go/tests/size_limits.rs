use super::{fixture::*, *};
use crate::provider_runtime::{ProviderRuntime, RuntimeEventSink};

const NORMALIZED_LIMIT: usize = 64 * 1024;

#[derive(Default)]
struct TextSink(parking_lot::Mutex<String>);

impl RuntimeEventSink for TextSink {
    fn emit(&self, event: &AdapterEventKind) -> Result<(), ProviderRuntimeError> {
        if let AdapterEventKind::Block(NormalizedBlock::Text { text, .. }) = event {
            self.0.lock().push_str(text);
        }
        Ok(())
    }
}

fn padding_frame(bytes: usize) -> String {
    frame(Some("keepalive"), json!({"padding":"x".repeat(bytes)}))
}

async fn run_through_runtime(
    wire: OpenCodeGoWire,
    stream: String,
    max_output_bytes: usize,
    sink: &TextSink,
) -> Result<(), ProviderRuntimeError> {
    let mut fixture = serve(wire, stream, None).await;
    let (_cancel, cancellation) = tokio::sync::watch::channel(0);
    let (_ask, ask_waiting) = tokio::sync::watch::channel(false);
    let mut request = fixture_request("fixture-wire", Vec::new(), cancellation);
    request.policy.max_output_bytes = max_output_bytes;
    match ProviderRuntime::run_adapter_step(
        &mut fixture.adapter,
        request,
        None,
        Some(sink),
        ask_waiting,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(error) => Err(error),
    }
}

#[tokio::test]
async fn all_wires_admit_raw_sse_over_normalized_limit_when_output_is_small() {
    let mut outcomes = Vec::new();
    for wire in WIRES {
        let stream = padding_frame(NORMALIZED_LIMIT)
            + &text_frame(wire, "ok")
            + &terminal(wire, false, false);
        assert!(stream.len() > NORMALIZED_LIMIT, "{wire:?}");
        assert!(stream.len() < MAX_RESPONSE_BYTES, "{wire:?}");
        let sink = TextSink::default();

        let outcome = run_through_runtime(wire, stream, NORMALIZED_LIMIT, &sink).await;

        outcomes.push((wire, outcome, sink.0.lock().clone()));
    }
    assert!(
        outcomes
            .iter()
            .all(|(_, outcome, text)| outcome == &Ok(()) && text == "ok"),
        "{outcomes:?}"
    );
}

#[tokio::test]
async fn all_wires_still_reject_normalized_output_over_the_policy_limit() {
    let mut outcomes = Vec::new();
    for wire in WIRES {
        let stream = text_frame(wire, &"x".repeat(2 * 1024)) + &terminal(wire, false, false);
        let sink = TextSink::default();

        let outcome = run_through_runtime(wire, stream, 1024, &sink).await;

        outcomes.push((wire, outcome, sink.0.lock().clone()));
    }
    assert!(
        outcomes.iter().all(|(_, outcome, text)| {
            outcome
                == &Err(ProviderRuntimeError::Protocol(
                    "provider output exceeded its byte limit".into(),
                ))
                && text.is_empty()
        }),
        "{outcomes:?}"
    );
}

#[tokio::test]
async fn all_wires_reject_raw_sse_over_the_transport_ceiling() {
    let frame = padding_frame(1024);
    let mut stream = String::with_capacity(MAX_RESPONSE_BYTES + frame.len());
    while stream.len() <= MAX_RESPONSE_BYTES {
        stream.push_str(&frame);
    }
    assert!(stream.len() > MAX_RESPONSE_BYTES);

    for wire in WIRES {
        let sink = TextSink::default();
        let outcome = run_through_runtime(wire, stream.clone(), usize::MAX, &sink).await;

        assert_eq!(
            outcome,
            Err(ProviderRuntimeError::Protocol(
                "provider response is too large".into()
            )),
            "{wire:?}: {outcome:?}"
        );
        assert!(sink.0.lock().is_empty(), "{wire:?}");
    }
}
