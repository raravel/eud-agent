use std::{fs, io, path::PathBuf, sync::OnceLock};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{provider::ProviderId, provider_runtime::RunIdentity};

use super::DurableToolCompletion;

#[path = "receipt_recovery.rs"]
mod recovery;
pub use recovery::{acknowledge_native_run, clear_native_run_recovery, unresolved_native_runs};

const RECEIPT_SCHEMA_VERSION: u32 = 2;
const MAX_RECEIPT_BYTES: usize = 64 * 1024 * 1024;
const MAX_NATIVE_ID_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeRunReceiptState {
    Pending,
    Unknown,
    /// The run was cancelled or failed after the native session identity was
    /// observed. The turn itself did not settle, but that session stays
    /// resumable, so the next run continues from its candidate id instead of
    /// refusing the conversation.
    Interrupted,
    Completed,
    Cleared,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnresolvedNativeRun {
    pub provider: ProviderId,
    pub prior_native_id: Option<String>,
    pub candidate_native_id: Option<String>,
    pub state: NativeRunReceiptState,
    pub identity: RunIdentity,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunReceipt {
    schema_version: u32,
    session_id: String,
    run_id: u64,
    request_id: String,
    cancellation_generation: u64,
    session_kind: crate::session::SessionKind,
    provider: Option<ProviderId>,
    prior_native_id: Option<String>,
    candidate_native_id: Option<String>,
    lifecycle: Option<NativeRunReceiptState>,
    completions: Vec<DurableToolCompletion>,
}

pub(super) struct RunReceiptStore {
    path: PathBuf,
    receipt: Mutex<RunReceipt>,
    claim: OnceLock<Result<(), String>>,
}

impl RunReceiptStore {
    pub(super) fn new(journal_dir: PathBuf, identity: &RunIdentity) -> Self {
        Self {
            path: receipt_path(&journal_dir, identity),
            claim: OnceLock::new(),
            receipt: Mutex::new(RunReceipt {
                schema_version: RECEIPT_SCHEMA_VERSION,
                session_id: identity.session_id.clone(),
                run_id: identity.run_id.get(),
                request_id: identity.request_id.clone(),
                cancellation_generation: identity.cancellation_generation,
                session_kind: identity.session_kind,
                provider: None,
                prior_native_id: None,
                candidate_native_id: None,
                lifecycle: None,
                completions: Vec::new(),
            }),
        }
    }

    pub(super) fn path(&self) -> PathBuf {
        self.path.clone()
    }

    pub(super) fn begin_native(
        &self,
        provider: ProviderId,
        prior_native_id: Option<&str>,
    ) -> Result<(), String> {
        validate_native_id(prior_native_id)?;
        let mut receipt = self.receipt.lock();
        receipt.provider = Some(provider);
        receipt.prior_native_id = prior_native_id.map(str::to_string);
        receipt.candidate_native_id = None;
        receipt.lifecycle = Some(NativeRunReceiptState::Pending);
        self.persist_owned(&receipt)
    }

    pub(super) fn mark_unknown(&self) -> Result<(), String> {
        self.update_lifecycle(NativeRunReceiptState::Unknown, None)
    }

    pub(super) fn mark_interrupted(&self, candidate_native_id: &str) -> Result<(), String> {
        validate_native_id(Some(candidate_native_id))?;
        self.update_lifecycle(
            NativeRunReceiptState::Interrupted,
            Some(candidate_native_id),
        )
    }

    pub(super) fn mark_completed(&self, candidate_native_id: Option<&str>) -> Result<(), String> {
        validate_native_id(candidate_native_id)?;
        self.update_lifecycle(NativeRunReceiptState::Completed, candidate_native_id)
    }

    fn update_lifecycle(
        &self,
        state: NativeRunReceiptState,
        candidate_native_id: Option<&str>,
    ) -> Result<(), String> {
        let mut receipt = self.receipt.lock();
        if receipt.lifecycle.is_none() {
            return Err("native run receipt was not started".to_string());
        }
        receipt.lifecycle = Some(state);
        receipt.candidate_native_id = candidate_native_id.map(str::to_string);
        self.persist_owned(&receipt)
    }

    pub(super) fn persist(&self, completions: &[DurableToolCompletion]) -> Result<(), String> {
        let mut receipt = self.receipt.lock();
        receipt.completions = completions.to_vec();
        self.persist_owned(&receipt)
    }

    pub(super) fn acknowledge(&self) -> Result<(), String> {
        match self.claim.get() {
            Some(Ok(())) => remove_receipt(&self.path),
            Some(Err(error)) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn persist_owned(&self, receipt: &RunReceipt) -> Result<(), String> {
        self.claim
            .get_or_init(|| {
                let parent = self
                    .path
                    .parent()
                    .ok_or_else(|| "native run receipt has no directory".to_string())?;
                fs::create_dir_all(parent).map_err(|error| {
                    format!("native run receipt directory cannot be created: {error}")
                })?;
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&self.path)
                    .map_err(|error| {
                        format!("native run receipt already exists or cannot be claimed: {error}")
                    })?;
                Ok(())
            })
            .as_ref()
            .map_err(Clone::clone)?;
        persist_receipt(&self.path, receipt)
    }
}

fn persist_receipt(path: &std::path::Path, receipt: &RunReceipt) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|error| format!("provider tool receipt cannot be serialized: {error}"))?;
    if bytes.len() > MAX_RECEIPT_BYTES {
        return Err("provider tool receipt exceeds its bounded size".to_string());
    }
    crate::memory::write_atomic_bytes(path, &bytes)
        .map_err(|error| format!("provider tool receipt cannot be persisted: {error}"))
}

fn read_receipt(path: &std::path::Path) -> Result<RunReceipt, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("native run receipt metadata cannot be read: {error}"))?;
    if metadata.len() > MAX_RECEIPT_BYTES as u64 {
        return Err("native run receipt exceeds its bounded size".to_string());
    }
    let bytes =
        fs::read(path).map_err(|error| format!("native run receipt cannot be read: {error}"))?;
    let receipt: RunReceipt = serde_json::from_slice(&bytes)
        .map_err(|error| format!("native run receipt is invalid: {error}"))?;
    if receipt.schema_version != RECEIPT_SCHEMA_VERSION {
        return Err("native run receipt schema version is unsupported".to_string());
    }
    Ok(receipt)
}

fn remove_receipt(path: &std::path::Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("provider tool receipt cannot be removed: {error}")),
    }
}

fn validate_native_id(value: Option<&str>) -> Result<(), String> {
    if value.is_some_and(|value| value.len() > MAX_NATIVE_ID_BYTES) {
        Err("native conversation id exceeds its bounded size".to_string())
    } else {
        Ok(())
    }
}

fn receipt_directory(journal_dir: &std::path::Path, session_id: &str) -> PathBuf {
    journal_dir
        .join("provider-run-receipts")
        .join(digest_key(session_id.as_bytes()))
}

fn corrupt_receipt_audit_directory(journal_dir: &std::path::Path, session_id: &str) -> PathBuf {
    journal_dir
        .join("provider-run-receipt-audit")
        .join(digest_key(session_id.as_bytes()))
}

fn receipt_path(journal_dir: &std::path::Path, identity: &RunIdentity) -> PathBuf {
    let request_key = digest_key(identity.request_id.as_bytes());
    receipt_directory(journal_dir, &identity.session_id).join(format!(
        "{}-{}-{}.json",
        identity.run_id.get(),
        identity.cancellation_generation,
        &request_key[..16]
    ))
}

fn digest_key(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut key = String::with_capacity(digest.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        key.push(char::from(HEX[usize::from(byte >> 4)]));
        key.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    key
}

#[cfg(test)]
#[path = "receipt_tests.rs"]
mod tests;
