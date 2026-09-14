use super::*;

impl ProviderTranscriptStore {
    pub fn new(dirs: &DataDirs) -> Self {
        Self {
            root: dirs.provider_sessions_dir(),
            write_lock: Arc::new(parking_lot::Mutex::new(())),
        }
    }

    pub fn checkpoint_writer(
        &self,
        provider: ProviderId,
        session_id: &str,
        expected_revision: u64,
        branch: TranscriptBranch,
        blocks: Vec<TranscriptBlock>,
    ) -> Result<RunCheckpointWriter, String> {
        ensure_direct_provider(provider)?;
        validate_session_id(session_id)?;
        let current = self.current_revision(provider, session_id)?;
        if current != expected_revision {
            return Err(format!(
                "provider transcript revision changed: expected {expected_revision}, found {current}"
            ));
        }
        Ok(RunCheckpointWriter {
            store: self.clone(),
            provider,
            session_id: session_id.to_string(),
            branch,
            state: Arc::new(parking_lot::Mutex::new(CheckpointWriterState {
                revision: current,
                blocks,
                active_batch: None,
            })),
        })
    }

    pub fn current_revision(&self, provider: ProviderId, session_id: &str) -> Result<u64, String> {
        let Some(pointer) = self.read_pointer(provider, session_id)? else {
            return Ok(0);
        };
        self.load_generation_for_pointer(&pointer)?;
        Ok(pointer.revision)
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

    pub fn publish_checkpoint(
        &self,
        provider: ProviderId,
        session_id: &str,
        expected_revision: u64,
        checkpoint: TranscriptCheckpoint,
    ) -> Result<TranscriptGeneration, String> {
        let _write_guard = self.write_lock.lock();
        ensure_direct_provider(provider)?;
        validate_session_id(session_id)?;
        validate_checkpoint(&checkpoint, provider)?;
        let current = self.current_revision(provider, session_id)?;
        if current != expected_revision {
            return Err(format!(
                "provider transcript revision changed: expected {expected_revision}, found {current}"
            ));
        }
        let revision = self.next_generation_revision(session_id, current)?;
        let generation = TranscriptGeneration {
            schema_version: TRANSCRIPT_SCHEMA_VERSION,
            provider,
            session_id: session_id.to_string(),
            revision,
            legacy_source: false,
            checkpoint,
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
}

impl ProviderTranscriptStore {
    pub(super) fn next_generation_revision(
        &self,
        session_id: &str,
        current: u64,
    ) -> Result<u64, String> {
        let path = self.session_dir(session_id)?.join("generations");
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(1),
            Err(_) => return Err("provider transcript generations cannot be read".to_string()),
        };
        let mut maximum = current;
        for entry in entries {
            let entry =
                entry.map_err(|_| "provider transcript generations cannot be read".to_string())?;
            if let Some(revision) = entry
                .path()
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.parse::<u64>().ok())
            {
                maximum = maximum.max(revision);
            }
        }
        maximum
            .checked_add(1)
            .ok_or_else(|| "provider transcript revision overflow".to_string())
    }

    pub(super) fn read_pointer(
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
        if !matches!(
            pointer.schema_version,
            LEGACY_TRANSCRIPT_SCHEMA_VERSION | TRANSCRIPT_SCHEMA_VERSION
        ) || pointer.provider != provider
            || pointer.session_id != session_id
            || pointer.revision == 0
        {
            return Err("provider transcript pointer is invalid".to_string());
        }
        Ok(Some(pointer))
    }

    pub(super) fn load_generation_for_pointer(
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
        let generation = decode_generation(&bytes)?;
        validate_generation(
            &generation,
            pointer.provider,
            &pointer.session_id,
            pointer.revision,
        )?;
        Ok(generation)
    }

    pub(super) fn session_dir(&self, session_id: &str) -> Result<PathBuf, String> {
        validate_session_id(session_id)?;
        Ok(self.root.join(session_id))
    }
}

pub(super) fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
