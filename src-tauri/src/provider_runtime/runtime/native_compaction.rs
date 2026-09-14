use super::{
    events::{handle_event, EventContext},
    ProviderRuntime, RuntimeEventSink,
};
use crate::{
    provider::ProviderConversationState,
    provider_runtime::{CompactionRequest, ProviderAdapter, ProviderRuntimeError},
};
use std::time::Duration;

impl ProviderRuntime {
    pub(super) async fn prepare_native_compaction(
        adapter: &mut dyn ProviderAdapter,
        request: &CompactionRequest,
        mut cancellation: tokio::sync::watch::Receiver<u64>,
    ) -> Result<(), ProviderRuntimeError> {
        if *cancellation.borrow_and_update() != request.identity.cancellation_generation {
            return Err(ProviderRuntimeError::Cancelled);
        }
        let preparation = adapter.prepare_compaction(request);
        tokio::pin!(preparation);
        let deadline = tokio::time::sleep(
            request
                .policy
                .active_deadline
                .unwrap_or(Duration::from_secs(365 * 24 * 60 * 60)),
        );
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                biased;
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow_and_update() != request.identity.cancellation_generation {
                        return Err(ProviderRuntimeError::Cancelled);
                    }
                }
                _ = &mut deadline, if request.policy.active_deadline.is_some() => return Err(ProviderRuntimeError::TimedOut),
                result = &mut preparation => return result,
            }
        }
    }

    pub(super) async fn run_native_compaction(
        adapter: &mut dyn ProviderAdapter,
        request: &CompactionRequest,
        sink: &dyn RuntimeEventSink,
        cancellation: tokio::sync::watch::Receiver<u64>,
    ) -> Result<ProviderConversationState, ProviderRuntimeError> {
        let mut cancellation_observer = cancellation.clone();
        let cancellation_generation = request.identity.cancellation_generation;
        if *cancellation_observer.borrow_and_update() != cancellation_generation {
            return Err(ProviderRuntimeError::Cancelled);
        }
        let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(64);
        let compact = adapter.compact(request.identity.clone(), cancellation, events_tx);
        tokio::pin!(compact);
        let mut blocks = Vec::new();
        let mut started = None;
        let mut finished = None;
        let mut streamed_bytes = 0;
        let mut native_completion_in_flight = None;
        let mut events_open = true;
        let parked = Duration::from_secs(365 * 24 * 60 * 60);
        let deadline = tokio::time::sleep(request.policy.active_deadline.unwrap_or(parked));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                biased;
                changed = cancellation_observer.changed() => {
                    if changed.is_err()
                        || *cancellation_observer.borrow_and_update() != cancellation_generation
                    {
                        return Err(ProviderRuntimeError::Cancelled);
                    }
                }
                _ = &mut deadline, if request.policy.active_deadline.is_some() => {
                    return Err(ProviderRuntimeError::TimedOut);
                }
                event = events_rx.recv(), if events_open => {
                    let Some(event) = event else { events_open = false; continue };
                    handle_event(
                        event,
                        EventContext {
                            identity: &request.identity,
                            sink: Some(sink),
                            blocks: &mut blocks,
                            started: &mut started,
                            finished: &mut finished,
                            streamed_bytes: &mut streamed_bytes,
                            max_output_bytes: request.policy.max_output_bytes,
                            expected_provider: request.binding.provider,
                            native_tool_events: None,
                            native_completion_in_flight: &mut native_completion_in_flight,
                        },
                    )?;
                }
                result = &mut compact => {
                    let conversation = result?;
                    events_rx.close();
                    while let Ok(event) = events_rx.try_recv() {
                        handle_event(
                            event,
                            EventContext {
                                identity: &request.identity,
                                sink: Some(sink),
                                blocks: &mut blocks,
                                started: &mut started,
                                finished: &mut finished,
                                streamed_bytes: &mut streamed_bytes,
                                max_output_bytes: request.policy.max_output_bytes,
                                expected_provider: request.binding.provider,
                                native_tool_events: None,
                                native_completion_in_flight: &mut native_completion_in_flight,
                            },
                        )?;
                    }
                    if !finished.is_some_and(|(_, complete)| complete) {
                        return Err(ProviderRuntimeError::Protocol(
                            "native compaction ended without a complete boundary".into(),
                        ));
                    }
                    return Ok(conversation);
                }
            }
        }
    }
}
