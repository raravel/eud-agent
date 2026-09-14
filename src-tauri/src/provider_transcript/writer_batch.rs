use super::*;

impl RunCheckpointWriter {
    pub fn begin_tool_batch(
        &self,
        response_id: &str,
        batch_id: &str,
        blocks: &[crate::provider_runtime::NormalizedBlock],
    ) -> Result<(), String> {
        validate_identifier(response_id, "response id")?;
        validate_identifier(batch_id, "batch id")?;
        let mut state = self.state.lock();
        if state.active_batch.is_some() {
            return Err("provider transcript tool batch is already active".to_string());
        }
        let mut transcript_blocks = state.blocks.clone();
        let mut call_count = 0_usize;
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
                    if let Some(continuation) = continuation {
                        validate_continuation(continuation, self.provider)?;
                    }
                    transcript_blocks.push(TranscriptBlock::AssistantReasoning {
                        response_id: block_response.clone(),
                        text: text.clone(),
                        continuation: continuation.clone(),
                    });
                }
                crate::provider_runtime::NormalizedBlock::ToolCall {
                    response_id: block_response,
                    batch_id: block_batch,
                    call,
                    continuation,
                } if block_response == response_id && block_batch == batch_id => {
                    if let Some(continuation) = continuation {
                        validate_continuation(continuation, self.provider)?;
                    }
                    validate_tool_coordinates(response_id, batch_id, &call.id, &call.name)?;
                    if !call.arguments.is_object() {
                        return Err("provider transcript tool arguments are incomplete".to_string());
                    }
                    transcript_blocks.push(TranscriptBlock::ToolCall {
                        response_id: block_response.clone(),
                        batch_id: block_batch.clone(),
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        audit_only: false,
                        continuation: continuation.clone(),
                    });
                    call_count = call_count.saturating_add(1);
                }
                crate::provider_runtime::NormalizedBlock::Text { .. }
                | crate::provider_runtime::NormalizedBlock::Reasoning { .. }
                | crate::provider_runtime::NormalizedBlock::ToolCall { .. } => {
                    return Err("provider transcript tool batch identity mismatch".to_string());
                }
                crate::provider_runtime::NormalizedBlock::ToolResult { .. } => {
                    return Err(
                        "provider transcript tool batch already contains results".to_string()
                    );
                }
            }
        }
        if call_count == 0 {
            return Err("provider transcript tool batch has no calls".to_string());
        }
        state.blocks = transcript_blocks;
        state.active_batch = Some(ActiveToolBatch {
            response_id: response_id.to_string(),
            batch_id: batch_id.to_string(),
        });
        Ok(())
    }

    pub fn commit_provisional_tool_result(
        &self,
        call: &crate::provider_tool_loop::DirectToolCall,
        result: &crate::provider_tool_loop::DirectToolResult,
    ) -> Result<u64, String> {
        if call.id != result.id || call.name != result.name {
            return Err("provider transcript tool completion does not match its call".to_string());
        }
        let mut state = self.state.lock();
        let active = state
            .active_batch
            .as_ref()
            .ok_or_else(|| "provider transcript tool batch is not active".to_string())?
            .clone();
        if !state.blocks.iter().any(|block| {
            matches!(block, TranscriptBlock::ToolCall {
                response_id,
                batch_id,
                id,
                name,
                arguments,
                ..
            } if response_id == &active.response_id
                && batch_id == &active.batch_id
                && id == &call.id
                && name == &call.name
                && arguments == &call.arguments)
        }) {
            return Err("provider transcript tool completion has no active call".to_string());
        }
        if state.blocks.iter().any(|block| {
            matches!(block, TranscriptBlock::ToolResult {
                response_id,
                batch_id,
                id,
                ..
            } if response_id == &active.response_id
                && batch_id == &active.batch_id
                && id == &call.id)
        }) {
            return Err("provider transcript tool completion is already committed".to_string());
        }
        let mut blocks = state.blocks.clone();
        blocks.push(TranscriptBlock::ToolResult {
            response_id: active.response_id.clone(),
            batch_id: active.batch_id.clone(),
            id: result.id.clone(),
            name: result.name.clone(),
            result: result.result.clone(),
            is_error: result.is_error,
        });
        let completed_call_ids = blocks
            .iter()
            .filter_map(|block| match block {
                TranscriptBlock::ToolResult {
                    response_id: existing_response,
                    batch_id: existing_batch,
                    id,
                    ..
                } if existing_response == &active.response_id
                    && existing_batch == &active.batch_id =>
                {
                    Some(id.clone())
                }
                TranscriptBlock::User { .. }
                | TranscriptBlock::AssistantText { .. }
                | TranscriptBlock::AssistantReasoning { .. }
                | TranscriptBlock::ToolCall { .. }
                | TranscriptBlock::ToolResult { .. }
                | TranscriptBlock::Compaction { .. } => None,
            })
            .collect();
        let checkpoint = TranscriptCheckpoint {
            branch: self.branch.clone(),
            blocks,
            boundary: CheckpointBoundary::ToolResultsCommitted {
                response_id: active.response_id,
                batch_id: active.batch_id,
                completed_call_ids,
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
        Ok(state.revision)
    }
}
