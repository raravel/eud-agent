use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider_runtime::{RunId, RunIdentity};

use super::{DirectToolCall, DirectToolResult};

const MAX_TOOL_ERROR_CHARS: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DurableToolCompletion {
    pub sequence: u64,
    pub run_id: RunId,
    pub request_id: String,
    pub call_id: Option<String>,
    pub name: String,
    pub result: Value,
    pub is_error: bool,
}

pub(super) fn tool_result(
    call: &DirectToolCall,
    outcome: Result<Value, String>,
) -> DirectToolResult {
    match outcome {
        Ok(value) => DirectToolResult {
            id: call.id.clone(),
            name: call.name.clone(),
            result: value,
            is_error: false,
        },
        Err(message) => DirectToolResult {
            id: call.id.clone(),
            name: call.name.clone(),
            result: Value::String(message.chars().take(MAX_TOOL_ERROR_CHARS).collect()),
            is_error: true,
        },
    }
}

pub(super) fn durable_completion(
    sequence: u64,
    identity: &RunIdentity,
    call: &DirectToolCall,
    result: &DirectToolResult,
) -> DurableToolCompletion {
    DurableToolCompletion {
        sequence,
        run_id: identity.run_id,
        request_id: identity.request_id.clone(),
        call_id: (!call.id.is_empty()).then(|| call.id.clone()),
        name: call.name.clone(),
        result: result.result.clone(),
        is_error: result.is_error,
    }
}
