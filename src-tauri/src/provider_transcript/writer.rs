use super::*;

impl RunCheckpointWriter {
    pub fn commit_context(
        &self,
        request_id: &str,
        text: &str,
        images: &[crate::provider_runtime::ConversationImage],
    ) -> Result<u64, String> {
        validate_identifier(request_id, "request id")?;
        let mut state = self.state.lock();
        if state.active_batch.is_some() {
            return Err(
                "provider transcript cannot commit context while a tool batch is active"
                    .to_string(),
            );
        }
        let mut blocks = state.blocks.clone();
        blocks.push(TranscriptBlock::User {
            request_id: request_id.to_string(),
            text: text.to_string(),
            images: images
                .iter()
                .map(|image| TranscriptImage {
                    id: image.id.clone(),
                    mime_type: image.mime.clone(),
                    data_base64: image.data_base64.clone(),
                })
                .collect(),
        });
        let generation = self.store.publish_checkpoint(
            self.provider,
            &self.session_id,
            state.revision,
            TranscriptCheckpoint {
                branch: self.branch.clone(),
                blocks,
                boundary: CheckpointBoundary::ContextCommitted {
                    request_id: request_id.to_string(),
                },
                continuation: None,
            },
        )?;
        state.revision = generation.revision;
        state.blocks = generation.checkpoint.blocks;
        Ok(state.revision)
    }

    pub fn commit_response(
        &self,
        response_id: &str,
        blocks: &[crate::provider_runtime::NormalizedBlock],
        continuation: Option<crate::provider_runtime::ProviderContinuation>,
    ) -> Result<u64, String> {
        validate_identifier(response_id, "response id")?;
        let mut state = self.state.lock();
        if state.active_batch.is_some() {
            return Err(
                "provider transcript cannot complete a response while a tool batch is active"
                    .to_string(),
            );
        }
        let mut transcript_blocks = state.blocks.clone();
        for block in blocks {
            match block {
                crate::provider_runtime::NormalizedBlock::Text {
                    response_id: block_response,
                    text,
                } if block_response == response_id => {
                    transcript_blocks.push(TranscriptBlock::AssistantText {
                        response_id: block_response.clone(),
                        text: text.clone(),
                    });
                }
                crate::provider_runtime::NormalizedBlock::Reasoning {
                    response_id: block_response,
                    text,
                    continuation,
                } if block_response == response_id => {
                    transcript_blocks.push(TranscriptBlock::AssistantReasoning {
                        response_id: block_response.clone(),
                        text: text.clone(),
                        continuation: continuation.clone(),
                    });
                }
                crate::provider_runtime::NormalizedBlock::Text { .. }
                | crate::provider_runtime::NormalizedBlock::Reasoning { .. } => {
                    return Err("provider transcript response block identity mismatch".to_string());
                }
                crate::provider_runtime::NormalizedBlock::ToolCall { .. }
                | crate::provider_runtime::NormalizedBlock::ToolResult { .. } => {
                    return Err(
                        "provider transcript response commit received tool blocks twice"
                            .to_string(),
                    );
                }
            }
        }
        let generation = self.store.publish_checkpoint(
            self.provider,
            &self.session_id,
            state.revision,
            TranscriptCheckpoint {
                branch: self.branch.clone(),
                blocks: transcript_blocks,
                boundary: CheckpointBoundary::ResponseCompleted {
                    response_id: response_id.to_string(),
                },
                continuation,
            },
        )?;
        state.revision = generation.revision;
        state.blocks = generation.checkpoint.blocks;
        state.active_batch = None;
        Ok(state.revision)
    }

    pub fn mark_batch_resumable(
        &self,
        response_id: &str,
        batch_id: &str,
        continuation: Option<crate::provider_runtime::ProviderContinuation>,
    ) -> Result<u64, String> {
        let mut state = self.state.lock();
        let active = state
            .active_batch
            .as_ref()
            .ok_or_else(|| "provider transcript tool batch is not active".to_string())?;
        if active.response_id != response_id || active.batch_id != batch_id {
            return Err("provider transcript tool batch identity mismatch".to_string());
        }
        let checkpoint = TranscriptCheckpoint {
            branch: self.branch.clone(),
            blocks: state.blocks.clone(),
            boundary: CheckpointBoundary::CompletedToolBatch {
                response_id: response_id.to_string(),
                batch_id: batch_id.to_string(),
            },
            continuation,
        };
        let generation = self.store.publish_checkpoint(
            self.provider,
            &self.session_id,
            state.revision,
            checkpoint,
        )?;
        state.revision = generation.revision;
        state.blocks = generation.checkpoint.blocks;
        state.active_batch = None;
        Ok(state.revision)
    }

    pub fn mark_write_transition_resumable(
        &self,
        response_id: &str,
        batch_id: &str,
        executed_call_ids: &[String],
        observed_continuation: Option<crate::provider_runtime::ProviderContinuation>,
    ) -> Result<u64, String> {
        let mut state = self.state.lock();
        let active = state
            .active_batch
            .as_ref()
            .ok_or_else(|| "provider transcript tool batch is not active".to_string())?;
        if active.response_id != response_id || active.batch_id != batch_id {
            return Err("provider transcript write transition identity mismatch".to_string());
        }
        let executed = executed_call_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut blocks = state.blocks.clone();
        for block in &mut blocks {
            if let TranscriptBlock::ToolCall {
                response_id: block_response,
                batch_id: block_batch,
                id,
                audit_only,
                ..
            } = block
            {
                if block_response == response_id && block_batch == batch_id {
                    *audit_only = !executed.contains(id.as_str());
                }
            }
        }
        let checkpoint = TranscriptCheckpoint {
            branch: self.branch.clone(),
            blocks,
            boundary: CheckpointBoundary::WriteTransition {
                response_id: response_id.to_string(),
                batch_id: batch_id.to_string(),
                executed_call_ids: executed_call_ids.to_vec(),
                observed_continuation,
            },
            continuation: None,
        };
        let generation = self.store.publish_checkpoint(
            self.provider,
            &self.session_id,
            state.revision,
            checkpoint,
        )?;
        state.revision = generation.revision;
        state.blocks = generation.checkpoint.blocks;
        state.active_batch = None;
        Ok(state.revision)
    }

    pub fn history(&self) -> Vec<crate::provider_runtime::ConversationItem> {
        let state = self.state.lock();
        transcript_history(&state.blocks)
    }

    pub fn revision(&self) -> u64 {
        self.state.lock().revision
    }

    pub fn branch(&self) -> TranscriptBranch {
        self.branch.clone()
    }
}
