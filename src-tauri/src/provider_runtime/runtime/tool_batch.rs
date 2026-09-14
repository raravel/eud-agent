use super::tool_events::RunToolEvents;
use crate::{
    provider_runtime::{NormalizedBlock, ProviderRuntimeError},
    provider_tool_loop::{DirectDispatchBatch, DirectToolCall, RunGate},
};
use std::time::Duration;

pub(super) async fn dispatch_tool_batch(
    gate: &RunGate,
    tool_events: &mut RunToolEvents,
    calls: Vec<DirectToolCall>,
    remaining: Option<Duration>,
    mut cancellation: tokio::sync::watch::Receiver<u64>,
    cancellation_generation: u64,
    mut ask_waiting: tokio::sync::watch::Receiver<bool>,
) -> Result<(DirectDispatchBatch, Option<Duration>), ProviderRuntimeError> {
    tool_events.check_fatal()?;
    let dispatch = gate.dispatch_batch(calls, false);
    tokio::pin!(dispatch);
    let parked = Duration::from_secs(365 * 24 * 60 * 60);
    let timer = tokio::time::sleep(remaining.unwrap_or(parked));
    tokio::pin!(timer);
    let mut active_started = tokio::time::Instant::now();
    let mut remaining = remaining;
    let mut ask_paused = *ask_waiting.borrow();
    let mut ask_open = true;
    if *cancellation.borrow() != cancellation_generation {
        return Err(ProviderRuntimeError::Cancelled);
    }
    if ask_paused {
        timer.as_mut().reset(tokio::time::Instant::now() + parked);
    }
    loop {
        tokio::select! {
            biased;
            changed = cancellation.changed() => {
                if changed.is_err()
                    || *cancellation.borrow_and_update() != cancellation_generation
                {
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
            published = tool_events.next() => published?,
            result = &mut dispatch => {
                tool_events.drain_ready()?;
                tool_events.check_fatal()?;
                if !ask_paused {
                    remaining = remaining.map(|value| value.saturating_sub(active_started.elapsed()));
                }
                return result
                    .map(|batch| (batch, remaining))
                    .map_err(ProviderRuntimeError::Protocol);
            }
        }
    }
}

pub(super) fn tool_batch_coordinates(
    blocks: &[NormalizedBlock],
    calls: &[DirectToolCall],
) -> Result<(String, String), ProviderRuntimeError> {
    let tool_blocks = blocks
        .iter()
        .filter_map(|block| match block {
            NormalizedBlock::ToolCall {
                response_id,
                batch_id,
                call,
                continuation,
            } => Some((response_id, batch_id, call, continuation)),
            NormalizedBlock::Text { .. }
            | NormalizedBlock::Reasoning { .. }
            | NormalizedBlock::ToolResult { .. } => None,
        })
        .collect::<Vec<_>>();
    if tool_blocks.len() != calls.len()
        || tool_blocks
            .iter()
            .zip(calls)
            .any(|((_, _, observed, _), expected)| observed != &expected)
    {
        return Err(ProviderRuntimeError::Protocol(
            "provider tool batch does not match streamed calls".into(),
        ));
    }
    let Some((response_id, batch_id, _, _)) = tool_blocks.first() else {
        return Err(ProviderRuntimeError::Protocol(
            "provider requested an empty tool batch".into(),
        ));
    };
    if tool_blocks
        .iter()
        .any(|(other_response, other_batch, _, _)| {
            other_response != response_id || other_batch != batch_id
        })
    {
        return Err(ProviderRuntimeError::Protocol(
            "provider tool batch coordinates changed within one response".into(),
        ));
    }
    Ok(((*response_id).clone(), (*batch_id).clone()))
}
