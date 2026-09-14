use super::{
    shutdown::RunLifetimeGuard,
    tool_batch::{dispatch_tool_batch, tool_batch_coordinates},
    tool_events::RunToolEvents,
    ProviderRuntime,
};
use crate::{
    mcp,
    provider_runtime::{
        AdapterLoopKind, AdapterOutput, AdapterRequestKind, AdapterStepOutcome, AdapterStepRequest,
        ConversationItem, ForegroundRequest, ProviderRuntimeError, RunOutcome,
    },
    provider_tool_loop::RunGate,
};
use std::sync::Arc;

// allow: SIZE_OK — this foreground state machine keeps the gate, MCP server,
// transcript writer, and workspace recorder within one cancellation lifetime.
impl ProviderRuntime {
    pub(super) fn foreground(
        &mut self,
        mut request: ForegroundRequest,
    ) -> super::AdapterFuture<'_, RunOutcome> {
        Box::pin(async move {
            let run_started = tokio::time::Instant::now();
            if *self.cancellation.borrow() != request.identity.cancellation_generation {
                return RunOutcome::Cancelled;
            }
            if request.binding.provider != self.binding.provider
                || request.binding.model != self.binding.model
                || request.binding.reasoning != self.binding.reasoning
                || request.binding.base_url != self.binding.base_url
                || request.binding.capabilities != self.binding.capabilities
                || request.identity.session_id != self.tools.session_id()
            {
                return RunOutcome::Failed(ProviderRuntimeError::InvalidBinding(
                    "foreground request does not match its runtime".into(),
                ));
            }
            if request.turn.workspace_access == super::WorkspaceAccess::Write
                && !self.tools.owns_write_registration()
            {
                return RunOutcome::Failed(ProviderRuntimeError::Protocol(
                    "write execution requires an active workspace registration".into(),
                ));
            }
            let workspace = match self.prepare_foreground_workspace(&mut request).await {
                Ok(workspace) => workspace,
                Err(error) => return RunOutcome::Failed(error),
            };
            let mut recorder = if request.turn.workspace_access == super::WorkspaceAccess::Write {
                match self
                    .workspace
                    .begin_turn(&workspace, &request.identity.request_id)
                {
                    Ok(baseline) => Some(crate::workspace::WorkspaceTurnRecorder::new(
                        self.workspace.clone(),
                        baseline,
                        self.tools.journal().clone(),
                    )),
                    Err(error) => {
                        return RunOutcome::Failed(ProviderRuntimeError::Transport(
                            error.to_string(),
                        ))
                    }
                }
            } else {
                None
            };
            let direct = self.adapter.loop_kind() == AdapterLoopKind::DirectSteps;
            if !direct {
                if let Err(error) = self.recover_native_conversation(&request.identity).await {
                    return RunOutcome::Failed(error);
                }
                request.binding.conversation = self.conversation.clone();
            }
            let (writer, mut continuation) = if direct {
                match self.direct_writer(&request) {
                    Ok((writer, continuation)) => (Some(writer), continuation),
                    Err(error) => return RunOutcome::Failed(error),
                }
            } else {
                (None, None)
            };
            if let Some(writer) = writer.as_ref() {
                let images = match crate::provider_transcript::conversation_images(
                    &request.turn.image_paths,
                ) {
                    Ok(images) => images,
                    Err(error) => return RunOutcome::Failed(ProviderRuntimeError::Protocol(error)),
                };
                if let Err(error) =
                    writer.commit_context(&request.identity.request_id, &request.turn.text, &images)
                {
                    return RunOutcome::Failed(ProviderRuntimeError::Protocol(error));
                }
            }
            let gate = RunGate::new(
                request.identity.clone(),
                self.tools.clone(),
                request.turn.workspace_access,
                writer.clone(),
            );
            let _lifetime = RunLifetimeGuard(gate.clone());
            let mut tool_events = match RunToolEvents::new(&gate, self.events.clone()) {
                Ok(events) => events,
                Err(error) => return RunOutcome::Failed(error),
            };
            if !direct {
                if let Err(error) = gate.begin_native_run(
                    self.binding.provider,
                    self.conversation.conversation_key().as_deref(),
                ) {
                    return RunOutcome::Failed(ProviderRuntimeError::Transport(error));
                }
            }
            let mut mcp = if self.adapter.loop_kind() == AdapterLoopKind::NativeSession {
                match mcp::serve(gate.clone()).await {
                    Ok(server) => Some(server),
                    Err(error) => {
                        return RunOutcome::Failed(ProviderRuntimeError::Transport(error))
                    }
                }
            } else {
                None
            };
            macro_rules! stop_failed {
                ($error:expr) => {{
                    let error = $error;
                    self.stop_run(
                        &request.identity,
                        &gate,
                        &mut mcp,
                        &mut tool_events,
                        request.policy.shutdown_grace,
                    )
                    .await;
                    return RunOutcome::Failed(error);
                }};
            }
            let endpoint = mcp.as_ref().map(|server| server.endpoint().to_owned());
            let mut prior_results: Arc<[crate::provider_tool_loop::DirectToolResult]> =
                Arc::from([]);
            let ask_waiting = self.tools.subscribe_ask_waiting();
            let mut active_remaining = request
                .policy
                .active_deadline
                .map(|deadline| deadline.saturating_sub(run_started.elapsed()));
            let mut active_mark = tokio::time::Instant::now();
            for _ in 0..request.policy.max_tool_rounds {
                let history: Arc<[ConversationItem]> = writer
                    .as_ref()
                    .map(|writer| Arc::from(writer.history()))
                    .unwrap_or_else(|| Arc::from([]));
                active_remaining = active_remaining
                    .map(|remaining| remaining.saturating_sub(active_mark.elapsed()));
                let mut step_policy = request.policy.clone();
                step_policy.active_deadline = active_remaining;
                let step_request = AdapterStepRequest {
                    identity: request.identity.clone(),
                    binding: request.binding.clone(),
                    kind: AdapterRequestKind::Foreground(request.turn.clone()),
                    policy: step_policy,
                    continuation: continuation.clone(),
                    history,
                    prior_tool_results: prior_results.clone(),
                    tool_descriptors: match self.adapter.loop_kind() {
                        AdapterLoopKind::DirectSteps => Arc::from(gate.descriptors()),
                        AdapterLoopKind::NativeSession => Arc::from([]),
                    },
                    native_mcp_endpoint: endpoint.clone(),
                    cancellation: self.cancellation.clone(),
                };
                let step = Self::run_adapter_step(
                    self.adapter.as_mut(),
                    step_request,
                    Some(&mut tool_events),
                    Some(self.events.as_ref()),
                    ask_waiting.clone(),
                )
                .await;
                let step = match step {
                    Ok(step) => step,
                    Err(error) => {
                        self.stop_run(
                            &request.identity,
                            &gate,
                            &mut mcp,
                            &mut tool_events,
                            request.policy.shutdown_grace,
                        )
                        .await;
                        return match error {
                            ProviderRuntimeError::Cancelled => RunOutcome::Cancelled,
                            other => RunOutcome::Failed(other),
                        };
                    }
                };
                active_remaining = step.remaining_active;
                active_mark = tokio::time::Instant::now();
                if matches!(&step.outcome, AdapterStepOutcome::Cancelled) {
                    self.stop_run(
                        &request.identity,
                        &gate,
                        &mut mcp,
                        &mut tool_events,
                        request.policy.shutdown_grace,
                    )
                    .await;
                    return RunOutcome::Cancelled;
                }
                if !step
                    .finished
                    .as_ref()
                    .is_some_and(|(_, complete)| *complete)
                {
                    self.stop_run(
                        &request.identity,
                        &gate,
                        &mut mcp,
                        &mut tool_events,
                        request.policy.shutdown_grace,
                    )
                    .await;
                    return RunOutcome::Failed(ProviderRuntimeError::Protocol(
                        "provider response ended without a complete boundary".into(),
                    ));
                }
                let continuation_candidate = match &step.outcome {
                    AdapterStepOutcome::NeedsTools { continuation, .. }
                    | AdapterStepOutcome::Completed { continuation, .. } => continuation.as_ref(),
                    AdapterStepOutcome::Cancelled => None,
                };
                if let Some(continuation) = continuation_candidate {
                    if let Err(error) = continuation.validate(self.binding.provider) {
                        stop_failed!(error);
                    }
                }
                if let AdapterStepOutcome::Completed {
                    native_conversation: Some(conversation),
                    ..
                } = &step.outcome
                {
                    if conversation.provider() != self.binding.provider {
                        stop_failed!(ProviderRuntimeError::InvalidBinding(
                            "provider returned a conversation for another provider".into()
                        ));
                    }
                }
                match step.outcome {
                    AdapterStepOutcome::NeedsTools {
                        calls,
                        continuation: next,
                    } => {
                        if !direct {
                            stop_failed!(ProviderRuntimeError::Protocol(
                                "native adapter returned a direct tool batch".into()
                            ));
                        }
                        let (response_id, batch_id) =
                            match tool_batch_coordinates(&step.blocks, &calls) {
                                Ok(coordinates) => coordinates,
                                Err(error) => stop_failed!(error),
                            };
                        let Some(writer) = writer.as_ref() else {
                            stop_failed!(ProviderRuntimeError::Protocol(
                                "direct provider checkpoint is unavailable".into()
                            ));
                        };
                        if let Err(error) =
                            writer.begin_tool_batch(&response_id, &batch_id, &step.blocks)
                        {
                            stop_failed!(ProviderRuntimeError::Protocol(error));
                        }
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
                            Err(error) => {
                                self.stop_run(
                                    &request.identity,
                                    &gate,
                                    &mut mcp,
                                    &mut tool_events,
                                    request.policy.shutdown_grace,
                                )
                                .await;
                                return match error {
                                    ProviderRuntimeError::Cancelled => RunOutcome::Cancelled,
                                    other => RunOutcome::Failed(other),
                                };
                            }
                        };
                        if batch.stop_for_write_transition {
                            if let Err(error) = Self::finish_run(
                                &gate,
                                &mut mcp,
                                &mut tool_events,
                                request.policy.shutdown_grace,
                                step.native_completion_in_flight,
                            )
                            .await
                            {
                                stop_failed!(error);
                            }
                            let executed_call_ids = batch
                                .results
                                .iter()
                                .map(|result| result.id.clone())
                                .collect::<Vec<_>>();
                            if let Err(error) = writer.mark_write_transition_resumable(
                                &response_id,
                                &batch_id,
                                &executed_call_ids,
                                next,
                            ) {
                                stop_failed!(ProviderRuntimeError::Protocol(error));
                            }
                            let conversation = self.direct_conversation(writer.revision());
                            self.conversation = conversation.clone();
                            self.binding.conversation = conversation;
                            return RunOutcome::WriteTransition;
                        }
                        if let Err(error) =
                            writer.mark_batch_resumable(&response_id, &batch_id, next.clone())
                        {
                            stop_failed!(ProviderRuntimeError::Protocol(error));
                        }
                        prior_results = Arc::from(batch.results);
                        continuation = next;
                    }
                    AdapterStepOutcome::Completed {
                        output,
                        continuation: next,
                        native_conversation,
                    } => {
                        let AdapterOutput::Text(text) = output else {
                            stop_failed!(ProviderRuntimeError::Protocol(
                                "foreground provider returned structured job output".into()
                            ));
                        };
                        if text.len() > request.policy.max_output_bytes {
                            stop_failed!(ProviderRuntimeError::Protocol(
                                "provider output exceeded its byte limit".into()
                            ));
                        }
                        if let Err(error) = Self::finish_run(
                            &gate,
                            &mut mcp,
                            &mut tool_events,
                            request.policy.shutdown_grace,
                            step.native_completion_in_flight,
                        )
                        .await
                        {
                            stop_failed!(error);
                        }
                        let conversation = if let Some(writer) = writer.as_ref() {
                            let response_id = step
                                .finished
                                .as_ref()
                                .map(|(response_id, _)| response_id.as_str())
                                .unwrap_or_default();
                            if let Err(error) =
                                writer.commit_response(response_id, &step.blocks, next)
                            {
                                stop_failed!(ProviderRuntimeError::Protocol(error));
                            }
                            self.direct_conversation(writer.revision())
                        } else {
                            let Some(conversation) = native_conversation else {
                                stop_failed!(ProviderRuntimeError::Protocol(
                                    "native provider omitted its confirmed conversation checkpoint"
                                        .into()
                                ));
                            };
                            if !conversation.is_started() {
                                stop_failed!(ProviderRuntimeError::Protocol(
                                    "native provider returned an empty conversation checkpoint"
                                        .into()
                                ));
                            }
                            conversation
                        };
                        self.conversation = conversation.clone();
                        self.binding.conversation = conversation.clone();
                        if let Some(recorder) = recorder.as_mut() {
                            if let Err(error) = recorder.finish() {
                                return RunOutcome::Failed(ProviderRuntimeError::Transport(
                                    error.to_string(),
                                ));
                            }
                        }
                        if !direct {
                            if let Err(error) = gate.mark_native_run_completed(
                                conversation.conversation_key().as_deref(),
                            ) {
                                return RunOutcome::Failed(ProviderRuntimeError::Transport(error));
                            }
                            self.pending_acknowledgements.push(request.identity.clone());
                            if gate.has_granted_write_transition() {
                                return RunOutcome::WriteTransition;
                            }
                        }
                        return RunOutcome::Completed { text, conversation };
                    }
                    AdapterStepOutcome::Cancelled => {
                        self.stop_run(
                            &request.identity,
                            &gate,
                            &mut mcp,
                            &mut tool_events,
                            request.policy.shutdown_grace,
                        )
                        .await;
                        return RunOutcome::Cancelled;
                    }
                }
            }
            self.stop_run(
                &request.identity,
                &gate,
                &mut mcp,
                &mut tool_events,
                request.policy.shutdown_grace,
            )
            .await;
            RunOutcome::Failed(ProviderRuntimeError::ToolRoundLimit)
        })
    }
}
