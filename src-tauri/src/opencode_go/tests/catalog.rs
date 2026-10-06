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

#[tokio::test]
async fn zen_adapter_reports_and_seeds_only_its_own_provider() {
    let mut adapter = OpenCodeGoAdapter::new_for_test(
        reqwest::Client::new(),
        "http://127.0.0.1:1/v1".to_string(),
        "http://127.0.0.1:1/models".to_string(),
        "http://127.0.0.1:1/models.dev".to_string(),
        "fixture-key".to_string(),
    )
    .with_flavor(OpenCodeFlavor::Zen);

    assert_eq!(adapter.provider(), ProviderId::OpencodeZen);
    adapter
        .seed(ProviderConversationState::OpencodeZen {
            transcript_revision: 3,
        })
        .await
        .unwrap();
    assert!(adapter
        .seed(ProviderConversationState::OpencodeGo {
            transcript_revision: 3,
        })
        .await
        .is_err());
    assert_eq!(
        OpenCodeFlavor::from_provider(ProviderId::OpencodeZen),
        Some(OpenCodeFlavor::Zen)
    );
    assert_eq!(OpenCodeFlavor::from_provider(ProviderId::Ollama), None);
}

#[test]
fn zen_catalog_reads_the_opencode_key_and_drops_unsupported_wires() {
    let metadata = json!({
        "opencode": {
            "npm": "@ai-sdk/openai-compatible",
            "api": "https://opencode.ai/zen/v1",
            "models": {
                "muse-spark-1.3-contributor-free": {
                    "name": "Muse Spark 1.3 (free)",
                    "provider": {"npm": "@ai-sdk/openai"},
                    "tool_call": true,
                    "structured_output": true
                },
                "zen-chat": {"name": "Zen Chat", "tool_call": true},
                "zen-messages": {
                    "name": "Zen Messages",
                    "provider": {"npm": "@ai-sdk/anthropic"},
                    "limit": {"output": 32000}
                },
                "zen-gemini": {
                    "name": "Zen Gemini",
                    "provider": {"npm": "@ai-sdk/google"}
                }
            }
        }
    });
    let models = parse_models_dev(&metadata, OpenCodeFlavor::Zen).unwrap();
    let wire = |id: &str| {
        models
            .iter()
            .find(|model| model.id == id)
            .map(|model| model.wire)
    };
    assert_eq!(models.len(), 3);
    assert_eq!(
        wire("muse-spark-1.3-contributor-free"),
        Some(OpenCodeGoWire::Responses)
    );
    assert_eq!(wire("zen-chat"), Some(OpenCodeGoWire::ChatCompletions));
    assert_eq!(
        wire("zen-messages"),
        Some(OpenCodeGoWire::AnthropicMessages)
    );
    assert_eq!(wire("zen-gemini"), None);
    let muse = models
        .iter()
        .find(|model| model.id == "muse-spark-1.3-contributor-free")
        .unwrap()
        .provider_model(Some("muse-spark-1.3-contributor-free"));
    assert_eq!(muse.provider, ProviderId::OpencodeZen);
    assert!(muse.is_default);
    assert!(muse.capabilities.tool_calls);
    assert!(muse.capabilities.strict_structured_output);
    // The Go key is not part of the Zen document.
    assert_eq!(
        parse_models_dev(&metadata, OpenCodeFlavor::Go),
        Err("provider_protocol_changed".to_string())
    );
}

#[test]
fn go_and_zen_catalogs_and_continuations_never_cross() {
    let document = json!({
        "opencode-go": {
            "npm": "@ai-sdk/openai-compatible",
            "models": {"go-only": {"name": "Go Only"}, "shared-id": {"name": "Go Shared"}}
        },
        "opencode": {
            "npm": "@ai-sdk/openai-compatible",
            "models": {"zen-only": {"name": "Zen Only"}, "shared-id": {"name": "Zen Shared"}}
        }
    });
    let catalog = |flavor| {
        let mut rows = parse_models_dev(&document, flavor)
            .unwrap()
            .into_iter()
            .map(|model| {
                let row = model.provider_model(None);
                (row.model, row.display_name, row.provider)
            })
            .collect::<Vec<_>>();
        rows.sort();
        rows
    };
    assert_eq!(
        catalog(OpenCodeFlavor::Go),
        [
            ("go-only".into(), "Go Only".into(), ProviderId::OpencodeGo),
            (
                "shared-id".into(),
                "Go Shared".into(),
                ProviderId::OpencodeGo
            ),
        ]
    );
    assert_eq!(
        catalog(OpenCodeFlavor::Zen),
        [
            (
                "shared-id".into(),
                "Zen Shared".into(),
                ProviderId::OpencodeZen
            ),
            (
                "zen-only".into(),
                "Zen Only".into(),
                ProviderId::OpencodeZen
            ),
        ]
    );

    let go_continuation = Some(ProviderContinuation {
        provider: ProviderId::OpencodeGo,
        data: json!({"wire":"responses","opaque":{"id":"rs_go"}}),
    });
    assert!(continuation_opaque(ProviderId::OpencodeGo, &go_continuation, "responses").is_some());
    assert!(continuation_opaque(ProviderId::OpencodeZen, &go_continuation, "responses").is_none());
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
    let models = parse_models_dev(&metadata, OpenCodeFlavor::Go).unwrap();
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
    let metadata = parse_models_dev(
        &json!({
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
        }),
        OpenCodeFlavor::Go,
    )
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
