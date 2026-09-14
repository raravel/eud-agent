use super::{ProviderRuntimeError, MAX_PROVIDER_CONTINUATION_BYTES};
use crate::{
    provider::ProviderId,
    provider_tool_loop::{DirectToolCall, DirectToolResult},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderContinuation {
    pub provider: ProviderId,
    pub data: Value,
}

impl ProviderContinuation {
    pub fn validate(&self, provider: ProviderId) -> Result<(), ProviderRuntimeError> {
        if self.provider != provider {
            return Err(ProviderRuntimeError::ContinuationProviderMismatch);
        }
        let size = serde_json::to_vec(&self.data)
            .map_err(|_| ProviderRuntimeError::ContinuationInvalid)?
            .len();
        if size > MAX_PROVIDER_CONTINUATION_BYTES {
            return Err(ProviderRuntimeError::ContinuationTooLarge);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NormalizedBlock {
    Text {
        response_id: String,
        text: String,
    },
    Reasoning {
        response_id: String,
        text: String,
        continuation: Option<ProviderContinuation>,
    },
    ToolCall {
        response_id: String,
        batch_id: String,
        call: DirectToolCall,
        continuation: Option<ProviderContinuation>,
    },
    ToolResult {
        response_id: String,
        batch_id: String,
        result: DirectToolResult,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationImage {
    pub id: String,
    pub mime: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConversationItem {
    User {
        request_id: String,
        text: String,
        images: Vec<ConversationImage>,
    },
    Assistant(NormalizedBlock),
    Compaction {
        summary: String,
        previous_revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedUsage {
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub context_usage: Option<crate::ipc::ContextUsage>,
    pub provider_details: Option<Value>,
}
