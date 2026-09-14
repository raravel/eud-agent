use super::{
    events::{handle_event, EventContext},
    tool_events::RunToolEvents,
    ProviderRuntime, RuntimeEventSink,
};
use crate::provider_runtime::{
    AdapterLoopKind, AdapterStepOutcome, AdapterStepRequest, NormalizedBlock, ProviderAdapter,
    ProviderRuntimeError,
};
use std::time::Duration;

impl ProviderRuntime {
    pub(crate) async fn run_adapter_step(
        adapter: &mut dyn ProviderAdapter,
        request: AdapterStepRequest,
        mut tool_events: Option<&mut RunToolEvents>,
        sink: Option<&dyn RuntimeEventSink>,
        mut ask_waiting: tokio::sync::watch::Receiver<bool>,
    ) -> Result<StepResult, ProviderRuntimeError> {
        let expected_identity = request.identity.clone();
        let expected_provider = request.binding.provider;
        let native_session = adapter.loop_kind() == AdapterLoopKind::NativeSession;
        let mut cancellation = request.cancellation.clone();
        let cancellation_generation = expected_identity.cancellation_generation;
        if *cancellation.borrow_and_update() != cancellation_generation {
            return Err(ProviderRuntimeError::Cancelled);
        }
        let max_output_bytes = request.policy.max_output_bytes;
        let deadline = request.policy.active_deadline;
        let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(64);
        let step = adapter.run_step(request, events_tx);
        tokio::pin!(step);
        let parked = Duration::from_secs(365 * 24 * 60 * 60);
        let timer = tokio::time::sleep(deadline.unwrap_or(parked));
        tokio::pin!(timer);
        let mut active_started = tokio::time::Instant::now();
        let mut remaining = deadline;
        let mut ask_paused = *ask_waiting.borrow();
        let mut ask_open = true;
        if ask_paused {
            timer.as_mut().reset(tokio::time::Instant::now() + parked);
        }
        let mut blocks = Vec::new();
        let mut started = None;
        let mut finished = None;
        let mut events_open = true;
        let mut streamed_bytes = 0_usize;
        let mut native_completion_in_flight = None;
        loop {
            tokio::select! {
                biased;
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow_and_update() != cancellation_generation {
                        return Err(ProviderRuntimeError::Cancelled);
                    }
                }
                changed = ask_waiting.changed(), if ask_open => {
                    if changed.is_err() {
                        ask_open = false;
                        continue;
                    }
                    let now_paused = *ask_waiting.borrow_and_update();
                    if now_paused && !ask_paused {
                        remaining = remaining.map(|value| value.saturating_sub(active_started.elapsed()));
                        timer.as_mut().reset(tokio::time::Instant::now() + parked);
                    } else if !now_paused && ask_paused {
                        active_started = tokio::time::Instant::now();
                        if let Some(value) = remaining {
                            timer.as_mut().reset(active_started + value);
                        }
                    }
                    ask_paused = now_paused;
                }
                _ = &mut timer, if remaining.is_some() && !ask_paused => {
                    return Err(ProviderRuntimeError::TimedOut);
                }
                published = async {
                    match tool_events.as_mut() {
                        Some(events) => events.next().await,
                        None => std::future::pending().await,
                    }
                }, if tool_events.is_some() => published?,
                event = events_rx.recv(), if events_open => {
                    let Some(event) = event else { events_open = false; continue };
                    handle_event(
                        event,
                        EventContext {
                            identity: &expected_identity,
                            sink,
                            blocks: &mut blocks,
                            started: &mut started,
                            finished: &mut finished,
                            streamed_bytes: &mut streamed_bytes,
                            max_output_bytes,
                            expected_provider,
                            native_tool_events: if native_session {
                                tool_events.as_deref()
                            } else {
                                None
                            },
                            native_completion_in_flight: &mut native_completion_in_flight,
                        },
                    )?;
                }
                outcome = &mut step => {
                    if let Some(events) = tool_events.as_mut() {
                        events.drain_ready()?;
                        events.check_fatal()?;
                    }
                    events_rx.close();
                    while let Ok(event) = events_rx.try_recv() {
                        handle_event(
                            event,
                            EventContext {
                                identity: &expected_identity,
                                sink,
                                blocks: &mut blocks,
                                started: &mut started,
                                finished: &mut finished,
                                streamed_bytes: &mut streamed_bytes,
                                max_output_bytes,
                                expected_provider,
                                native_tool_events: if native_session {
                                    tool_events.as_deref()
                                } else {
                                    None
                                },
                                native_completion_in_flight: &mut native_completion_in_flight,
                            },
                        )?;
                    }
                    if !ask_paused {
                        remaining = remaining.map(|value| value.saturating_sub(active_started.elapsed()));
                    }
                    return Ok(StepResult {
                        outcome: outcome?,
                        blocks,
                        finished,
                        remaining_active: remaining,
                        native_completion_in_flight,
                    });
                }
            }
        }
    }
}

pub(crate) struct StepResult {
    pub(crate) outcome: AdapterStepOutcome,
    pub(crate) blocks: Vec<NormalizedBlock>,
    pub(crate) finished: Option<(String, bool)>,
    pub(crate) remaining_active: Option<Duration>,
    pub(crate) native_completion_in_flight: Option<usize>,
}
