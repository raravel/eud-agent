mod batch;
mod cancellation;
mod stream;
mod structured;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    body::Bytes,
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use parking_lot::Mutex;
use serde_json::{json, Value};

use super::*;
use crate::{
    antigravity_auth::{AntigravityAuthHandle, AntigravityCredential},
    provider::ProviderConversationState,
    provider_runtime::{
        AdapterEventKind, AdapterOutput, AdapterRequestKind, AdapterStepOutcome,
        AdapterStepRequest, AgentTurnInput, BindingSnapshot, ConversationItem, NormalizedBlock,
        ProviderAdapter, ProviderContinuation, ProviderRuntimeError, RunId, RunPolicy,
        StructuredJobKind,
    },
    provider_tool_loop::{DirectToolResult, RunGate},
    session::SessionKind,
    tool_exec::ToolServices,
    tools::map_mcp_tool_descriptors,
};

#[derive(Clone)]
struct FixtureState {
    streams: Arc<Vec<String>>,
    stream_index: Arc<AtomicUsize>,
    catalog_index: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
}

async fn catalog(State(state): State<FixtureState>) -> impl IntoResponse {
    let model = if state.catalog_index.fetch_add(1, Ordering::SeqCst) == 0 {
        json!({
            "displayName":"Fixture",
            "supportsImages":true,
            "supportsThinking":true,
            "thinkingBudget":64,
            "maxOutputTokens":16384
        })
    } else {
        json!({"displayName":"Drifted","supportsThinking":false,"maxOutputTokens":1})
    };
    axum::Json(json!({
        "models": {
            "fixture-model": model
        }
    }))
}

async fn stream(State(state): State<FixtureState>, body: Bytes) -> Response {
    let value = serde_json::from_slice(&body).unwrap();
    state.requests.lock().push(value);
    let index = state.stream_index.fetch_add(1, Ordering::SeqCst);
    let Some(payload) = state.streams.get(index) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        payload.clone(),
    )
        .into_response()
}

async fn fixture(streams: Vec<String>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let state = FixtureState {
        streams: Arc::new(streams),
        stream_index: Arc::new(AtomicUsize::new(0)),
        catalog_index: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let requests = state.requests.clone();
    let app = Router::new()
        .route("/v1internal:fetchAvailableModels", post(catalog))
        .route("/v1internal:streamGenerateContent", post(stream))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), requests)
}

fn adapter(endpoint: String) -> AntigravityAdapter {
    let credential = AntigravityCredential {
        access_token: "fixture-token".into(),
        refresh_token: "fixture-refresh".into(),
        expires_at: u64::MAX,
        granted_scopes: Vec::new(),
        project_id: "fixture-project".into(),
    };
    let client = reqwest::Client::builder().build().unwrap();
    AntigravityAdapter::with_transport(
        "fixture-model".into(),
        AntigravityAuthHandle::fixed(credential),
        client,
        endpoint,
    )
    .unwrap()
}

fn request(
    history: Vec<ConversationItem>,
    prior_tool_results: Vec<DirectToolResult>,
) -> AdapterStepRequest {
    let (_cancel, cancellation) = tokio::sync::watch::channel(7);
    AdapterStepRequest {
        identity: crate::provider_runtime::RunIdentity {
            session_id: "session-fixture".into(),
            run_id: RunId::new(11),
            request_id: "request-fixture".into(),
            session_kind: SessionKind::Eps,
            cancellation_generation: 7,
        },
        binding: BindingSnapshot {
            provider: ProviderId::Antigravity,
            model: "fixture-model".into(),
            reasoning: None,
            base_url: None,
            capabilities: Some(ModelCapabilities {
                vision: true,
                tool_calls: true,
                strict_structured_output: true,
                reasoning_levels: Vec::new(),
                native_compaction: false,
                context_window: Some(128_000),
                hosted_web_search: false,
            }),
            conversation: ProviderConversationState::Antigravity {
                transcript_revision: 0,
            },
        },
        kind: AdapterRequestKind::Foreground(AgentTurnInput::text("question")),
        policy: RunPolicy {
            active_deadline: Some(Duration::from_secs(60)),
            shutdown_grace: Duration::from_secs(2),
            max_output_bytes: 1024 * 1024,
            max_output_tokens: Some(8_192),
            max_tool_rounds: 64,
            allow_resume: true,
        },
        continuation: None,
        history: history.into(),
        prior_tool_results: prior_tool_results.into(),
        tool_descriptors: Arc::new([]),
        native_mcp_endpoint: None,
        cancellation,
    }
}

fn structured_request() -> AdapterStepRequest {
    let mut request = request(Vec::new(), Vec::new());
    request.kind = AdapterRequestKind::Structured {
        kind: StructuredJobKind::TaskStateCompiler,
        prompt: "compile input".into(),
        workspace_root: PathBuf::from("fixture"),
        output_schema: json!({
            "type":"object",
            "properties":{"ok":{"type":"boolean"}},
            "required":["ok"]
        }),
    };
    request
}

fn user_history() -> Vec<ConversationItem> {
    vec![ConversationItem::User {
        request_id: "request-fixture".into(),
        text: "question".into(),
        images: Vec::new(),
    }]
}

fn sse(value: Value) -> String {
    format!("data: {value}\n\n")
}

fn direct_operation_names(schema: &Value) -> BTreeSet<String> {
    let operation = &schema["properties"]["op"];
    operation
        .get("enum")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .chain(operation.get("const").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn operation_contracts(schema: &Value) -> Vec<(BTreeSet<String>, BTreeSet<String>)> {
    let names = direct_operation_names(schema);
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let base = (names, required);
    if let Some(variants) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    {
        return variants
            .iter()
            .flat_map(operation_contracts)
            .map(|branch| merge_operation_contract(&base, &branch))
            .collect();
    }
    let Some(members) = schema.get("allOf").and_then(Value::as_array) else {
        return vec![base];
    };
    members.iter().fold(vec![base], |contracts, member| {
        let member_contracts = operation_contracts(member);
        let mut merged = Vec::with_capacity(contracts.len() * member_contracts.len());
        for contract in &contracts {
            for member in &member_contracts {
                merged.push(merge_operation_contract(contract, member));
            }
        }
        merged
    })
}

fn merge_operation_contract(
    left: &(BTreeSet<String>, BTreeSet<String>),
    right: &(BTreeSet<String>, BTreeSet<String>),
) -> (BTreeSet<String>, BTreeSet<String>) {
    (
        left.0.union(&right.0).cloned().collect(),
        left.1.union(&right.1).cloned().collect(),
    )
}

fn operation_names(schema: &Value) -> BTreeSet<String> {
    operation_contracts(schema)
        .into_iter()
        .flat_map(|(names, _)| names)
        .collect()
}

fn operation_requirements(schema: &Value) -> BTreeMap<String, BTreeSet<String>> {
    let mut requirements = BTreeMap::new();
    for (names, required) in operation_contracts(schema) {
        for name in names {
            assert!(requirements.insert(name, required.clone()).is_none());
        }
    }
    requirements
}

#[test]
fn preserves_cloud_code_schema_unions_and_local_references() {
    // Given: two operation variants whose state contracts are local definitions.
    let normalized = normalize_cca_parameters(&json!({
        "type":"object",
        "additionalProperties":false,
        "definitions":{
            "unitState":{"type":"object","properties":{"unitId":{"type":"integer"}},"required":["unitId"]},
            "spriteState":{"type":"object","properties":{"spriteId":{"type":"integer"}},"required":["spriteId"]}
        },
        "properties":{"operation":{"oneOf":[
            {"type":"object","properties":{"op":{"const":"unit.add"},"state":{"$ref":"#/definitions/unitState"}},"required":["op","state"]},
            {"type":"object","properties":{"op":{"const":"sprite.add"},"state":{"$ref":"#/definitions/spriteState"}},"required":["op","state"]}
        ]}}
    }));

    // When: the common JSON schema is translated to Cloud Code's Schema wire shape.
    let variants = normalized["properties"]["operation"]["anyOf"]
        .as_array()
        .expect("operation variants must remain distinct");

    // Then: every discriminator, state reference, and referenced field remains available.
    assert_eq!(variants.len(), 2);
    assert_eq!(variants[0]["properties"]["op"]["enum"], json!(["unit.add"]));
    assert_eq!(
        variants[0]["properties"]["state"]["ref"],
        "#/defs/unitState"
    );
    assert_eq!(
        variants[1]["properties"]["op"]["enum"],
        json!(["sprite.add"])
    );
    assert_eq!(
        variants[1]["properties"]["state"]["ref"],
        "#/defs/spriteState"
    );
    assert_eq!(
        normalized["defs"]["unitState"]["properties"]["unitId"]["type"],
        "integer"
    );
    assert_eq!(
        normalized["defs"]["spriteState"]["properties"]["spriteId"]["type"],
        "integer"
    );
    assert!(normalized.get("additionalProperties").is_none());
}

#[tokio::test]
async fn advertises_every_map_operation_and_accepts_a_nonfirst_call_over_the_real_adapter() {
    // Given: the production Map descriptor and a Cloud Code peer returning doodad.add.
    let call_arguments = json!({
        "operations":[{
            "op":"doodad.add",
            "state":{"doodadId":7,"x":12,"y":34,"owner":11,"disabled":false}
        }]
    });
    let call = sse(json!({"response":{"candidates":[{"content":{"parts":[{
        "functionCall":{"id":"map-call","name":"map_draft_patch","args":call_arguments}
    }]},"finishReason":"STOP"}]}}));
    let (endpoint, requests) = fixture(vec![call]).await;
    let mut adapter = adapter(endpoint);
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(8);
    let descriptor = map_mcp_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor["name"] == "map_draft_patch")
        .expect("map_draft_patch must be advertised");
    let source_operations = &descriptor["inputSchema"]["properties"]["operations"]["items"];
    let expected_operation_names = operation_names(source_operations);
    let expected_operation_requirements = operation_requirements(source_operations);
    let mut step = request(user_history(), Vec::new());
    step.tool_descriptors = Arc::from([descriptor]);

    // When: the production Antigravity adapter sends and decodes one full HTTP step.
    let outcome = adapter.run_step(step, events_tx).await.unwrap();

    // Then: all operation arms reach the wire and the non-first operation survives decoding.
    let sent = requests.lock();
    let parameters = &sent[0]["request"]["tools"][0]["functionDeclarations"][0]["parameters"];
    let variants = parameters["properties"]["operations"]["items"]["anyOf"]
        .as_array()
        .expect("Map operation alternatives must reach Cloud Code");
    assert_eq!(
        variants
            .iter()
            .flat_map(operation_names)
            .collect::<BTreeSet<_>>(),
        expected_operation_names
    );
    assert_eq!(
        operation_requirements(&parameters["properties"]["operations"]["items"]),
        expected_operation_requirements
    );
    let doodad_add = variants
        .iter()
        .find(|variant| {
            variant["properties"]["op"]["enum"]
                .as_array()
                .is_some_and(|names| names.contains(&json!("doodad.add")))
        })
        .expect("doodad.add must reach Cloud Code");
    let state = &doodad_add["properties"]["state"];
    let state = state
        .get("ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("#/defs/"))
        .map_or(state, |name| &parameters["defs"][name]);
    assert_eq!(state["properties"]["doodadId"]["type"], "integer");
    assert_eq!(state["properties"]["disabled"]["type"], "boolean");
    let AdapterStepOutcome::NeedsTools { calls, .. } = outcome else {
        panic!("expected Map tool call")
    };
    assert_eq!(calls[0].name, "map_draft_patch");
    assert_eq!(calls[0].arguments, call_arguments);
}

#[tokio::test]
async fn preserves_the_production_palette_query_union_and_local_gate_contract() {
    // Given: the production palette descriptor and a valid query returned by Cloud Code.
    let call_arguments = json!({"kind":"tiles","query":"floor"});
    let call = sse(json!({"response":{"candidates":[{"content":{"parts":[{
        "functionCall":{"id":"palette-call","name":"map_palette_query","args":call_arguments}
    }]},"finishReason":"STOP"}]}}));
    let (endpoint, requests) = fixture(vec![call]).await;
    let mut adapter = adapter(endpoint);
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(8);
    let descriptor = map_mcp_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor["name"] == "map_palette_query")
        .expect("map_palette_query must be advertised");
    let mut step = request(user_history(), Vec::new());
    step.tool_descriptors = Arc::from([descriptor]);

    // When: the production adapter sends and decodes one complete HTTP step.
    let outcome = adapter.run_step(step, events_tx).await.unwrap();

    // Then: both root object alternatives and their complete field contracts reach the wire.
    {
        let sent = requests.lock();
        let parameters = &sent[0]["request"]["tools"][0]["functionDeclarations"][0]["parameters"];
        assert_eq!(parameters["type"], "object");
        let variants = parameters["anyOf"]
            .as_array()
            .expect("palette query alternatives must reach Cloud Code");
        assert_eq!(variants.len(), 2);
        let query = variants
            .iter()
            .find(|variant| {
                variant["required"]
                    .as_array()
                    .is_some_and(|required| required.contains(&json!("query")))
            })
            .expect("query alternative must reach Cloud Code");
        let filter = variants
            .iter()
            .find(|variant| {
                variant["required"]
                    .as_array()
                    .is_some_and(|required| required.contains(&json!("filter")))
            })
            .expect("filter alternative must reach Cloud Code");
        assert_eq!(query["properties"]["kind"]["type"], "string");
        assert_eq!(query["properties"]["query"]["type"], "string");
        assert_eq!(filter["properties"]["filter"]["type"], "object");
        assert_eq!(
            filter["properties"]["filter"]["properties"]["id"]["type"],
            "integer"
        );
        assert!(filter["properties"]["filter"]["properties"]
            .get("tileId")
            .is_none());
    }
    let AdapterStepOutcome::NeedsTools { calls, .. } = outcome else {
        panic!("expected palette query tool call")
    };
    assert_eq!(calls[0].name, "map_palette_query");
    assert_eq!(calls[0].arguments, call_arguments);

    // And: the common RunGate rejects the captured invalid legacy field before execution,
    // returning a model-correctable usage error instead of a fatal admission failure.
    let runtime = ToolServices::for_tests().map_session("palette-gate-session");
    let (_cancel, cancellation) = tokio::sync::watch::channel(9_u64);
    runtime.set_cancellation(cancellation);
    runtime
        .begin_request("palette-gate-request", "fixture-map")
        .unwrap();
    let gate = RunGate::new(
        crate::provider_runtime::RunIdentity {
            session_id: runtime.session_id().to_string(),
            run_id: RunId::new(12),
            request_id: "palette-gate-request".to_string(),
            session_kind: SessionKind::Map,
            cancellation_generation: 9,
        },
        runtime,
        crate::provider_runtime::WorkspaceAccess::Read,
        None,
    );
    let result = gate
        .dispatch_native(
            Some("invalid-palette-call".to_string()),
            "map_palette_query".to_string(),
            json!({"kind":"tiles","filter":{"tileId":0}}),
        )
        .await
        .expect("schema violation is recoverable, not a dispatch failure");
    assert!(result.is_error);
    assert!(result.result.as_str().is_some_and(
        |message| message.contains("arguments do not match the documented input schema")
    ));
    assert!(gate.fatal_admission_error().is_none());
    assert_eq!(gate.completed().len(), 1);
}

#[tokio::test]
async fn flattens_nested_all_of_without_losing_inherited_fields_or_local_references() {
    // Given: a legal common schema with inherited operation fields and a local state reference.
    let descriptor = json!({
        "name":"nested_patch",
        "description":"Nested operation fixture.",
        "inputSchema":{
            "type":"object",
            "$defs":{
                "d":{
                    "type":"object",
                    "properties":{"doodadId":{"type":"integer"}},
                    "required":["doodadId"]
                }
            },
            "properties":{"operations":{"type":"array","items":{"allOf":[
                {
                    "type":"object",
                    "properties":{"op":{"type":"string"}},
                    "required":["op"]
                },
                {"oneOf":[
                    {
                        "type":"object",
                        "properties":{
                            "op":{"const":"doodad.add"},
                            "state":{"$ref":"#/$defs/d"}
                        },
                        "required":["state"]
                    },
                    {"allOf":[
                        {
                            "type":"object",
                            "properties":{
                                "ordinal":{"type":"integer"},
                                "beforeFingerprint":{"type":"string"}
                            },
                            "required":["ordinal","beforeFingerprint"]
                        },
                        {"oneOf":[
                            {
                                "type":"object",
                                "properties":{
                                    "op":{"const":"unit.set"},
                                    "state":{"type":"object","properties":{"owner":{"type":"integer"}}}
                                },
                                "required":["state"]
                            },
                            {
                                "type":"object",
                                "properties":{
                                    "op":{"const":"unit.move"},
                                    "x":{"type":"integer"},
                                    "y":{"type":"integer"}
                                },
                                "required":["x","y"]
                            }
                        ]}
                    ]}
                ]}
            ]}}},
            "required":["operations"]
        }
    });
    let call_arguments = json!({
        "operations":[{
            "op":"unit.move",
            "ordinal":7,
            "beforeFingerprint":"before",
            "x":12,
            "y":34
        }]
    });
    let call = sse(json!({"response":{"candidates":[{"content":{"parts":[{
        "functionCall":{"id":"nested-call","name":"nested_patch","args":call_arguments}
    }]},"finishReason":"STOP"}]}}));
    let (endpoint, requests) = fixture(vec![call]).await;
    let mut adapter = adapter(endpoint);
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(8);
    let mut step = request(user_history(), Vec::new());
    step.tool_descriptors = Arc::from([descriptor]);

    // When: the production adapter sends and decodes one complete HTTP step.
    let outcome = adapter.run_step(step, events_tx).await.unwrap();

    // Then: every flattened branch retains inherited types, required names, and references.
    let sent = requests.lock();
    let parameters = &sent[0]["request"]["tools"][0]["functionDeclarations"][0]["parameters"];
    let variants = parameters["properties"]["operations"]["items"]["anyOf"]
        .as_array()
        .expect("nested operation alternatives must be flattened");
    assert_eq!(
        operation_names(&parameters["properties"]["operations"]["items"]),
        BTreeSet::from([
            "doodad.add".to_string(),
            "unit.move".to_string(),
            "unit.set".to_string()
        ])
    );
    let unit_move = variants
        .iter()
        .find(|variant| operation_names(variant).contains("unit.move"))
        .expect("unit.move must remain represented");
    assert_eq!(unit_move["properties"]["op"]["type"], "string");
    assert_eq!(unit_move["properties"]["op"]["enum"], json!(["unit.move"]));
    assert_eq!(unit_move["properties"]["ordinal"]["type"], "integer");
    assert_eq!(
        unit_move["properties"]["beforeFingerprint"]["type"],
        "string"
    );
    assert_eq!(unit_move["properties"]["x"]["type"], "integer");
    assert_eq!(unit_move["properties"]["y"]["type"], "integer");
    assert_eq!(
        unit_move["required"]
            .as_array()
            .expect("unit.move required names")
            .iter()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["beforeFingerprint", "op", "ordinal", "x", "y"])
    );
    let doodad_add = variants
        .iter()
        .find(|variant| operation_names(variant).contains("doodad.add"))
        .expect("doodad.add must remain represented");
    assert_eq!(doodad_add["properties"]["state"]["ref"], "#/defs/d");
    assert_eq!(
        parameters["defs"]["d"]["properties"]["doodadId"]["type"],
        "integer"
    );
    let AdapterStepOutcome::NeedsTools { calls, .. } = outcome else {
        panic!("expected nested tool call")
    };
    assert_eq!(calls[0].name, "nested_patch");
    assert_eq!(calls[0].arguments, call_arguments);
}
