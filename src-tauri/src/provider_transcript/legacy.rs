use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum TranscriptEntry {
    User {
        text: String,
        #[serde(default)]
        images: Vec<TranscriptImage>,
    },
    AssistantText {
        text: String,
    },
    AssistantReasoning {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    ToolResult {
        id: String,
        name: String,
        result: serde_json::Value,
        is_error: bool,
    },
    Compaction {
        summary: String,
        previous_revision: u64,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyTranscriptGeneration {
    schema_version: u32,
    provider: ProviderId,
    session_id: String,
    revision: u64,
    entries: Vec<TranscriptEntry>,
}

pub(super) fn legacy_entries_to_checkpoint(
    provider: ProviderId,
    revision: u64,
    entries: &[TranscriptEntry],
) -> Result<TranscriptCheckpoint, String> {
    let mut blocks = Vec::with_capacity(entries.len());
    let mut response_number = 0_u64;
    let mut request_number = 0_u64;
    let mut batch_number = 0_u64;
    let mut current_response = String::new();
    let mut current_batch = String::new();
    let mut call_coordinates: HashMap<&str, (String, String)> = HashMap::new();
    let mut previous_was_call = false;

    for entry in entries {
        match entry {
            TranscriptEntry::User { text, images } => {
                request_number = request_number.saturating_add(1);
                blocks.push(TranscriptBlock::User {
                    request_id: format!("legacy-r{revision}-request-{request_number}"),
                    text: text.clone(),
                    images: images.clone(),
                });
                current_response.clear();
                current_batch.clear();
                previous_was_call = false;
            }
            TranscriptEntry::AssistantText { text } => {
                if current_response.is_empty() {
                    response_number = response_number.saturating_add(1);
                    current_response = format!("legacy-r{revision}-response-{response_number}");
                }
                blocks.push(TranscriptBlock::AssistantText {
                    response_id: current_response.clone(),
                    text: text.clone(),
                });
                previous_was_call = false;
            }
            TranscriptEntry::AssistantReasoning { text } => {
                if current_response.is_empty() {
                    response_number = response_number.saturating_add(1);
                    current_response = format!("legacy-r{revision}-response-{response_number}");
                }
                blocks.push(TranscriptBlock::AssistantReasoning {
                    response_id: current_response.clone(),
                    text: text.clone(),
                    continuation: None,
                });
                previous_was_call = false;
            }
            TranscriptEntry::ToolCall {
                id,
                name,
                arguments,
                thought_signature,
            } => {
                if current_response.is_empty() {
                    response_number = response_number.saturating_add(1);
                    current_response = format!("legacy-r{revision}-response-{response_number}");
                }
                if !previous_was_call {
                    batch_number = batch_number.saturating_add(1);
                    current_batch = format!("legacy-r{revision}-batch-{batch_number}");
                }
                let continuation = thought_signature.as_ref().map(|signature| {
                    crate::provider_runtime::ProviderContinuation {
                        provider,
                        data: serde_json::json!({"thoughtSignature": signature}),
                    }
                });
                blocks.push(TranscriptBlock::ToolCall {
                    response_id: current_response.clone(),
                    batch_id: current_batch.clone(),
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    audit_only: false,
                    continuation,
                });
                call_coordinates.insert(id, (current_response.clone(), current_batch.clone()));
                previous_was_call = true;
            }
            TranscriptEntry::ToolResult {
                id,
                name,
                result,
                is_error,
            } => {
                let (response_id, batch_id) =
                    call_coordinates.get(id.as_str()).ok_or_else(|| {
                        "legacy provider transcript tool result has no matching call".to_string()
                    })?;
                blocks.push(TranscriptBlock::ToolResult {
                    response_id: response_id.clone(),
                    batch_id: batch_id.clone(),
                    id: id.clone(),
                    name: name.clone(),
                    result: result.clone(),
                    is_error: *is_error,
                });
                previous_was_call = false;
            }
            TranscriptEntry::Compaction {
                summary,
                previous_revision,
            } => {
                blocks.push(TranscriptBlock::Compaction {
                    summary: summary.clone(),
                    previous_revision: *previous_revision,
                });
                current_response.clear();
                current_batch.clear();
                previous_was_call = false;
            }
        }
    }

    let boundary = match blocks.last() {
        Some(TranscriptBlock::Compaction {
            previous_revision, ..
        }) => CheckpointBoundary::Compaction {
            previous_revision: *previous_revision,
        },
        Some(TranscriptBlock::User { request_id, .. }) if current_response.is_empty() => {
            CheckpointBoundary::ContextCommitted {
                request_id: request_id.clone(),
            }
        }
        Some(_) if !current_response.is_empty() => CheckpointBoundary::ResponseCompleted {
            response_id: current_response,
        },
        _ => return Err("provider transcript checkpoint is empty".to_string()),
    };
    let checkpoint = TranscriptCheckpoint {
        branch: TranscriptBranch::legacy(),
        blocks,
        boundary,
        continuation: None,
    };
    validate_checkpoint(&checkpoint, provider)?;
    Ok(checkpoint)
}

pub(super) fn validate_generation(
    generation: &TranscriptGeneration,
    provider: ProviderId,
    session_id: &str,
    revision: u64,
) -> Result<(), String> {
    if generation.schema_version != TRANSCRIPT_SCHEMA_VERSION
        || generation.provider != provider
        || generation.session_id != session_id
        || generation.revision != revision
    {
        return Err("provider transcript generation authority mismatch".to_string());
    }
    validate_checkpoint(&generation.checkpoint, provider)
}

pub(super) fn decode_generation(bytes: &[u8]) -> Result<TranscriptGeneration, String> {
    let header: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| "provider transcript generation is corrupt".to_string())?;
    let schema_version = header
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "provider transcript generation is corrupt".to_string())?;
    match schema_version {
        value if value == u64::from(TRANSCRIPT_SCHEMA_VERSION) => {
            let generation: TranscriptGeneration = serde_json::from_slice(bytes)
                .map_err(|_| "provider transcript generation is corrupt".to_string())?;
            validate_checkpoint(&generation.checkpoint, generation.provider)?;
            Ok(generation)
        }
        value if value == u64::from(LEGACY_TRANSCRIPT_SCHEMA_VERSION) => {
            let legacy: LegacyTranscriptGeneration = serde_json::from_slice(bytes)
                .map_err(|_| "provider transcript generation is corrupt".to_string())?;
            if legacy.schema_version != LEGACY_TRANSCRIPT_SCHEMA_VERSION {
                return Err("provider transcript generation schema is unsupported".to_string());
            }
            let checkpoint =
                legacy_entries_to_checkpoint(legacy.provider, legacy.revision, &legacy.entries)?;
            Ok(TranscriptGeneration {
                schema_version: TRANSCRIPT_SCHEMA_VERSION,
                provider: legacy.provider,
                session_id: legacy.session_id,
                revision: legacy.revision,
                legacy_source: true,
                checkpoint,
            })
        }
        _ => Err("provider transcript generation schema is unsupported".to_string()),
    }
}
