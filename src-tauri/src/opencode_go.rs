use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::provider::{ModelCapabilities, ProviderConversationState, ProviderId, ProviderModel};
use crate::provider_runtime::{
    AdapterEvent, AdapterEventKind, AdapterFuture, AdapterLoopKind, AdapterOutput,
    AdapterRequestKind, AdapterStepOutcome, AdapterStepRequest, ConversationItem, NormalizedBlock,
    NormalizedUsage, ProviderAdapter, ProviderContinuation, ProviderRuntimeError, RunIdentity,
};
use crate::provider_secrets::ProviderSecretStore;
use crate::provider_tool_loop::{DirectToolCall, DirectToolResult, NormalizedAssistantStep};

mod adapter;
mod anthropic_history;
mod block_order;
mod catalog;
mod chat_history;
mod continuation_blocks;
mod credential;
mod history;
mod parser;
mod request;
mod responses_history;
mod sse;
mod step;
mod stream;
mod transport;
use anthropic_history::*;
use catalog::*;
use chat_history::*;
use continuation_blocks::*;
use credential::*;
use history::*;
use parser::*;
use request::*;
use responses_history::*;
use stream::*;
use transport::*;

const BASE_URL: &str = "https://opencode.ai/zen/go/v1";
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const STRUCTURED_TOOL: &str = "submit_structured_result";
const TASK_COMPILER_RESPONSE_INSTRUCTIONS: &str = "You are the coding assistant's structured task-state compiler. Return exactly one call to submit_structured_result with the complete result. This function is a response channel, not a project tool, so the prohibition on tools does not prohibit submitting it. Do not emit the JSON as ordinary text. Make one pass through the input and submit; do not repeatedly reconsider equivalent fact IDs or quote lengths.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeGoWire {
    Responses,
    ChatCompletions,
    AnthropicMessages,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveOpenCodeGoModel {
    id: String,
    name: String,
    description: String,
    wire: OpenCodeGoWire,
    vision: bool,
    tool_calls: bool,
    structured_output: bool,
    context_window: Option<u64>,
    max_output_tokens: Option<u64>,
}

impl LiveOpenCodeGoModel {
    fn provider_model(&self, selected: Option<&str>) -> ProviderModel {
        ProviderModel {
            provider: ProviderId::OpencodeGo,
            model: self.id.clone(),
            display_name: self.name.clone(),
            description: self.description.clone(),
            is_default: selected == Some(self.id.as_str()),
            capabilities: ModelCapabilities {
                vision: self.vision,
                tool_calls: self.tool_calls,
                strict_structured_output: self.structured_output,
                reasoning_levels: Vec::new(),
                native_compaction: false,
                context_window: self.context_window,
                hosted_web_search: false,
            },
            privacy: None,
        }
    }
}

pub async fn fetch_catalog(
    client: &reqwest::Client,
    api_key: &str,
    selected_model: Option<&str>,
) -> Result<Vec<ProviderModel>, String> {
    Ok(fetch_live_catalog(client, api_key)
        .await?
        .into_iter()
        .map(|model| model.provider_model(selected_model))
        .collect())
}

#[cfg(test)]
pub(crate) async fn fetch_live_contract_catalog(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<Vec<(ProviderModel, OpenCodeGoWire)>, String> {
    Ok(fetch_live_catalog(client, api_key)
        .await?
        .into_iter()
        .map(|model| {
            let wire = model.wire;
            (model.provider_model(None), wire)
        })
        .collect())
}

pub(crate) struct OpenCodeGoAdapter {
    client: reqwest::Client,
    credential: OpenCodeGoCredential,
    inference_base_url: String,
    live_models_base_url: String,
    metadata_url: String,
    models_by_run: HashMap<crate::provider_runtime::RunId, LiveOpenCodeGoModel>,
}

struct ParsedStream {
    deferred_blocks: Vec<NormalizedBlock>,
    step: NormalizedAssistantStep,
    continuation: Option<ProviderContinuation>,
}

#[derive(Default)]
pub(crate) struct SseDecoder {
    buffer: Vec<u8>,
}

pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[cfg(test)]
mod tests;
