//! Crash-safe normalized transcript generations for direct HTTP providers.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::DataDirs;
use crate::memory::write_atomic_bytes;
use crate::provider::ProviderId;

const TRANSCRIPT_SCHEMA_VERSION: u32 = 1;
const MAX_ENTRY_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptImage {
    pub id: String,
    pub mime_type: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TranscriptEntry {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptGeneration {
    pub schema_version: u32,
    pub provider: ProviderId,
    pub session_id: String,
    pub revision: u64,
    pub entries: Vec<TranscriptEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TranscriptPointer {
    schema_version: u32,
    provider: ProviderId,
    session_id: String,
    revision: u64,
    sha256: String,
}

#[derive(Debug, Clone)]
pub struct ProviderTranscriptStore {
    root: PathBuf,
}

impl ProviderTranscriptStore {
    pub fn new(dirs: &DataDirs) -> Self {
        Self {
            root: dirs.provider_sessions_dir(),
        }
    }

    pub fn current_revision(&self, provider: ProviderId, session_id: &str) -> Result<u64, String> {
        Ok(self
            .read_pointer(provider, session_id)?
            .map(|pointer| pointer.revision)
            .unwrap_or(0))
    }

    pub fn load_current(
        &self,
        provider: ProviderId,
        session_id: &str,
    ) -> Result<TranscriptGeneration, String> {
        let pointer = self
            .read_pointer(provider, session_id)?
            .ok_or_else(|| "provider transcript is empty".to_string())?;
        self.load_generation_for_pointer(&pointer)
    }

    pub fn publish(
        &self,
        provider: ProviderId,
        session_id: &str,
        expected_revision: u64,
        entries: Vec<TranscriptEntry>,
    ) -> Result<TranscriptGeneration, String> {
        ensure_direct_provider(provider)?;
        validate_session_id(session_id)?;
        validate_entries(&entries)?;
        let current = self.current_revision(provider, session_id)?;
        if current != expected_revision {
            return Err(format!(
                "provider transcript revision changed: expected {expected_revision}, found {current}"
            ));
        }
        let revision = current
            .checked_add(1)
            .ok_or_else(|| "provider transcript revision overflow".to_string())?;
        let generation = TranscriptGeneration {
            schema_version: TRANSCRIPT_SCHEMA_VERSION,
            provider,
            session_id: session_id.to_string(),
            revision,
            entries,
        };
        let bytes = serde_json::to_vec_pretty(&generation)
            .map_err(|_| "provider transcript cannot be serialized".to_string())?;
        let sha256 = hex_sha256(&bytes);
        let session_dir = self.session_dir(session_id)?;
        let generations = session_dir.join("generations");
        fs::create_dir_all(&generations)
            .map_err(|_| "provider transcript directory cannot be created".to_string())?;
        write_atomic_bytes(&generations.join(format!("{revision}.json")), &bytes)
            .map_err(|_| "provider transcript generation cannot be written".to_string())?;
        let pointer = TranscriptPointer {
            schema_version: TRANSCRIPT_SCHEMA_VERSION,
            provider,
            session_id: session_id.to_string(),
            revision,
            sha256,
        };
        let pointer_bytes = serde_json::to_vec_pretty(&pointer)
            .map_err(|_| "provider transcript pointer cannot be serialized".to_string())?;
        write_atomic_bytes(&session_dir.join("current.json"), &pointer_bytes)
            .map_err(|_| "provider transcript pointer cannot be written".to_string())?;
        Ok(generation)
    }

    pub fn rewind(
        &self,
        provider: ProviderId,
        session_id: &str,
        revision: u64,
    ) -> Result<(), String> {
        if revision == 0 {
            return self.delete_session(session_id);
        }
        let generation_path = self
            .session_dir(session_id)?
            .join("generations")
            .join(format!("{revision}.json"));
        let bytes = fs::read(&generation_path)
            .map_err(|_| "provider transcript generation is unavailable".to_string())?;
        let generation: TranscriptGeneration = serde_json::from_slice(&bytes)
            .map_err(|_| "provider transcript generation is corrupt".to_string())?;
        validate_generation(&generation, provider, session_id, revision)?;
        let pointer = TranscriptPointer {
            schema_version: TRANSCRIPT_SCHEMA_VERSION,
            provider,
            session_id: session_id.to_string(),
            revision,
            sha256: hex_sha256(&bytes),
        };
        let pointer_bytes = serde_json::to_vec_pretty(&pointer)
            .map_err(|_| "provider transcript pointer cannot be serialized".to_string())?;
        write_atomic_bytes(
            &self.session_dir(session_id)?.join("current.json"),
            &pointer_bytes,
        )
        .map_err(|_| "provider transcript pointer cannot be written".to_string())
    }

    pub fn delete_session(&self, session_id: &str) -> Result<(), String> {
        let path = self.session_dir(session_id)?;
        match fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("provider transcript cannot be deleted".to_string()),
        }
    }

    fn read_pointer(
        &self,
        provider: ProviderId,
        session_id: &str,
    ) -> Result<Option<TranscriptPointer>, String> {
        ensure_direct_provider(provider)?;
        let path = self.session_dir(session_id)?.join("current.json");
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("provider transcript pointer cannot be read".to_string()),
        };
        let pointer: TranscriptPointer = serde_json::from_slice(&bytes)
            .map_err(|_| "provider transcript pointer is corrupt".to_string())?;
        if pointer.schema_version != TRANSCRIPT_SCHEMA_VERSION
            || pointer.provider != provider
            || pointer.session_id != session_id
            || pointer.revision == 0
        {
            return Err("provider transcript pointer is invalid".to_string());
        }
        Ok(Some(pointer))
    }

    fn load_generation_for_pointer(
        &self,
        pointer: &TranscriptPointer,
    ) -> Result<TranscriptGeneration, String> {
        let path = self
            .session_dir(&pointer.session_id)?
            .join("generations")
            .join(format!("{}.json", pointer.revision));
        let bytes = fs::read(path)
            .map_err(|_| "provider transcript generation cannot be read".to_string())?;
        if hex_sha256(&bytes) != pointer.sha256 {
            return Err("provider transcript generation hash mismatch".to_string());
        }
        let generation: TranscriptGeneration = serde_json::from_slice(&bytes)
            .map_err(|_| "provider transcript generation is corrupt".to_string())?;
        validate_generation(
            &generation,
            pointer.provider,
            &pointer.session_id,
            pointer.revision,
        )?;
        Ok(generation)
    }

    fn session_dir(&self, session_id: &str) -> Result<PathBuf, String> {
        validate_session_id(session_id)?;
        Ok(self.root.join(session_id))
    }
}

fn ensure_direct_provider(provider: ProviderId) -> Result<(), String> {
    if matches!(
        provider,
        ProviderId::Antigravity | ProviderId::OpencodeGo | ProviderId::Ollama
    ) {
        Ok(())
    } else {
        Err("provider transcript is available only to direct providers".to_string())
    }
}

fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || session_id.len() > 128
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("provider transcript session id is invalid".to_string());
    }
    Ok(())
}

fn validate_generation(
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
    validate_entries(&generation.entries)
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

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> (PathBuf, ProviderTranscriptStore) {
        let base =
            std::env::temp_dir().join(format!("eud-transcript-{tag}-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        (base, ProviderTranscriptStore::new(&dirs))
    }

    #[test]
    fn generation_publish_load_rewind_and_delete_are_addressed() {
        let (base, store) = store("roundtrip");
        let one = store
            .publish(
                ProviderId::OpencodeGo,
                "session-a",
                0,
                vec![TranscriptEntry::User {
                    text: "first".to_string(),
                    images: Vec::new(),
                }],
            )
            .unwrap();
        let two = store
            .publish(
                ProviderId::OpencodeGo,
                "session-a",
                one.revision,
                vec![
                    one.entries[0].clone(),
                    TranscriptEntry::AssistantText {
                        text: "answer".to_string(),
                    },
                ],
            )
            .unwrap();
        store
            .publish(
                ProviderId::Antigravity,
                "session-b",
                0,
                vec![TranscriptEntry::User {
                    text: "other".to_string(),
                    images: Vec::new(),
                }],
            )
            .unwrap();
        assert_eq!(
            store
                .load_current(ProviderId::OpencodeGo, "session-a")
                .unwrap(),
            two
        );
        store
            .rewind(ProviderId::OpencodeGo, "session-a", 1)
            .unwrap();
        assert_eq!(
            store
                .load_current(ProviderId::OpencodeGo, "session-a")
                .unwrap()
                .entries,
            one.entries
        );
        store.delete_session("session-a").unwrap();
        assert_eq!(
            store
                .current_revision(ProviderId::OpencodeGo, "session-a")
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .current_revision(ProviderId::Antigravity, "session-b")
                .unwrap(),
            1
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn mismatched_provider_and_revision_fail_closed() {
        let (base, store) = store("authority");
        store
            .publish(
                ProviderId::OpencodeGo,
                "session-a",
                0,
                vec![TranscriptEntry::AssistantText {
                    text: "answer".to_string(),
                }],
            )
            .unwrap();
        assert!(store
            .load_current(ProviderId::Antigravity, "session-a")
            .is_err());
        assert!(store
            .publish(ProviderId::OpencodeGo, "session-a", 0, Vec::new())
            .is_err());
        fs::remove_dir_all(base).ok();
    }
}
