use super::*;

pub fn conversation_images(
    paths: &[PathBuf],
) -> Result<Vec<crate::provider_runtime::ConversationImage>, String> {
    paths
        .iter()
        .map(|path| {
            let bytes = fs::read(path).map_err(|_| "provider image cannot be read".to_string())?;
            if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
                return Err("provider image size is unsupported".to_string());
            }
            let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                "image/png"
            } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
                "image/jpeg"
            } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
                "image/webp"
            } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
                "image/gif"
            } else {
                return Err("provider image format is unsupported".to_string());
            };
            let digest = Sha256::digest(&bytes);
            let id = digest[..16]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            Ok(crate::provider_runtime::ConversationImage {
                id,
                mime: mime.to_string(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            })
        })
        .collect()
}

pub(super) fn transcript_history(
    blocks: &[TranscriptBlock],
) -> Vec<crate::provider_runtime::ConversationItem> {
    blocks
        .iter()
        .filter(|block| {
            !matches!(
                block,
                TranscriptBlock::ToolCall {
                    audit_only: true,
                    ..
                }
            )
        })
        .map(|block| match block {
            TranscriptBlock::User {
                request_id,
                text,
                images,
            } => crate::provider_runtime::ConversationItem::User {
                request_id: request_id.clone(),
                text: text.clone(),
                images: images
                    .iter()
                    .map(|image| crate::provider_runtime::ConversationImage {
                        id: image.id.clone(),
                        mime: image.mime_type.clone(),
                        data_base64: image.data_base64.clone(),
                    })
                    .collect(),
            },
            TranscriptBlock::AssistantText { response_id, text } => {
                crate::provider_runtime::ConversationItem::Assistant(
                    crate::provider_runtime::NormalizedBlock::Text {
                        response_id: response_id.clone(),
                        text: text.clone(),
                    },
                )
            }
            TranscriptBlock::AssistantReasoning {
                response_id,
                text,
                continuation,
            } => crate::provider_runtime::ConversationItem::Assistant(
                crate::provider_runtime::NormalizedBlock::Reasoning {
                    response_id: response_id.clone(),
                    text: text.clone(),
                    continuation: continuation.clone(),
                },
            ),
            TranscriptBlock::ToolCall {
                response_id,
                batch_id,
                id,
                name,
                arguments,
                audit_only: _,
                continuation,
            } => crate::provider_runtime::ConversationItem::Assistant(
                crate::provider_runtime::NormalizedBlock::ToolCall {
                    response_id: response_id.clone(),
                    batch_id: batch_id.clone(),
                    call: crate::provider_tool_loop::DirectToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    },
                    continuation: continuation.clone(),
                },
            ),
            TranscriptBlock::ToolResult {
                response_id,
                batch_id,
                id,
                name,
                result,
                is_error,
            } => crate::provider_runtime::ConversationItem::Assistant(
                crate::provider_runtime::NormalizedBlock::ToolResult {
                    response_id: response_id.clone(),
                    batch_id: batch_id.clone(),
                    result: crate::provider_tool_loop::DirectToolResult {
                        id: id.clone(),
                        name: name.clone(),
                        result: result.clone(),
                        is_error: *is_error,
                    },
                },
            ),
            TranscriptBlock::Compaction {
                summary,
                previous_revision,
            } => crate::provider_runtime::ConversationItem::Compaction {
                summary: summary.clone(),
                previous_revision: *previous_revision,
            },
        })
        .collect()
}
