use std::collections::HashSet;

use serde_json::{json, Value};

use crate::{
    antigravity_auth::{AntigravityCredential, ANTIGRAVITY_USER_AGENT, CLOUD_CODE_ENDPOINT},
    provider::ProviderModel,
    provider_runtime::ProviderRuntimeError,
};

use super::LiveAntigravityModel;

const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;

pub async fn fetch_catalog(
    client: &reqwest::Client,
    credential: &AntigravityCredential,
    selected: Option<&str>,
) -> Result<Vec<ProviderModel>, String> {
    fetch_live_catalog_at(client, credential, CLOUD_CODE_ENDPOINT)
        .await
        .map_err(|error| match error {
            ProviderRuntimeError::Protocol(code) | ProviderRuntimeError::Transport(code) => code,
            other => other.to_string(),
        })
        .map(|models| {
            models
                .into_iter()
                .map(|model| model.provider_model(selected))
                .collect()
        })
}

pub(super) async fn fetch_live_catalog_at(
    client: &reqwest::Client,
    credential: &AntigravityCredential,
    endpoint: &str,
) -> Result<Vec<LiveAntigravityModel>, ProviderRuntimeError> {
    let response = client
        .post(format!("{endpoint}/v1internal:fetchAvailableModels"))
        .bearer_auth(&credential.access_token)
        .header(reqwest::header::USER_AGENT, ANTIGRAVITY_USER_AGENT)
        .json(&json!({}))
        .send()
        .await
        .map_err(|_| ProviderRuntimeError::Transport("provider_catalog_unavailable".into()))?;
    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CATALOG_BYTES as u64)
    {
        return Err(ProviderRuntimeError::Protocol(
            "provider_protocol_changed".into(),
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| ProviderRuntimeError::Transport("provider_catalog_unavailable".into()))?;
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(ProviderRuntimeError::Protocol(
            "provider_protocol_changed".into(),
        ));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ProviderRuntimeError::Protocol("provider_protocol_changed".into()))?;
    parse_live_catalog(&value).map_err(ProviderRuntimeError::Protocol)
}

fn parse_live_catalog(value: &Value) -> Result<Vec<LiveAntigravityModel>, String> {
    let models = value
        .get("models")
        .and_then(Value::as_object)
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    let mut ids = Vec::with_capacity(models.len());
    let mut seen = HashSet::with_capacity(models.len());
    for id in value
        .get("agentModelSorts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|sort| {
            sort.get("groups")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .flat_map(|group| {
            group
                .get("modelIds")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(Value::as_str)
    {
        if models.contains_key(id) && seen.insert(id) {
            ids.push(id);
        }
    }
    for id in models.keys() {
        if seen.insert(id) {
            ids.push(id);
        }
    }
    Ok(ids
        .into_iter()
        .filter_map(|id| parse_live_model(id, &models[id]))
        .collect())
}

fn parse_live_model(id: &str, value: &Value) -> Option<LiveAntigravityModel> {
    if id.is_empty()
        || id.len() > 256
        || id.chars().any(char::is_control)
        || value.get("isInternal") == Some(&Value::Bool(true))
    {
        return None;
    }
    Some(LiveAntigravityModel {
        id: id.to_string(),
        display_name: bounded_string(value.get("displayName"), 256)
            .unwrap_or_else(|| id.to_string()),
        supports_images: value
            .get("supportsImages")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        supports_thinking: value
            .get("supportsThinking")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        thinking_budget: positive_u64(value.get("thinkingBudget")),
        context_window: positive_u64(value.get("maxTokens")),
        max_output_tokens: positive_u64(value.get("maxOutputTokens")),
        api_provider: bounded_string(value.get("apiProvider"), 128),
        model_provider: bounded_string(value.get("modelProvider"), 128),
    })
}

fn bounded_string(value: Option<&Value>, max_len: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= max_len)
        .map(str::to_string)
}

fn positive_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64).filter(|value| *value > 0)
}

pub(super) fn status_error(status: reqwest::StatusCode) -> ProviderRuntimeError {
    let code = match status.as_u16() {
        401 => "provider_cloud_code_unauthorized",
        403 => "provider_quota_exhausted",
        404 => "provider_protocol_changed",
        429 => "provider_rate_limited",
        500..=599 => "provider_transport_closed",
        _ => "provider_protocol_changed",
    };
    if status.is_server_error() {
        ProviderRuntimeError::Transport(code.into())
    } else {
        ProviderRuntimeError::Protocol(code.into())
    }
}
