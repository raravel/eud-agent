use std::collections::BTreeMap;
use std::net::IpAddr;

use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::opencode_go::SseDecoder;
use crate::provider::{
    ModelCapabilities, ProviderConversationState, ProviderId, ProviderModel, ReasoningLevel,
    ReasoningSelection,
};
use crate::provider_runtime::{
    AdapterEvent, AdapterEventKind, AdapterFuture, AdapterLoopKind, AdapterOutput,
    AdapterRequestKind, AdapterStepOutcome, AdapterStepRequest, ConversationItem, NormalizedBlock,
    NormalizedUsage, ProviderAdapter, ProviderRuntimeError, RunIdentity,
};
use crate::provider_tool_loop::{DirectToolCall, DirectToolResult, NormalizedAssistantStep};

mod adapter;
mod request;
mod step;
mod stream;
use request::*;
use stream::*;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const STRUCTURED_OUTPUT_NAME: &str = "structured_result";

pub fn normalize_base_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 2_048 || value.chars().any(char::is_control) {
        return Err("provider_endpoint_invalid".to_string());
    }
    let url = reqwest::Url::parse(value).map_err(|_| "provider_endpoint_invalid".to_string())?;
    if url.cannot_be_a_base()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("provider_endpoint_invalid".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "provider_endpoint_invalid".to_string())?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => return Err("provider_endpoint_invalid".to_string()),
    }
    let append_v1 = url.path() == "/";
    let mut normalized = url.as_str().trim_end_matches('/').to_string();
    if append_v1 {
        normalized.push_str("/v1");
    }
    Ok(normalized)
}

pub fn validate_model(model: &str) -> Result<&str, String> {
    let model = model.trim();
    if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
        Err("provider_model_unavailable".to_string())
    } else {
        Ok(model)
    }
}

pub fn provider_model(model: &str, selected: Option<&str>) -> Result<ProviderModel, String> {
    let model = validate_model(model)?;
    Ok(ProviderModel {
        provider: ProviderId::Ollama,
        model: model.to_string(),
        display_name: model.to_string(),
        description: "Ollama OpenAI 호환 API 모델 · 실제 기능은 설치된 모델에 따라 달라집니다."
            .to_string(),
        is_default: selected == Some(model),
        capabilities: ModelCapabilities {
            vision: true,
            tool_calls: true,
            strict_structured_output: true,
            reasoning_levels: vec![
                ReasoningLevel::None,
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::Max,
            ],
            native_compaction: false,
            context_window: None,
            hosted_web_search: false,
        },
        privacy: None,
    })
}

fn validate_reasoning(reasoning: Option<&ReasoningSelection>) -> Result<(), String> {
    let Some(reasoning) = reasoning else {
        return Ok(());
    };
    if matches!(
        reasoning.level.as_str(),
        "none" | "low" | "medium" | "high" | "max"
    ) {
        Ok(())
    } else {
        Err("provider_capability_unsupported".to_string())
    }
}

pub async fn probe(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<(), String> {
    let base_url = normalize_base_url(base_url)?;
    let mut request = client.get(format!("{base_url}/models"));
    if let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) {
        request = request.bearer_auth(api_key.trim());
    }
    let response = request
        .send()
        .await
        .map_err(|_| "provider_transport_closed".to_string())?;
    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }
    let body = bounded_body(response).await?;
    let value: Value =
        serde_json::from_slice(&body).map_err(|_| "provider_protocol_changed".to_string())?;
    if !value.get("data").is_some_and(Value::is_array) {
        return Err("provider_protocol_changed".to_string());
    }
    Ok(())
}

pub struct ProductionOllamaAdapter {
    client: reqwest::Client,
    api_key: Option<Zeroizing<String>>,
}

impl ProductionOllamaAdapter {
    pub(crate) fn new(api_key: Option<String>) -> Result<Self, ProviderRuntimeError> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(600))
            .user_agent(concat!("eud-agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| {
                ProviderRuntimeError::Transport("provider_transport_closed".to_string())
            })?;
        Ok(Self {
            client,
            api_key: api_key.map(Zeroizing::new),
        })
    }

    #[cfg(test)]
    fn with_client(client: reqwest::Client, api_key: Option<&str>) -> Self {
        Self {
            client,
            api_key: api_key.map(|value| Zeroizing::new(value.to_string())),
        }
    }
}

async fn send_event(
    events: &tokio::sync::mpsc::Sender<AdapterEvent>,
    identity: &RunIdentity,
    kind: AdapterEventKind,
) -> Result<(), ProviderRuntimeError> {
    events
        .send(AdapterEvent {
            identity: identity.clone(),
            kind,
        })
        .await
        .map_err(|_| ProviderRuntimeError::Cancelled)
}

async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err("provider response is too large".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "provider_transport_closed".to_string())?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("provider response is too large".to_string());
    }
    Ok(bytes.to_vec())
}

fn status_error(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        400 | 422 => "provider_capability_unsupported",
        401 => "provider_not_authenticated",
        402 | 403 => "provider_quota_exhausted",
        404 => "provider_model_unavailable",
        429 => "provider_rate_limited",
        500..=599 => "provider_transport_closed",
        _ => "provider_protocol_changed",
    }
    .to_string()
}

#[cfg(test)]
mod tests;
