use super::*;

fn store(tag: &str) -> (PathBuf, ProviderTranscriptStore) {
    let base = std::env::temp_dir().join(format!("eud-transcript-{tag}-{}", uuid::Uuid::new_v4()));
    let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
    (base, ProviderTranscriptStore::new(&dirs))
}

impl ProviderTranscriptStore {
    #[cfg(test)]
    fn publish(
        &self,
        provider: ProviderId,
        session_id: &str,
        expected_revision: u64,
        entries: Vec<TranscriptEntry>,
    ) -> Result<TranscriptGeneration, String> {
        validate_entries(&entries)?;
        let next_revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| "provider transcript revision overflow".to_string())?;
        let checkpoint = legacy_entries_to_checkpoint(provider, next_revision, &entries)?;
        self.publish_checkpoint(provider, session_id, expected_revision, checkpoint)
    }
}

fn validate_entries(entries: &[TranscriptEntry]) -> Result<(), String> {
    if entries.len() > MAX_ENTRIES {
        return Err("provider transcript has too many entries".to_string());
    }
    for entry in entries {
        let bytes = serde_json::to_vec(entry)
            .map_err(|_| "provider transcript entry cannot be serialized".to_string())?;
        if bytes.len() > MAX_ENTRY_BYTES {
            return Err("provider transcript entry is too large".to_string());
        }
        match entry {
            TranscriptEntry::ToolCall { id, name, .. }
            | TranscriptEntry::ToolResult { id, name, .. }
                if id.is_empty() || name.is_empty() =>
            {
                return Err("provider transcript tool entry is incomplete".to_string())
            }
            TranscriptEntry::User { images, .. }
                if images.iter().any(|image| {
                    image.id.is_empty()
                        || image.id.len() > 128
                        || !image
                            .id
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                        || !matches!(
                            image.mime_type.as_str(),
                            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
                        )
                        || image.data_base64.is_empty()
                }) =>
            {
                return Err("provider transcript image is invalid".to_string())
            }
            _ => {}
        }
    }
    Ok(())
}

#[path = "checkpoint_tests.rs"]
mod checkpoints;
#[path = "compatibility_tests.rs"]
mod compatibility;
#[path = "storage_tests.rs"]
mod storage;
#[path = "transition_tests.rs"]
mod transition;
