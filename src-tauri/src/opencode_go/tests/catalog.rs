use super::*;

#[test]
fn production_adapter_exposes_only_transport_configuration() {
    let adapter = OpenCodeGoAdapter::new_for_test(
        reqwest::Client::new(),
        "http://127.0.0.1:1/v1".to_string(),
        "http://127.0.0.1:1/models".to_string(),
        "http://127.0.0.1:1/models.dev".to_string(),
        "fixture-key".to_string(),
    );

    assert_eq!(adapter.provider(), ProviderId::OpencodeGo);
    assert_eq!(adapter.loop_kind(), AdapterLoopKind::DirectSteps);
}

#[test]
fn models_dev_metadata_routes_arbitrary_future_models() {
    let metadata = json!({
        "opencode-go": {
            "npm": "@ai-sdk/openai-compatible",
            "models": {
                "provider-future-chat": {
                    "name": "Provider Future Chat",
                    "description": "Chat model",
                    "attachment": true,
                    "tool_call": true,
                    "structured_output": true,
                    "limit": {"context": 123456, "output": 65432}
                },
                "provider-future-response": {
                    "name": "Provider Future Response",
                    "provider": {"npm": "@ai-sdk/openai"}
                },
                "provider-future-messages": {
                    "name": "Provider Future Messages",
                    "provider": {"npm": "@ai-sdk/anthropic"},
                    "limit": {"output": 32768}
                },
                "provider-unknown-wire": {
                    "provider": {"npm": "@ai-sdk/future"}
                }
            }
        }
    });
    let models = parse_models_dev(&metadata).unwrap();
    assert_eq!(models.len(), 3);
    assert_eq!(
        models
            .iter()
            .find(|model| model.id == "provider-future-chat")
            .unwrap()
            .wire,
        OpenCodeGoWire::ChatCompletions
    );
    assert_eq!(
        models
            .iter()
            .find(|model| model.id == "provider-future-response")
            .unwrap()
            .wire,
        OpenCodeGoWire::Responses
    );
    assert_eq!(
        models
            .iter()
            .find(|model| model.id == "provider-future-messages")
            .unwrap()
            .wire,
        OpenCodeGoWire::AnthropicMessages
    );
    let chat = models
        .iter()
        .find(|model| model.id == "provider-future-chat")
        .unwrap()
        .provider_model(None);
    assert!(chat.capabilities.vision);
    assert!(chat.capabilities.tool_calls);
    assert!(chat.capabilities.strict_structured_output);
    assert_eq!(chat.capabilities.context_window, Some(123_456));
    assert!(chat.privacy.is_none());
}

#[tokio::test]
async fn fake_catalog_server_validates_bearer_and_live_metadata_join() {
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse as _;
    use axum::routing::get;

    let app = axum::Router::new().route(
        "/models",
        get(|headers: HeaderMap| async move {
            if headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                != Some("Bearer test-key")
            {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            axum::Json(json!({
                "data": [
                    {"id": "provider-future-response"},
                    {"id": "provider-future-chat"},
                    {"id": "provider-metadata-pending"}
                ]
            }))
            .into_response()
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let metadata = parse_models_dev(&json!({
        "opencode-go": {
            "npm": "@ai-sdk/openai-compatible",
            "models": {
                "provider-future-response": {
                    "name": "Provider Future Response",
                    "provider": {"npm": "@ai-sdk/openai"}
                },
                "provider-future-chat": {
                    "name": "Provider Future Chat"
                }
            }
        }
    }))
    .unwrap();
    let client = reqwest::Client::new();
    let base_url = format!("http://{address}");
    let live_ids = fetch_live_ids_at(&client, "test-key", &base_url)
        .await
        .unwrap();
    let models = join_live_catalog(&live_ids, &metadata);
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["provider-future-response", "provider-future-chat"]
    );
    assert!(
        models[0]
            .provider_model(Some("provider-future-response"))
            .is_default
    );
    assert_eq!(
        fetch_live_ids_at(&client, "bad-key", &base_url).await,
        Err("provider_not_authenticated".to_string())
    );
    server.abort();
}

#[test]
fn wire_requests_use_session_and_protocol_specific_authentication_headers() {
    let client = reqwest::Client::new();
    for wire in [
        OpenCodeGoWire::Responses,
        OpenCodeGoWire::ChatCompletions,
        OpenCodeGoWire::AnthropicMessages,
    ] {
        let request = authenticated_request(
            &client,
            "https://example.test".to_string(),
            wire,
            "test-key",
            "session-a",
        )
        .build()
        .unwrap();
        assert_eq!(
            request
                .headers()
                .get("x-opencode-session")
                .and_then(|value| value.to_str().ok()),
            Some("session-a")
        );
        if wire == OpenCodeGoWire::AnthropicMessages {
            assert_eq!(
                request
                    .headers()
                    .get("x-api-key")
                    .and_then(|value| value.to_str().ok()),
                Some("test-key")
            );
            assert!(!request.headers().contains_key("authorization"));
            assert_eq!(
                request
                    .headers()
                    .get("anthropic-version")
                    .and_then(|value| value.to_str().ok()),
                Some("2023-06-01")
            );
        } else {
            assert_eq!(
                request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer test-key")
            );
            assert!(!request.headers().contains_key("x-api-key"));
        }
    }
}
