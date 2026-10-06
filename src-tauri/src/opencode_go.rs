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

/// One OpenCode gateway the shared wire adapter talks to. Go and Zen share
/// every wire, parser and transcript rule; they differ only in the provider
/// identity, inference base URL, models.dev provider key and API-key secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeFlavor {
    Go,
    Zen,
}

impl OpenCodeFlavor {
    pub fn provider(self) -> ProviderId {
        match self {
            Self::Go => ProviderId::OpencodeGo,
            Self::Zen => ProviderId::OpencodeZen,
        }
    }

    pub fn from_provider(provider: ProviderId) -> Option<Self> {
        match provider {
            ProviderId::OpencodeGo => Some(Self::Go),
            ProviderId::OpencodeZen => Some(Self::Zen),
            ProviderId::Codex
            | ProviderId::ClaudeCode
            | ProviderId::Antigravity
            | ProviderId::Ollama => None,
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            Self::Go => "https://opencode.ai/zen/go/v1",
            Self::Zen => "https://opencode.ai/zen/v1",
        }
    }

    fn models_dev_key(self) -> &'static str {
        match self {
            Self::Go => "opencode-go",
            Self::Zen => "opencode",
        }
    }
}

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
    provider: ProviderId,
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
            provider: self.provider,
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
    provider: ProviderId,
    client: &reqwest::Client,
    api_key: &str,
    selected_model: Option<&str>,
) -> Result<Vec<ProviderModel>, String> {
    let flavor = OpenCodeFlavor::from_provider(provider)
        .ok_or_else(|| "unsupported_operation".to_string())?;
    Ok(fetch_live_catalog(flavor, client, api_key)
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
    Ok(fetch_live_catalog(OpenCodeFlavor::Go, client, api_key)
        .await?
        .into_iter()
        .map(|model| {
            let wire = model.wire;
            (model.provider_model(None), wire)
        })
        .collect())
}

pub(crate) struct OpenCodeGoAdapter {
    flavor: OpenCodeFlavor,
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
