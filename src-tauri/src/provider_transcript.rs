//! Crash-safe normalized transcript generations for direct HTTP providers.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::DataDirs;
use crate::memory::write_atomic_bytes;
use crate::provider::ProviderId;

const TRANSCRIPT_SCHEMA_VERSION: u32 = 2;
const LEGACY_TRANSCRIPT_SCHEMA_VERSION: u32 = 1;
const MAX_ENTRY_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptImage {
    pub id: String,
    pub mime_type: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptBranch {
    pub instruction_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_leaf_id: Option<String>,
}

impl TranscriptBranch {
    pub const fn legacy() -> Self {
        Self {
            instruction_epoch: 0,
            task_leaf_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TranscriptBlock {
    User {
        request_id: String,
        text: String,
        #[serde(default)]
        images: Vec<TranscriptImage>,
    },
    AssistantText {
        response_id: String,
        text: String,
    },
    AssistantReasoning {
        response_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        continuation: Option<crate::provider_runtime::ProviderContinuation>,
    },
    ToolCall {
        response_id: String,
        batch_id: String,
        id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(default)]
        audit_only: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        continuation: Option<crate::provider_runtime::ProviderContinuation>,
    },
    ToolResult {
        response_id: String,
        batch_id: String,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CheckpointBoundary {
    ContextCommitted {
        request_id: String,
    },
    ToolResultsCommitted {
        response_id: String,
        batch_id: String,
        completed_call_ids: Vec<String>,
    },
    CompletedToolBatch {
        response_id: String,
        batch_id: String,
    },
    WriteTransition {
        response_id: String,
        batch_id: String,
        executed_call_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observed_continuation: Option<crate::provider_runtime::ProviderContinuation>,
    },
    ResponseCompleted {
        response_id: String,
    },
    Compaction {
        previous_revision: u64,
    },
}

impl CheckpointBoundary {
    pub const fn is_resumable(&self) -> bool {
        matches!(
            self,
            Self::CompletedToolBatch { .. }
                | Self::ResponseCompleted { .. }
                | Self::Compaction { .. }
                | Self::WriteTransition { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptCheckpoint {
    pub branch: TranscriptBranch,
    pub blocks: Vec<TranscriptBlock>,
    pub boundary: CheckpointBoundary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<crate::provider_runtime::ProviderContinuation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptGeneration {
    pub schema_version: u32,
    pub provider: ProviderId,
    pub session_id: String,
    pub revision: u64,
    #[serde(skip)]
    legacy_source: bool,
    pub checkpoint: TranscriptCheckpoint,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RestoredTranscript {
    pub generation: TranscriptGeneration,
    pub metadata_was_stale: bool,
}

#[derive(Debug)]
struct CheckpointWriterState {
    revision: u64,
    blocks: Vec<TranscriptBlock>,
    active_batch: Option<ActiveToolBatch>,
}

#[derive(Debug, Clone)]
struct ActiveToolBatch {
    response_id: String,
    batch_id: String,
}

#[derive(Debug, Clone)]
pub struct RunCheckpointWriter {
    store: ProviderTranscriptStore,
    provider: ProviderId,
    session_id: String,
    branch: TranscriptBranch,
    state: Arc<parking_lot::Mutex<CheckpointWriterState>>,
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
    write_lock: Arc<parking_lot::Mutex<()>>,
}

mod boundary_validation;
mod history;
mod legacy;
mod restore;
mod store;
mod validation;
mod writer;
mod writer_batch;

use boundary_validation::validate_boundary;
pub use history::conversation_images;
use history::transcript_history;
use legacy::{decode_generation, validate_generation};
#[cfg(test)]
use legacy::{legacy_entries_to_checkpoint, TranscriptEntry};
use store::hex_sha256;
use validation::validate_checkpoint;
use validation::{
    ensure_direct_provider, validate_continuation, validate_identifier, validate_session_id,
    validate_tool_coordinates,
};

#[cfg(test)]
mod tests;
