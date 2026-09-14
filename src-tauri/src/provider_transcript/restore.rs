use super::*;

impl ProviderTranscriptStore {
    pub fn restore(
        &self,
        provider: ProviderId,
        session_id: &str,
        metadata_revision: u64,
        expected_branch: &TranscriptBranch,
    ) -> Result<Option<RestoredTranscript>, String> {
        let Some(pointer) = self.read_pointer(provider, session_id)? else {
            return if metadata_revision == 0 {
                Ok(None)
            } else {
                Err("provider transcript metadata points to a missing checkpoint".to_string())
            };
        };
        let mut generation = self.load_generation_for_pointer(&pointer)?;
        if metadata_revision > generation.revision {
            return Err(
                "provider transcript metadata is ahead of the committed checkpoint".to_string(),
            );
        }
        if generation.legacy_source {
            if metadata_revision != generation.revision {
                return Err(
                    "legacy provider transcript cannot repair stale branch metadata".to_string(),
                );
            }
            generation.checkpoint.branch = expected_branch.clone();
        } else if generation.checkpoint.branch.instruction_epoch
            != expected_branch.instruction_epoch
        {
            return Err("provider transcript checkpoint branch mismatch".to_string());
        }
        if !generation.checkpoint.boundary.is_resumable() {
            return Err(
                "provider transcript stopped at a non-resumable checkpoint; explicit recovery is required"
                    .to_string(),
            );
        }
        Ok(Some(RestoredTranscript {
            metadata_was_stale: metadata_revision < generation.revision,
            generation,
        }))
    }

    pub fn rewind(
        &self,
        provider: ProviderId,
        session_id: &str,
        revision: u64,
    ) -> Result<(), String> {
        let _write_guard = self.write_lock.lock();
        if revision == 0 {
            return self.clear_current_unlocked(session_id);
        }
        let generation_path = self
            .session_dir(session_id)?
            .join("generations")
            .join(format!("{revision}.json"));
        let bytes = fs::read(&generation_path)
            .map_err(|_| "provider transcript generation is unavailable".to_string())?;
        let generation = decode_generation(&bytes)?;
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
        let _write_guard = self.write_lock.lock();
        let path = self.session_dir(session_id)?;
        match fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("provider transcript cannot be deleted".to_string()),
        }
    }

    pub fn clear_current(&self, session_id: &str) -> Result<(), String> {
        let _write_guard = self.write_lock.lock();
        self.clear_current_unlocked(session_id)
    }

    pub(super) fn clear_current_unlocked(&self, session_id: &str) -> Result<(), String> {
        let path = self.session_dir(session_id)?.join("current.json");
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("provider transcript pointer cannot be cleared".to_string()),
        }
    }
}
