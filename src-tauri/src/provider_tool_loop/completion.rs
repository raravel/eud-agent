use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

use crate::provider_runtime::{RunId, RunIdentity};

use super::{DirectToolCall, DirectToolResult};

const MAX_TOOL_ERROR_CHARS: usize = 16_384;
/// A durable completion keeps a result verbatim up to this serialized size.
/// Larger observations (rendered maps, long reads) still reach the model in
/// full, but the receipt records only their identity so a long Map run of
/// multi-megabyte renders never outgrows the run receipt bound.
const MAX_DURABLE_RESULT_BYTES: usize = 64 * 1024;

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
        result: durable_result(&result.result),
        is_error: result.is_error,
    }
}

/// The receipt form of a tool result: an MCP image envelope keeps its
/// metadata with the PNG replaced by its byte count and SHA-256; any other
/// result above [`MAX_DURABLE_RESULT_BYTES`] becomes a digest record.
fn durable_result(result: &Value) -> Value {
    let mut durable = result.clone();
    if let Some(image) = durable.get_mut("image").and_then(Value::as_object_mut) {
        if let Some(data) = image.get("data").and_then(Value::as_str) {
            let bytes = data.len();
            let digest = sha256_hex(data.as_bytes());
            image.remove("data");
            image.insert("dataBytes".to_string(), json!(bytes));
            image.insert("dataSha256".to_string(), json!(digest));
        }
    }
    let bytes = serde_json::to_vec(&durable).unwrap_or_default();
    if bytes.len() <= MAX_DURABLE_RESULT_BYTES {
        return durable;
    }
    json!({
        "receiptOmitted": true,
        "bytes": bytes.len(),
        "sha256": sha256_hex(&bytes),
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}
