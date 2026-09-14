use super::*;

pub(super) const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const MODELS_DEV_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);
static MODELS_DEV_CACHE: tokio::sync::Mutex<
    Option<(tokio::time::Instant, Vec<LiveOpenCodeGoModel>)>,
> = tokio::sync::Mutex::const_new(None);

pub(super) fn wire_from_npm(npm: &str) -> Option<OpenCodeGoWire> {
    match npm {
        "@ai-sdk/openai" => Some(OpenCodeGoWire::Responses),
        "@ai-sdk/openai-compatible" => Some(OpenCodeGoWire::ChatCompletions),
        "@ai-sdk/anthropic" => Some(OpenCodeGoWire::AnthropicMessages),
        _ => None,
    }
}

pub(super) fn parse_models_dev(value: &Value) -> Result<Vec<LiveOpenCodeGoModel>, String> {
    let provider = value
        .get("opencode-go")
        .and_then(Value::as_object)
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    let provider_npm = provider
        .get("npm")
        .and_then(Value::as_str)
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    let models = provider
        .get("models")
        .and_then(Value::as_object)
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    Ok(models
        .iter()
        .filter_map(|(id, model)| {
            if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
                return None;
            }
            let npm = model
                .pointer("/provider/npm")
                .and_then(Value::as_str)
                .unwrap_or(provider_npm);
            let wire = wire_from_npm(npm)?;
            let name = model
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty() && name.len() <= 256)
                .unwrap_or(id)
                .to_string();
            let description = model
                .get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|description| description.len() <= 1024)
                .unwrap_or("")
                .to_string();
            let vision = model
                .get("attachment")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || model
                    .pointer("/modalities/input")
                    .and_then(Value::as_array)
                    .is_some_and(|modalities| {
                        modalities
                            .iter()
                            .any(|modality| modality.as_str() == Some("image"))
                    });
            Some(LiveOpenCodeGoModel {
                id: id.clone(),
                name,
                description,
                wire,
                vision,
                tool_calls: model
                    .get("tool_call")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                structured_output: model
                    .get("structured_output")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                context_window: model
                    .pointer("/limit/context")
                    .and_then(Value::as_u64)
                    .filter(|value| *value > 0),
                max_output_tokens: model
                    .pointer("/limit/output")
                    .and_then(Value::as_u64)
                    .filter(|value| *value > 0),
            })
        })
        .collect())
}

pub(super) async fn fetch_models_dev_at(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<LiveOpenCodeGoModel>, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "provider_catalog_unavailable".to_string())?;
    if !response.status().is_success() {
        return Err("provider_catalog_unavailable".to_string());
    }
    let bytes = bounded_body(response).await?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "provider_protocol_changed".to_string())?;
    parse_models_dev(&value)
}

pub(super) async fn fetch_models_dev(
    client: &reqwest::Client,
) -> Result<Vec<LiveOpenCodeGoModel>, String> {
    let mut cache = MODELS_DEV_CACHE.lock().await;
    if let Some((loaded_at, models)) = cache.as_ref() {
        if loaded_at.elapsed() < MODELS_DEV_CACHE_TTL {
            return Ok(models.clone());
        }
    }
    let models = fetch_models_dev_at(client, MODELS_DEV_URL).await?;
    *cache = Some((tokio::time::Instant::now(), models.clone()));
    Ok(models)
}

pub(super) async fn fetch_live_ids_at(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
) -> Result<Vec<String>, String> {
    let response = client
        .get(format!("{base_url}/models"))
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|_| "provider_catalog_unavailable".to_string())?;
    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }
    let bytes = bounded_body(response).await?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "provider_protocol_changed".to_string())?;
    let rows = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    rows.iter()
        .map(|row| {
            row.get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
                .map(str::to_string)
                .ok_or_else(|| "provider_protocol_changed".to_string())
        })
        .collect()
}

pub(super) fn join_live_catalog(
    live_ids: &[String],
    metadata: &[LiveOpenCodeGoModel],
) -> Vec<LiveOpenCodeGoModel> {
    let metadata = metadata
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect::<std::collections::HashMap<_, _>>();
    live_ids
        .iter()
        .filter_map(|id| metadata.get(id.as_str()).map(|model| (*model).clone()))
        .collect()
}

pub(super) async fn fetch_live_catalog(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<Vec<LiveOpenCodeGoModel>, String> {
    let (live_ids, metadata) = tokio::try_join!(
        fetch_live_ids_at(client, api_key, BASE_URL),
        fetch_models_dev(client)
    )?;
    Ok(join_live_catalog(&live_ids, &metadata))
}
