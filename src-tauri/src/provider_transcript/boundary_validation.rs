use super::validation::ToolCallsByBatch;
use super::*;

pub(super) fn validate_boundary(
    boundary: &CheckpointBoundary,
    blocks: &[TranscriptBlock],
    calls: &ToolCallsByBatch<'_>,
    results: &HashMap<(&str, &str), Vec<(&str, &str)>>,
    provider: ProviderId,
) -> Result<(), String> {
    match boundary {
        CheckpointBoundary::ToolResultsCommitted {
            response_id,
            batch_id,
            completed_call_ids,
        } => {
            validate_identifier(response_id, "response id")?;
            validate_identifier(batch_id, "batch id")?;
            if completed_call_ids.is_empty() {
                return Err("provider transcript committed tool boundary is empty".to_string());
            }
            let actual = results
                .get(&(response_id.as_str(), batch_id.as_str()))
                .ok_or_else(|| {
                    "provider transcript committed tool results are missing".to_string()
                })?;
            if completed_call_ids.len() != actual.len()
                || completed_call_ids
                    .iter()
                    .zip(actual.iter())
                    .any(|(expected, (actual, _))| expected != actual)
            {
                return Err(
                    "provider transcript committed tool result boundary mismatch".to_string(),
                );
            }
            if !calls.contains_key(&(response_id.as_str(), batch_id.as_str())) {
                return Err("provider transcript committed tool calls are missing".to_string());
            }
        }
        CheckpointBoundary::ContextCommitted { request_id } => {
            validate_identifier(request_id, "request id")?;
            if !matches!(
                blocks.last(),
                Some(TranscriptBlock::User {
                    request_id: block_id,
                    ..
                }) if block_id == request_id
            ) {
                return Err("provider transcript context boundary mismatch".to_string());
            }
        }
        CheckpointBoundary::CompletedToolBatch {
            response_id,
            batch_id,
        } => {
            validate_identifier(response_id, "response id")?;
            validate_identifier(batch_id, "batch id")?;
            let key = (response_id.as_str(), batch_id.as_str());
            let batch_calls = calls
                .get(&key)
                .ok_or_else(|| "provider transcript completed batch has no calls".to_string())?;
            let batch_results = results
                .get(&key)
                .ok_or_else(|| "provider transcript completed batch has no results".to_string())?;
            if batch_calls.len() != batch_results.len()
                || batch_calls.iter().zip(batch_results.iter()).any(
                    |((call_id, call_name, audit_only), (result_id, result_name))| {
                        *audit_only || call_id != result_id || call_name != result_name
                    },
                )
            {
                return Err("provider transcript completed batch is incomplete".to_string());
            }
            reject_incomplete_resumable_batches(calls, results)?;
        }
        CheckpointBoundary::WriteTransition {
            response_id,
            batch_id,
            executed_call_ids,
            observed_continuation,
        } => {
            validate_identifier(response_id, "response id")?;
            validate_identifier(batch_id, "batch id")?;
            if let Some(continuation) = observed_continuation {
                validate_continuation(continuation, provider)?;
            }
            let key = (response_id.as_str(), batch_id.as_str());
            let batch_calls = calls
                .get(&key)
                .ok_or_else(|| "provider transcript write transition has no calls".to_string())?;
            let batch_results = results
                .get(&key)
                .ok_or_else(|| "provider transcript write transition has no results".to_string())?;
            if batch_results.len() > batch_calls.len()
                || executed_call_ids.len() != batch_results.len()
                || executed_call_ids
                    .iter()
                    .zip(batch_results.iter())
                    .any(|(expected, (actual, _))| expected != actual)
            {
                return Err("provider transcript write transition boundary mismatch".to_string());
            }
            if batch_calls.iter().any(|(id, _, audit_only)| {
                *audit_only == executed_call_ids.iter().any(|executed| executed == id)
            }) {
                return Err(
                    "provider transcript write transition replay visibility mismatch".to_string(),
                );
            }
        }
        CheckpointBoundary::ResponseCompleted { response_id } => {
            validate_identifier(response_id, "response id")?;
            reject_incomplete_resumable_batches(calls, results)?;
            let has_response = blocks.iter().any(|block| match block {
                TranscriptBlock::AssistantText {
                    response_id: block_id,
                    ..
                }
                | TranscriptBlock::AssistantReasoning {
                    response_id: block_id,
                    ..
                }
                | TranscriptBlock::ToolCall {
                    response_id: block_id,
                    ..
                }
                | TranscriptBlock::ToolResult {
                    response_id: block_id,
                    ..
                } => block_id == response_id,
                TranscriptBlock::User { .. } | TranscriptBlock::Compaction { .. } => false,
            });
            if !has_response {
                return Err("provider transcript completed response is missing".to_string());
            }
        }
        CheckpointBoundary::Compaction { previous_revision } => {
            if !matches!(
                blocks.last(),
                Some(TranscriptBlock::Compaction {
                    previous_revision: block_revision,
                    ..
                }) if block_revision == previous_revision
            ) {
                return Err("provider transcript compaction boundary mismatch".to_string());
            }
        }
    }
    Ok(())
}

pub(super) fn reject_incomplete_resumable_batches(
    calls: &ToolCallsByBatch<'_>,
    results: &HashMap<(&str, &str), Vec<(&str, &str)>>,
) -> Result<(), String> {
    let has_visible_incomplete_batch = calls.iter().any(|(key, batch_calls)| {
        let result_ids = results
            .get(key)
            .into_iter()
            .flatten()
            .map(|(id, _)| *id)
            .collect::<HashSet<_>>();
        batch_calls
            .iter()
            .any(|(id, _, audit_only)| !result_ids.contains(id) && !audit_only)
    });
    if has_visible_incomplete_batch {
        return Err("provider transcript contains an incomplete resumable tool batch".to_string());
    }
    Ok(())
}
