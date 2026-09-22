use std::{sync::Arc, time::Duration};

use parking_lot::Mutex;

use super::{
    tool_batch::{dispatch_tool_batch, tool_batch_coordinates},
    tool_events::RunToolEvents,
    ProviderRuntime, RuntimeEventSink,
};
use crate::{
    mcp,
    provider::ProviderConversationState,
    provider_runtime::{
        AdapterEventKind, AdapterLoopKind, AdapterOutput, AdapterRequestKind, AdapterStepOutcome,
        AdapterStepRequest, AgentTurnInput, ConversationItem, DelegatedRunOutcome,
        DelegatedRunRequest, IterationBoundaryReason, NormalizedBlock, ProviderAdapter,
        ProviderRuntimeError, WorkspaceAccess,
    },
    provider_tool_loop::{validate_structured_output, DirectToolResult, RunGate},
    tool_exec::SessionToolRuntime,
};

/// The user message that opens a delegated run's final, submission-only round.
const FINAL_ROUND_NOTICE: &str = "[tool budget]\nThis run has used its tool rounds. No further reads execute; only submit_result is available. Submit the result now from what you have already read, stating any gap in it.";

/// Runs one isolated model context over a read-only tool profile until the
/// model calls `submit_result`. Shared by engine-owned workflow stages and by
/// model-invoked read delegation; only the caller and the profile differ.
///
/// Compared with the foreground it keeps no transcript checkpoint, no write
/// turn recorder, no native recovery receipt, and no continuation: history is
/// accumulated in memory for direct adapters and every native run starts a
/// fresh CLI session. Its text and reasoning never reach the session stream;
/// tool calls and results do, through the gate's ordinary event path.
pub struct DelegatedRunExecutor {
    adapter: Box<dyn ProviderAdapter>,
    tools: SessionToolRuntime,
    events: Arc<dyn RuntimeEventSink>,
    cancellation: tokio::sync::watch::Receiver<u64>,
}

/// Records provider usage for the run's outcome and drops the run's own
/// text/reasoning so it never enters the session stream.
struct UsageCapture {
    usage: Mutex<Option<crate::ipc::ContextUsage>>,
}

impl RuntimeEventSink for UsageCapture {
    fn emit(&self, event: &AdapterEventKind) -> Result<(), ProviderRuntimeError> {
        if let AdapterEventKind::Usage(usage) = event {
            if let Some(context_usage) = &usage.context_usage {
                *self.usage.lock() = Some(context_usage.clone());
            }
        }
        Ok(())
    }
}

impl DelegatedRunExecutor {
    pub fn new(
        adapter: Box<dyn ProviderAdapter>,
        tools: SessionToolRuntime,
        events: Arc<dyn RuntimeEventSink>,
        cancellation: tokio::sync::watch::Receiver<u64>,
    ) -> Self {
        Self {
            adapter,
            tools,
            events,
            cancellation,
        }
    }

    pub async fn run(&mut self, request: DelegatedRunRequest) -> DelegatedRunOutcome {
        let run_started = tokio::time::Instant::now();
        if *self.cancellation.borrow() != request.identity.cancellation_generation {
            return DelegatedRunOutcome::Cancelled;
        }
        if self.adapter.provider() != request.binding.provider {
            return DelegatedRunOutcome::Failed(ProviderRuntimeError::InvalidBinding(
                "delegated request does not match its adapter".into(),
            ));
        }
        if request.identity.session_id != self.tools.session_id() {
            return DelegatedRunOutcome::Failed(ProviderRuntimeError::InvalidBinding(
                "delegated request does not match its session".into(),
            ));
        }
        if request.identity.session_kind != crate::session::SessionKind::Eps {
            return DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
                "delegated runs are available on EPS sessions only".into(),
            ));
        }
        if !self.tools.matches_run_scope(&request.identity) {
            return DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
                "delegated run requires an open request on its session".into(),
            ));
        }
        if !request.allow_live_write_ticket && self.tools.write_ticket().is_some() {
            return DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
                "delegated run refused: the session holds a live write ticket".into(),
            ));
        }
        let mut binding = request.binding.clone();
        binding.conversation = ProviderConversationState::empty(binding.provider);
        let direct = self.adapter.loop_kind() == AdapterLoopKind::DirectSteps;
        let gate = RunGate::delegated(
            request.identity.clone(),
            self.tools.clone(),
            request.profile.clone(),
        );
        let _lifetime = super::shutdown::RunLifetimeGuard(gate.clone());
        let mut tool_events = match RunToolEvents::new(&gate, self.events.clone()) {
            Ok(events) => events,
            Err(error) => return DelegatedRunOutcome::Failed(error),
        };
        let mut mcp = if direct {
            None
        } else {
            match mcp::serve(gate.clone()).await {
                Ok(server) => Some(server),
                Err(error) => {
                    return DelegatedRunOutcome::Failed(ProviderRuntimeError::Transport(error))
                }
            }
        };
        let endpoint = mcp.as_ref().map(|server| server.endpoint().to_owned());
        let usage = Arc::new(UsageCapture {
            usage: Mutex::new(None),
        });
        let turn = AgentTurnInput {
            text: request.prompt.clone(),
            image_paths: Vec::new(),
            workspace_root: Some(request.workspace_root.clone()),
            workspace_temp: request.workspace_temp.clone(),
            workspace_access: WorkspaceAccess::Read,
            output_schema: None,
            forbid_tools: false,
        };
        let mut history: Vec<ConversationItem> = vec![ConversationItem::User {
            request_id: request.identity.request_id.clone(),
            text: request.prompt.clone(),
            images: Vec::new(),
        }];
        let mut prior_results: Arc<[DirectToolResult]> = Arc::from([]);
        let mut continuation = None;
        let ask_waiting = self.tools.subscribe_ask_waiting();
        let mut active_remaining = request
            .policy
            .active_deadline
            .map(|deadline| deadline.saturating_sub(run_started.elapsed()));
        let mut active_mark = tokio::time::Instant::now();
        let grace = request.policy.shutdown_grace;

        macro_rules! stop_with {
            ($outcome:expr) => {{
                let outcome = $outcome;
                self.stop(&request.identity, &gate, &mut mcp, &mut tool_events, grace)
                    .await;
                return outcome;
            }};
        }

        let max_tool_rounds = request.policy.max_tool_rounds;
        for round in 0..max_tool_rounds {
            // Soft bound: the last round after at least one tool round is
            // submission-only. The model is shown `submit_result` alone and
            // told why, so whatever it has read so far still becomes the
            // result instead of an exhausted run with nothing to show. Native
            // runs settle inside their first step, so only direct steps ever
            // reach this round.
            if direct && round > 0 && round + 1 == max_tool_rounds {
                gate.enter_submission_only();
                history.push(ConversationItem::User {
                    request_id: request.identity.request_id.clone(),
                    text: FINAL_ROUND_NOTICE.to_string(),
                    images: Vec::new(),
                });
            }
            active_remaining =
                active_remaining.map(|remaining| remaining.saturating_sub(active_mark.elapsed()));
            let mut step_policy = request.policy.clone();
            step_policy.active_deadline = active_remaining;
            let step_request = AdapterStepRequest {
                identity: request.identity.clone(),
                binding: binding.clone(),
                kind: AdapterRequestKind::Foreground(turn.clone()),
                policy: step_policy,
                continuation: continuation.clone(),
                history: Arc::from(history.clone()),
                prior_tool_results: prior_results.clone(),
                tool_descriptors: if direct {
                    Arc::from(gate.advertised_descriptors())
                } else {
                    Arc::from([])
                },
                native_mcp_endpoint: endpoint.clone(),
                cancellation: self.cancellation.clone(),
            };
            let step = ProviderRuntime::run_adapter_step(
                self.adapter.as_mut(),
                step_request,
                Some(&mut tool_events),
                Some(usage.as_ref()),
                ask_waiting.clone(),
            )
            .await;
            let step = match step {
                Ok(step) => step,
                Err(ProviderRuntimeError::Cancelled) => stop_with!(DelegatedRunOutcome::Cancelled),
                // The accepted submission is the run's result; a native CLI
                // that then runs out its deadline while wrapping up its turn
                // (final text, late reads) does not lose it.
                Err(ProviderRuntimeError::TimedOut) if gate.delegated_result().is_some() => {
                    let value = gate.delegated_result().expect("checked above");
                    stop_with!(Self::accept(&request, value, &gate, &usage))
                }
                Err(error) => stop_with!(DelegatedRunOutcome::Failed(error)),
            };
            active_remaining = step.remaining_active;
            active_mark = tokio::time::Instant::now();
            if matches!(&step.outcome, AdapterStepOutcome::Cancelled) {
                stop_with!(DelegatedRunOutcome::Cancelled);
            }
            if !step
                .finished
                .as_ref()
                .is_some_and(|(_, complete)| *complete)
            {
                stop_with!(DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
                    "delegated response ended without a complete boundary".into(),
                )));
            }
            let continuation_candidate = match &step.outcome {
                AdapterStepOutcome::NeedsTools { continuation, .. }
                | AdapterStepOutcome::Completed { continuation, .. } => continuation.as_ref(),
                AdapterStepOutcome::Cancelled => None,
            };
            if let Some(candidate) = continuation_candidate {
                if let Err(error) = candidate.validate(binding.provider) {
                    stop_with!(DelegatedRunOutcome::Failed(error));
                }
            }
            match step.outcome {
                AdapterStepOutcome::NeedsTools {
                    calls,
                    continuation: next,
                } => {
                    if !direct {
                        stop_with!(DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
                            "native adapter returned a direct tool batch".into(),
                        )));
                    }
                    let (response_id, batch_id) = match tool_batch_coordinates(&step.blocks, &calls)
                    {
                        Ok(coordinates) => coordinates,
                        Err(error) => stop_with!(DelegatedRunOutcome::Failed(error)),
                    };
                    history.extend(step.blocks.iter().cloned().map(ConversationItem::Assistant));
                    active_remaining = active_remaining
                        .map(|remaining| remaining.saturating_sub(active_mark.elapsed()));
                    tool_events.set_batch(&response_id, &batch_id);
                    let batch = match dispatch_tool_batch(
                        &gate,
                        &mut tool_events,
                        calls,
                        active_remaining,
                        self.cancellation.clone(),
                        request.identity.cancellation_generation,
                        ask_waiting.clone(),
                    )
                    .await
                    {
                        Ok((batch, remaining)) => {
                            active_remaining = remaining;
                            active_mark = tokio::time::Instant::now();
                            batch
                        }
                        Err(ProviderRuntimeError::Cancelled) => {
                            stop_with!(DelegatedRunOutcome::Cancelled)
                        }
                        Err(error) => stop_with!(DelegatedRunOutcome::Failed(error)),
                    };
                    history.extend(batch.results.iter().cloned().map(|result| {
                        ConversationItem::Assistant(NormalizedBlock::ToolResult {
                            response_id: response_id.clone(),
                            batch_id: batch_id.clone(),
                            result,
                        })
                    }));
                    prior_results = Arc::from(batch.results);
                    continuation = next;
                    if let Some(reason) = gate.iteration_boundary_reason() {
                        // A user pause or the request action boundary ends the
                        // whole delegated run; it is never resumed.
                        stop_with!(match reason {
                            IterationBoundaryReason::ProviderContinuation => {
                                DelegatedRunOutcome::Cancelled
                            }
                            other => DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
                                format!("delegated run stopped at iteration boundary {other:?}"),
                            )),
                        });
                    }
                    if let Some(value) = gate.delegated_result() {
                        if let Err(error) = ProviderRuntime::finish_run(
                            &gate,
                            &mut mcp,
                            &mut tool_events,
                            grace,
                            None,
                        )
                        .await
                        {
                            return DelegatedRunOutcome::Failed(error);
                        }
                        return Self::accept(&request, value, &gate, &usage);
                    }
                }
                AdapterStepOutcome::Completed { output, .. } => {
                    if let AdapterOutput::Structured(_) = output {
                        stop_with!(DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
                            "delegated provider returned structured job output".into(),
                        )));
                    }
                    if let Err(error) = ProviderRuntime::finish_run(
                        &gate,
                        &mut mcp,
                        &mut tool_events,
                        grace,
                        step.native_completion_in_flight,
                    )
                    .await
                    {
                        return DelegatedRunOutcome::Failed(error);
                    }
                    return match gate.delegated_result() {
                        Some(value) => Self::accept(&request, value, &gate, &usage),
                        None => DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
                            "delegated run ended without submit_result".into(),
                        )),
                    };
                }
                AdapterStepOutcome::Cancelled => stop_with!(DelegatedRunOutcome::Cancelled),
            }
        }
        stop_with!(DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(
            "delegated run exhausted its tool rounds without submit_result".into(),
        )))
    }

    fn accept(
        request: &DelegatedRunRequest,
        value: serde_json::Value,
        gate: &RunGate,
        usage: &UsageCapture,
    ) -> DelegatedRunOutcome {
        // The gate admitted the submission against the same schema, so this
        // only re-checks the byte policy and guards against a drifted profile.
        let size = serde_json::to_vec(&value).map_or(usize::MAX, |bytes| bytes.len());
        if size > request.policy.max_output_bytes
            || validate_structured_output(&request.output_schema, &value).is_err()
        {
            return DelegatedRunOutcome::Failed(ProviderRuntimeError::StructuredOutputInvalid);
        }
        DelegatedRunOutcome::Result {
            value,
            completions: gate.completed().len(),
            usage: usage.usage.lock().clone(),
        }
    }

    /// Close admission, interrupt the provider, and settle the MCP server and
    /// tool events within one shared `grace` budget, so cancellation returns
    /// within `shutdown_grace` rather than three times it.
    async fn stop(
        &mut self,
        identity: &crate::provider_runtime::RunIdentity,
        gate: &RunGate,
        mcp: &mut Option<mcp::McpServerHandle>,
        tool_events: &mut RunToolEvents,
        grace: Duration,
    ) {
        let deadline = tokio::time::Instant::now() + grace;
        let remaining = || deadline.saturating_duration_since(tokio::time::Instant::now());
        gate.cancel();
        let _ = tokio::time::timeout_at(deadline, self.adapter.interrupt(identity)).await;
        if let Some(server) = mcp.as_mut() {
            let _ = server.close_and_drain(remaining()).await;
        }
        let _ = tool_events.drain(remaining()).await;
    }
}
