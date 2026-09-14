use super::{shutdown::RunLifetimeGuard, ProviderRuntime};
use crate::{
    provider::ProviderConversationState,
    provider_runtime::{
        AdapterLoopKind, AdapterOutput, AdapterRequestKind, AdapterStepOutcome, AdapterStepRequest,
        CompactionRequest, ProviderRuntimeError, StructuredJobKind,
    },
    provider_tool_loop::{validate_structured_output, RunGate},
    provider_transcript::{
        CheckpointBoundary, ProviderTranscriptStore, TranscriptBlock, TranscriptBranch,
        TranscriptCheckpoint,
    },
};
use std::sync::Arc;

impl ProviderRuntime {
    pub(super) fn compact_conversation(
        &mut self,
        mut request: CompactionRequest,
    ) -> super::AdapterFuture<'_, Result<ProviderConversationState, ProviderRuntimeError>> {
        Box::pin(async move {
            if request.binding.provider != self.binding.provider
                || request.binding.model != self.binding.model
                || request.binding.reasoning != self.binding.reasoning
                || request.binding.base_url != self.binding.base_url
            {
                return Err(ProviderRuntimeError::InvalidBinding(
                    "compaction request does not match its runtime".into(),
                ));
            }
            if self.adapter.loop_kind() == AdapterLoopKind::NativeSession {
                self.recover_native_conversation(&request.identity).await?;
                let preparation_started = tokio::time::Instant::now();
                Self::prepare_native_compaction(
                    self.adapter.as_mut(),
                    &request,
                    self.cancellation.clone(),
                )
                .await?;
                request.policy.active_deadline = request
                    .policy
                    .active_deadline
                    .map(|deadline| deadline.saturating_sub(preparation_started.elapsed()));
                let preparation_error =
                    if *self.cancellation.borrow() != request.identity.cancellation_generation {
                        Some(ProviderRuntimeError::Cancelled)
                    } else if request
                        .policy
                        .active_deadline
                        .is_some_and(|deadline| deadline.is_zero())
                    {
                        Some(ProviderRuntimeError::TimedOut)
                    } else {
                        None
                    };
                if let Some(error) = preparation_error {
                    self.adapter.seed(self.conversation.clone()).await?;
                    return Err(error);
                }
                let gate = RunGate::new(
                    request.identity.clone(),
                    self.tools.clone(),
                    crate::provider_runtime::WorkspaceAccess::Read,
                    None,
                );
                let _lifetime = RunLifetimeGuard(gate.clone());
                if let Err(error) = gate.begin_native_run(
                    self.binding.provider,
                    self.conversation.conversation_key().as_deref(),
                ) {
                    self.adapter.seed(self.conversation.clone()).await?;
                    return Err(ProviderRuntimeError::Transport(error));
                }
                let compacted = Self::run_native_compaction(
                    self.adapter.as_mut(),
                    &request,
                    self.events.as_ref(),
                    self.cancellation.clone(),
                )
                .await;
                let conversation = match compacted {
                    Ok(conversation) => conversation,
                    Err(error) => {
                        let _ = gate.mark_native_run_unknown();
                        let _ = tokio::time::timeout(
                            request.policy.shutdown_grace,
                            self.adapter.interrupt(&request.identity),
                        )
                        .await;
                        return Err(error);
                    }
                };
                if conversation.provider() != self.binding.provider || !conversation.is_started() {
                    return Err(ProviderRuntimeError::Protocol(
                        "native compaction returned an invalid conversation checkpoint".into(),
                    ));
                }
                gate.mark_native_run_completed(conversation.conversation_key().as_deref())
                    .map_err(ProviderRuntimeError::Transport)?;
                self.pending_acknowledgements.push(request.identity.clone());
                self.conversation = conversation.clone();
                self.binding.conversation = conversation.clone();
                return Ok(conversation);
            }
            let store = ProviderTranscriptStore::new(&self.dirs);
            let revision = self.direct_revision()?;
            let current_branch = TranscriptBranch {
                instruction_epoch: request.next_instruction_epoch.saturating_sub(1),
                task_leaf_id: None,
            };
            let restored = store
                .restore(
                    self.binding.provider,
                    &request.identity.session_id,
                    revision,
                    &current_branch,
                )
                .map_err(ProviderRuntimeError::Protocol)?
                .ok_or_else(|| {
                    ProviderRuntimeError::Protocol(
                        "provider transcript is unavailable for compaction".into(),
                    )
                })?;
            let writer = store
                .checkpoint_writer(
                    self.binding.provider,
                    &request.identity.session_id,
                    restored.generation.revision,
                    current_branch,
                    restored.generation.checkpoint.blocks,
                )
                .map_err(ProviderRuntimeError::Protocol)?;
            let schema = serde_json::json!({
                "type": "object",
                "properties": { "summary": { "type": "string", "minLength": 1 } },
                "required": ["summary"],
                "additionalProperties": false
            });
            let step_request = AdapterStepRequest {
                identity: request.identity.clone(),
                binding: request.binding,
                kind: AdapterRequestKind::Structured {
                    kind: StructuredJobKind::ConversationCompaction,
                    prompt: "Summarize the durable conversation state for exact continuation. Preserve decisions, completed tools, pending work, and user constraints.".to_string(),
                    workspace_root: request.workspace_root,
                    output_schema: schema.clone(),
                },
                policy: request.policy,
                continuation: restored.generation.checkpoint.continuation,
                history: Arc::from(writer.history()),
                prior_tool_results: Arc::from([]),
                tool_descriptors: Arc::from([]),
                native_mcp_endpoint: None,
                cancellation: self.cancellation.clone(),
            };
            let (_ask_sender, ask_waiting) = tokio::sync::watch::channel(false);
            let step = Self::run_adapter_step(
                self.adapter.as_mut(),
                step_request,
                None,
                None,
                ask_waiting,
            )
            .await?;
            let value = match step.outcome {
                AdapterStepOutcome::Completed {
                    output: AdapterOutput::Structured(value),
                    ..
                } if step.finished.is_some_and(|(_, complete)| complete) => value,
                AdapterStepOutcome::Completed { .. }
                | AdapterStepOutcome::NeedsTools { .. }
                | AdapterStepOutcome::Cancelled => {
                    return Err(ProviderRuntimeError::StructuredOutputInvalid)
                }
            };
            validate_structured_output(&schema, &value)
                .map_err(|_| ProviderRuntimeError::StructuredOutputInvalid)?;
            let summary = value
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .ok_or(ProviderRuntimeError::StructuredOutputInvalid)?;
            let previous_revision = writer.revision();
            let generation = store
                .publish_checkpoint(
                    self.binding.provider,
                    &request.identity.session_id,
                    previous_revision,
                    TranscriptCheckpoint {
                        branch: TranscriptBranch {
                            instruction_epoch: request.next_instruction_epoch,
                            task_leaf_id: None,
                        },
                        blocks: vec![TranscriptBlock::Compaction {
                            summary: summary.to_string(),
                            previous_revision,
                        }],
                        boundary: CheckpointBoundary::Compaction { previous_revision },
                        continuation: None,
                    },
                )
                .map_err(ProviderRuntimeError::Protocol)?;
            let conversation = self.direct_conversation(generation.revision);
            self.conversation = conversation.clone();
            self.binding.conversation = conversation.clone();
            Ok(conversation)
        })
    }
}
