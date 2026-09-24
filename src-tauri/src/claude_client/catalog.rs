//! Claude model catalog from the Anthropic Models API using the app-profile subscription token.
//!
//! The Claude CLI has no machine-readable model discovery command, but the same OAuth
//! credential it stores in `.credentials.json` (see `credentials`) is accepted by
//! `GET /v1/models` when sent as a bearer token with the `oauth-2025-04-20` beta header. The
//! catalog always keeps the `provider-default` entry first so a fetch failure degrades to
//! CLI-selected behavior.

use serde_json::Value;

use crate::provider::{ModelCapabilities, ProviderId, ProviderModel, ReasoningLevel};

pub(crate) const CLAUDE_PROVIDER_DEFAULT: &str = "provider-default";
const MODELS_URL: &str = "https://api.anthropic.com/v1/models";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const PAGE_LIMIT: usize = 1000;
const MAX_PAGES: usize = 4;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_MODEL_ID_BYTES: usize = 256;

pub fn provider_default_model(selected: Option<&str>) -> ProviderModel {
    ProviderModel {
        provider: ProviderId::ClaudeCode,
        model: CLAUDE_PROVIDER_DEFAULT.to_string(),
        display_name: "Claude Code 기본 모델".to_string(),
        description: "Claude Code가 현재 계정과 배포 기준으로 모델을 선택합니다.".to_string(),
        is_default: selected == Some(CLAUDE_PROVIDER_DEFAULT),
        capabilities: ModelCapabilities {
            vision: true,
            tool_calls: true,
            strict_structured_output: true,
            reasoning_levels: Vec::new(),
            native_compaction: true,
            context_window: None,
            hosted_web_search: false,
        },
        privacy: None,
    }
}

/// Synthetic entry for a session already bound to a catalog model while the live catalog is
/// unavailable. Only the saved reasoning level is advertised so the UI keeps the exact binding.
pub fn bound_model(
    model: &str,
    reasoning: Option<&crate::provider::ReasoningSelection>,
) -> Result<ProviderModel, String> {
    validate_model_id(model)?;
    if model == CLAUDE_PROVIDER_DEFAULT {
        return Ok(provider_default_model(Some(model)));
    }
    let reasoning_levels = reasoning
        .and_then(|selection| {
            [
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::Xhigh,
                ReasoningLevel::Max,
            ]
            .into_iter()
            .find(|level| level.as_str() == selection.level)
        })
        .into_iter()
        .collect();
    Ok(ProviderModel {
        provider: ProviderId::ClaudeCode,
        model: model.to_string(),
        display_name: model.to_string(),
        description: "모델 카탈로그를 불러오지 못해 저장된 선택만 표시합니다.".to_string(),
        is_default: true,
        capabilities: ModelCapabilities {
            vision: true,
            tool_calls: true,
            strict_structured_output: true,
            reasoning_levels,
            native_compaction: true,
            context_window: None,
            hosted_web_search: false,
        },
        privacy: None,
    })
}

/// A model id the CLI accepts on `--model`: bounded, printable, and not flag-shaped.
pub(crate) fn validate_model_id(model: &str) -> Result<(), String> {
    if model == CLAUDE_PROVIDER_DEFAULT {
        return Ok(());
    }
    let valid = !model.is_empty()
        && model.len() <= MAX_MODEL_ID_BYTES
        && !model.starts_with('-')
        && model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'));
    valid
        .then_some(())
        .ok_or_else(|| "provider_model_unavailable".to_string())
}

pub(crate) async fn fetch_catalog(
    client: &reqwest::Client,
    access_token: &str,
    selected: Option<&str>,
) -> Result<Vec<ProviderModel>, String> {
    fetch_catalog_at(client, MODELS_URL, access_token, selected).await
}

pub(crate) async fn fetch_catalog_at(
    client: &reqwest::Client,
    models_url: &str,
    access_token: &str,
    selected: Option<&str>,
) -> Result<Vec<ProviderModel>, String> {
    let mut models = vec![provider_default_model(selected)];
    let mut after_id: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut request = client
            .get(models_url)
            .query(&[("limit", PAGE_LIMIT.to_string())])
            .bearer_auth(access_token)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("anthropic-beta", OAUTH_BETA);
        if let Some(after_id) = after_id.as_deref() {
            request = request.query(&[("after_id", after_id)]);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "provider_catalog_unavailable".to_string())?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        let bytes = bounded_body(response).await?;
        let page: Value =
            serde_json::from_slice(&bytes).map_err(|_| "provider_protocol_changed".to_string())?;
        let (mut page_models, has_more, last_id) = parse_page(&page, selected)?;
        models.append(&mut page_models);
        match (has_more, last_id) {
            (true, Some(last_id)) if after_id.as_deref() != Some(last_id.as_str()) => {
                after_id = Some(last_id);
            }
            _ => break,
        }
    }
    Ok(models)
}

fn parse_page(
    page: &Value,
    selected: Option<&str>,
) -> Result<(Vec<ProviderModel>, bool, Option<String>), String> {
    let rows = page
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    let models = rows
        .iter()
        .map(|row| parse_model(row, selected))
        .collect::<Result<Vec<_>, _>>()?;
    let has_more = page
        .get("has_more")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let last_id = page
        .get("last_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok((models, has_more, last_id))
}

fn parse_model(row: &Value, selected: Option<&str>) -> Result<ProviderModel, String> {
    let id = row
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    validate_model_id(id).map_err(|_| "provider_protocol_changed".to_string())?;
    let display_name = row
        .get("display_name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
        .map(|name| name.chars().take(128).collect::<String>())
        .unwrap_or_else(|| id.to_string());
    let capabilities = row.get("capabilities");
    let supported = |pointer: &str| {
        capabilities
            .and_then(|value| value.pointer(pointer))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let reasoning_levels = if supported("/effort/supported") {
        [
            ("/effort/low/supported", ReasoningLevel::Low),
            ("/effort/medium/supported", ReasoningLevel::Medium),
            ("/effort/high/supported", ReasoningLevel::High),
            ("/effort/xhigh/supported", ReasoningLevel::Xhigh),
            ("/effort/max/supported", ReasoningLevel::Max),
        ]
        .into_iter()
        .filter(|(pointer, _)| supported(pointer))
        .map(|(_, level)| level)
        .collect()
    } else {
        Vec::new()
    };
    Ok(ProviderModel {
        provider: ProviderId::ClaudeCode,
        model: id.to_string(),
        display_name,
        description: "Claude Code가 이 모델로 실행됩니다. 구독 플랜에서 허용되지 않으면 턴 시작 시 거부됩니다."
            .to_string(),
        is_default: selected == Some(id),
        capabilities: ModelCapabilities {
            vision: supported("/image_input/supported"),
            tool_calls: true,
            strict_structured_output: supported("/structured_outputs/supported"),
            reasoning_levels,
            native_compaction: true,
            context_window: row.get("max_input_tokens").and_then(Value::as_u64),
            hosted_web_search: false,
        },
        privacy: None,
    })
}

async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err("provider_protocol_changed".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "provider_transport_closed".to_string())?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("provider_protocol_changed".to_string());
    }
    Ok(bytes.to_vec())
}

fn status_error(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        401 => "provider_not_authenticated",
        402 | 403 => "provider_quota_exhausted",
        429 => "provider_rate_limited",
        500..=599 => "provider_transport_closed",
        _ => "provider_protocol_changed",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page() -> Value {
        json!({
            "data": [
                {
                    "id": "claude-opus-5",
                    "display_name": "Claude Opus 5",
                    "max_input_tokens": 1_000_000,
                    "max_tokens": 128_000,
                    "capabilities": {
                        "image_input": {"supported": true},
                        "structured_outputs": {"supported": true},
                        "effort": {
                            "supported": true,
                            "low": {"supported": true},
                            "medium": {"supported": true},
                            "high": {"supported": true},
                            "xhigh": {"supported": true},
                            "max": {"supported": false}
                        }
                    }
                },
                {
                    "id": "claude-haiku-4-5-20251001",
                    "display_name": "Claude Haiku 4.5",
                    "max_input_tokens": 200_000,
                    "capabilities": {
                        "image_input": {"supported": true},
                        "effort": {"supported": false}
                    }
                }
            ],
            "has_more": false,
            "first_id": "claude-opus-5",
            "last_id": "claude-haiku-4-5-20251001"
        })
    }

    #[test]
    fn page_maps_effort_capabilities_and_context_window() {
        let (models, has_more, last_id) = parse_page(&page(), Some("claude-opus-5")).unwrap();
        assert!(!has_more);
        assert_eq!(last_id.as_deref(), Some("claude-haiku-4-5-20251001"));
        assert_eq!(models.len(), 2);
        let opus = &models[0];
        assert_eq!(opus.model, "claude-opus-5");
        assert_eq!(opus.display_name, "Claude Opus 5");
        assert!(opus.is_default);
        assert_eq!(opus.capabilities.context_window, Some(1_000_000));
        assert!(opus.capabilities.vision);
        assert!(opus.capabilities.strict_structured_output);
        assert_eq!(
            opus.capabilities.reasoning_levels,
            vec![
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::Xhigh
            ]
        );
        let haiku = &models[1];
        assert!(!haiku.is_default);
        assert!(haiku.capabilities.reasoning_levels.is_empty());
        assert!(!haiku.capabilities.strict_structured_output);
        assert_eq!(haiku.capabilities.context_window, Some(200_000));
    }

    #[test]
    fn page_rejects_missing_or_unsafe_ids() {
        assert!(parse_page(&json!({"models": []}), None).is_err());
        assert!(parse_page(&json!({"data": [{"display_name": "x"}]}), None).is_err());
        assert!(parse_page(&json!({"data": [{"id": "--model"}]}), None).is_err());
        assert!(parse_page(&json!({"data": [{"id": "bad id\n"}]}), None).is_err());
    }

    #[test]
    fn bound_model_keeps_saved_binding_without_a_catalog() {
        let saved = crate::provider::ReasoningSelection {
            level: "xhigh".to_string(),
        };
        let model = bound_model("claude-opus-5", Some(&saved)).unwrap();
        assert_eq!(model.model, "claude-opus-5");
        assert!(model.is_default);
        assert_eq!(
            model.capabilities.reasoning_levels,
            vec![ReasoningLevel::Xhigh]
        );
        assert!(bound_model("claude-opus-5", None)
            .unwrap()
            .capabilities
            .reasoning_levels
            .is_empty());
        assert_eq!(
            bound_model(CLAUDE_PROVIDER_DEFAULT, None).unwrap().model,
            CLAUDE_PROVIDER_DEFAULT
        );
        assert!(bound_model("-p", None).is_err());
    }

    #[test]
    fn model_id_validation_accepts_cli_shapes_only() {
        assert!(validate_model_id(CLAUDE_PROVIDER_DEFAULT).is_ok());
        assert!(validate_model_id("claude-opus-5").is_ok());
        assert!(validate_model_id("claude-opus-4-5-20251101").is_ok());
        assert!(validate_model_id("").is_err());
        assert!(validate_model_id("-p").is_err());
        assert!(validate_model_id("opus 5").is_err());
        assert!(validate_model_id(&"a".repeat(257)).is_err());
    }

    #[tokio::test]
    async fn catalog_keeps_provider_default_first_and_follows_pagination() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut seen = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = vec![0_u8; 8192];
                let read = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer)
                    .await
                    .unwrap();
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let second = request.contains("after_id=claude-opus-5");
                seen.push(request);
                let body = if second {
                    json!({"data": [{"id": "claude-sonnet-5", "display_name": "Claude Sonnet 5"}], "has_more": false, "last_id": "claude-sonnet-5"})
                } else {
                    json!({"data": [{"id": "claude-opus-5", "display_name": "Claude Opus 5"}], "has_more": true, "last_id": "claude-opus-5"})
                }
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes())
                    .await
                    .unwrap();
            }
            seen
        });
        let client = reqwest::Client::builder().build().unwrap();
        let models = fetch_catalog_at(
            &client,
            &format!("http://{address}/v1/models"),
            "token-value",
            Some("claude-sonnet-5"),
        )
        .await
        .unwrap();
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2);
        for request in &requests {
            assert!(
                request.contains("authorization: Bearer token-value")
                    || request.contains("Authorization: Bearer token-value")
            );
            assert!(request.contains("anthropic-beta: oauth-2025-04-20"));
            assert!(request.contains("anthropic-version: 2023-06-01"));
            assert!(request.contains("limit=1000"));
        }
        assert_eq!(
            models
                .iter()
                .map(|model| model.model.as_str())
                .collect::<Vec<_>>(),
            vec![CLAUDE_PROVIDER_DEFAULT, "claude-opus-5", "claude-sonnet-5"]
        );
        assert!(models[2].is_default);
        assert!(!models[0].is_default);
    }

    #[tokio::test]
    async fn catalog_maps_unauthorized_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0_u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer).await;
            tokio::io::AsyncWriteExt::write_all(
                &mut socket,
                b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
            )
            .await
            .unwrap();
        });
        let client = reqwest::Client::builder().build().unwrap();
        assert_eq!(
            fetch_catalog_at(
                &client,
                &format!("http://{address}/v1/models"),
                "token-value",
                None
            )
            .await,
            Err("provider_not_authenticated".to_string())
        );
    }
}
