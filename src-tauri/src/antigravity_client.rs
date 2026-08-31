//! Direct Cloud Code Assist Antigravity catalog, streaming, transcript, and tool loop.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::antigravity_auth::{
    access_credential, force_refresh, AntigravityCredential, ANTIGRAVITY_USER_AGENT,
    CLOUD_CODE_ENDPOINT,
};
use crate::codex_client::{AgentTurnInput, WorkspaceAccess};
use crate::engine::{
    AgentDriver, AgentEngineError, AgentTurnResult, EngineEvent, EventSink, SessionEventSink,
};
use crate::opencode_go::{transcript_image, SseDecoder};
use crate::provider::{
    ModelCapabilities, ProviderConversationState, ProviderId, ProviderModel, ReasoningSelection,
};
use crate::provider_tool_loop::{
    validate_structured_output, DirectToolCall, DirectToolResult, NormalizedAssistantStep,
    ProviderToolDispatcher,
};
use crate::provider_transcript::{ProviderTranscriptStore, TranscriptEntry};
use crate::tool_exec::SessionToolRuntime;
use crate::workspace::{PreparedWorkspace, WorkspaceManager, WorkspaceTurnRecorder};

const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOOL_ROUNDS: usize = 64;
const STRUCTURED_TOOL: &str = "submit_structured_result";

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveAntigravityModel {
    id: String,
    display_name: String,
    supports_images: bool,
    supports_thinking: bool,
    thinking_budget: Option<u64>,
    context_window: Option<u64>,
    max_output_tokens: Option<u64>,
    api_provider: Option<String>,
    model_provider: Option<String>,
}

impl LiveAntigravityModel {
    fn provider_model(&self, selected: Option<&str>) -> ProviderModel {
        let description = match (&self.model_provider, &self.api_provider) {
            (Some(model_provider), Some(api_provider)) => {
                format!("{model_provider} · {api_provider}")
            }
            (Some(provider), None) | (None, Some(provider)) => provider.clone(),
            (None, None) => "Antigravity live catalog".to_string(),
        };
        ProviderModel {
            provider: ProviderId::Antigravity,
            model: self.id.clone(),
            display_name: self.display_name.clone(),
            description,
            is_default: selected == Some(self.id.as_str()),
            capabilities: ModelCapabilities {
                vision: self.supports_images,
                tool_calls: true,
                strict_structured_output: true,
                reasoning_levels: Vec::new(),
                native_compaction: false,
                context_window: self.context_window,
                hosted_web_search: false,
            },
            privacy: None,
        }
    }
}

fn parse_live_model(id: &str, value: &Value) -> Option<LiveAntigravityModel> {
    if id.is_empty()
        || id.len() > 256
        || id.chars().any(char::is_control)
        || value.get("isInternal") == Some(&Value::Bool(true))
    {
        return None;
    }
    let display_name =
        bounded_catalog_string(value.get("displayName"), 256).unwrap_or_else(|| id.to_string());
    Some(LiveAntigravityModel {
        id: id.to_string(),
        display_name,
        supports_images: value
            .get("supportsImages")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        supports_thinking: value
            .get("supportsThinking")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        thinking_budget: positive_catalog_u64(value.get("thinkingBudget")),
        context_window: positive_catalog_u64(value.get("maxTokens")),
        max_output_tokens: positive_catalog_u64(value.get("maxOutputTokens")),
        api_provider: bounded_catalog_string(value.get("apiProvider"), 128),
        model_provider: bounded_catalog_string(value.get("modelProvider"), 128),
    })
}

fn bounded_catalog_string(value: Option<&Value>, max_len: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= max_len)
        .map(str::to_string)
}

fn positive_catalog_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64).filter(|value| *value > 0)
}

fn parse_live_catalog(value: &Value) -> Result<Vec<LiveAntigravityModel>, String> {
    let models = value
        .get("models")
        .and_then(Value::as_object)
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    let mut ordered_ids = Vec::with_capacity(models.len());
    let mut seen = HashSet::with_capacity(models.len());
    if let Some(sorts) = value.get("agentModelSorts").and_then(Value::as_array) {
        for sort in sorts {
            let Some(groups) = sort.get("groups").and_then(Value::as_array) else {
                continue;
            };
            for group in groups {
                let Some(ids) = group.get("modelIds").and_then(Value::as_array) else {
                    continue;
                };
                for id in ids.iter().filter_map(Value::as_str) {
                    if models.contains_key(id) && seen.insert(id) {
                        ordered_ids.push(id);
                    }
                }
            }
        }
    }
    for id in models.keys() {
        if seen.insert(id) {
            ordered_ids.push(id);
        }
    }
    Ok(ordered_ids
        .into_iter()
        .filter_map(|id| parse_live_model(id, &models[id]))
        .collect())
}

async fn fetch_live_catalog(
    client: &reqwest::Client,
    credential: &AntigravityCredential,
) -> Result<Vec<LiveAntigravityModel>, String> {
    let response = client
        .post(format!(
            "{CLOUD_CODE_ENDPOINT}/v1internal:fetchAvailableModels"
        ))
        .bearer_auth(&credential.access_token)
        .header(reqwest::header::USER_AGENT, ANTIGRAVITY_USER_AGENT)
        .json(&json!({}))
        .send()
        .await
        .map_err(|_| "provider_catalog_unavailable".to_string())?;
    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CATALOG_BYTES as u64)
    {
        return Err("provider_protocol_changed".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "provider_catalog_unavailable".to_string())?;
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err("provider_protocol_changed".to_string());
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "provider_protocol_changed".to_string())?;
    parse_live_catalog(&value)
}

pub async fn fetch_catalog(
    client: &reqwest::Client,
    credential: &AntigravityCredential,
    selected: Option<&str>,
) -> Result<Vec<ProviderModel>, String> {
    Ok(fetch_live_catalog(client, credential)
        .await?
        .into_iter()
        .map(|model| model.provider_model(selected))
        .collect())
}

pub struct ProductionAntigravityDriver {
    session_id: String,
    model: String,
    dirs: crate::config::DataDirs,
    client: reqwest::Client,
    transcripts: ProviderTranscriptStore,
    dispatcher: ProviderToolDispatcher,
    sink: SessionEventSink,
    runtime: SessionToolRuntime,
    workspace: WorkspaceManager,
    active_workspace: Option<PreparedWorkspace>,
    workspace_override: Option<PreparedWorkspace>,
    revision: u64,
    cancellation: tokio::sync::watch::Receiver<u64>,
    persist_context_usage: bool,
}

impl ProductionAntigravityDriver {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        session_id: String,
        model: String,
        _reasoning: Option<ReasoningSelection>,
        dirs: crate::config::DataDirs,
        runtime: SessionToolRuntime,
        sink: SessionEventSink,
        revision: u64,
        cancellation: tokio::sync::watch::Receiver<u64>,
    ) -> Result<Self, AgentEngineError> {
        if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
            return Err(AgentEngineError::new("provider_model_unavailable"));
        }
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(600))
            .user_agent(ANTIGRAVITY_USER_AGENT)
            .build()
            .map_err(|_| AgentEngineError::new("provider_transport_closed"))?;
        Ok(Self {
            session_id,
            model,
            dirs: dirs.clone(),
            client,
            transcripts: ProviderTranscriptStore::new(&dirs),
            dispatcher: ProviderToolDispatcher::new(runtime.clone()),
            sink,
            runtime,
            workspace: WorkspaceManager::new(dirs),
            active_workspace: None,
            workspace_override: None,
            revision,
            cancellation,
            persist_context_usage: true,
        })
    }

    pub(crate) fn use_workspace(&mut self, workspace: PreparedWorkspace) {
        self.workspace_override = Some(workspace);
    }

    pub(crate) fn disable_session_persistence(&mut self) {
        self.persist_context_usage = false;
    }

    async fn run_direct_turn(
        &mut self,
        mut input: AgentTurnInput,
    ) -> Result<AgentTurnResult, AgentEngineError> {
        let credential = access_credential(&self.dirs)
            .await
            .map_err(AgentEngineError::new)?;
        let catalog = fetch_live_catalog(&self.client, &credential)
            .await
            .map_err(AgentEngineError::new)?;
        let live_model = catalog
            .into_iter()
            .find(|model| model.id == self.model)
            .ok_or_else(|| AgentEngineError::new("provider_model_unavailable"))?;
        if !input.image_paths.is_empty() && !live_model.supports_images {
            return Err(AgentEngineError::new("provider_capability_unsupported"));
        }
        self.sink
            .emit(EngineEvent::Progress(crate::ipc::ProgressEvent {
                stage: crate::ipc::ProgressStage::Provider,
                detail: Some("Antigravity turn started".to_string()),
                provider: Some(ProviderId::Antigravity),
                model: Some(self.model.clone()),
            }))?;

        let request_id = self.runtime.current_request_id().ok_or_else(|| {
            AgentEngineError::new("no request is open for the provider workspace")
        })?;
        if input.workspace_access == WorkspaceAccess::Write
            && !self.runtime.owns_write_registration()
        {
            return Err(AgentEngineError::new(
                "write-mode provider execution requires an active workspace write registration",
            ));
        }
        let manager = self.workspace.clone();
        let workspace_override = self.workspace_override.clone();
        let session_id = self.session_id.clone();
        let baseline_request = request_id.clone();
        let access = input.workspace_access;
        let (workspace, baseline) = tokio::task::spawn_blocking(move || {
            let workspace = match workspace_override {
                Some(workspace) => workspace,
                None => manager.prepare_session_current(&session_id)?,
            };
            let baseline = if access == WorkspaceAccess::Write {
                Some(
                    manager
                        .begin_turn(&workspace, &baseline_request)
                        .map_err(|error| error.to_string())?,
                )
            } else {
                None
            };
            Ok::<_, String>((workspace, baseline))
        })
        .await
        .map_err(|error| AgentEngineError::new(error.to_string()))?
        .map_err(AgentEngineError::new)?;
        self.runtime
            .bind_workspace_root(&request_id, workspace.root.clone())
            .map_err(AgentEngineError::new)?;
        self.active_workspace = Some(workspace.clone());
        let mut recorder = baseline.map(|baseline| {
            WorkspaceTurnRecorder::new(
                self.workspace.clone(),
                baseline,
                self.runtime.journal().clone(),
            )
        });

        let cancellation_generation = *self.cancellation.borrow_and_update();
        let mut entries = if self.revision == 0 {
            Vec::new()
        } else {
            let generation = self
                .transcripts
                .load_current(ProviderId::Antigravity, &self.session_id)
                .map_err(AgentEngineError::new)?;
            if generation.revision != self.revision {
                return Err(AgentEngineError::new(
                    "provider transcript revision mismatch",
                ));
            }
            generation.entries
        };
        let images = input
            .image_paths
            .drain(..)
            .map(|path| transcript_image(&path))
            .collect::<Result<Vec<_>, _>>()
            .map_err(AgentEngineError::new)?;
        entries.push(TranscriptEntry::User {
            text: input.text,
            images,
        });
        let structured = input.forbid_tools && input.output_schema.is_some();
        let schema = input.output_schema.clone();
        let mut seen_tool_ids = HashSet::new();
        let mut answer = String::new();
        let mut credential = credential;

        for round in 0..MAX_TOOL_ROUNDS {
            if *self.cancellation.borrow() != cancellation_generation {
                return Ok(AgentTurnResult::Cancelled);
            }
            let tools = if structured {
                vec![structured_declaration(schema.as_ref().expect("checked"))]
            } else if input.forbid_tools {
                Vec::new()
            } else {
                function_declarations(&self.dispatcher.descriptors())
            };
            let body = request_body(
                &self.session_id,
                self.revision,
                round,
                &credential.project_id,
                &live_model,
                &entries,
                tools,
                structured,
            );
            let mut response = self
                .client
                .post(format!(
                    "{CLOUD_CODE_ENDPOINT}/v1internal:streamGenerateContent?alt=sse"
                ))
                .bearer_auth(&credential.access_token)
                .json(&body)
                .send()
                .await
                .map_err(|_| AgentEngineError::new("provider_transport_closed"))?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                credential = force_refresh(&self.dirs)
                    .await
                    .map_err(AgentEngineError::new)?;
                response = self
                    .client
                    .post(format!(
                        "{CLOUD_CODE_ENDPOINT}/v1internal:streamGenerateContent?alt=sse"
                    ))
                    .bearer_auth(&credential.access_token)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|_| AgentEngineError::new("provider_transport_closed"))?;
            }
            if !response.status().is_success() {
                return Err(AgentEngineError::new(status_error(response.status())));
            }
            let AntigravityAssistantStep {
                step,
                thought_signatures,
            } = parse_stream(
                response,
                &mut self.cancellation,
                cancellation_generation,
                round,
            )
            .await
            .map_err(AgentEngineError::new)?;
            self.emit_step(&step)?;
            if !step.reasoning.is_empty() {
                entries.push(TranscriptEntry::AssistantReasoning {
                    text: step.reasoning.clone(),
                });
            }
            if !step.text.is_empty() {
                answer.push_str(&step.text);
                entries.push(TranscriptEntry::AssistantText {
                    text: step.text.clone(),
                });
            }
            if let Some(usage) = step.usage {
                self.emit_usage(usage)?;
            }
            let mut tool_calls = step.tool_calls;
            if structured {
                if tool_calls.len() != 1 {
                    return Err(AgentEngineError::new("provider_structured_output_invalid"));
                }
                let call = tool_calls
                    .pop()
                    .filter(|call| call.name == STRUCTURED_TOOL)
                    .ok_or_else(|| AgentEngineError::new("provider_structured_output_invalid"))?;
                validate_structured_output(schema.as_ref().expect("checked"), &call.arguments)
                    .map_err(AgentEngineError::new)?;
                let text = serde_json::to_string(&call.arguments)
                    .map_err(|_| AgentEngineError::new("provider_structured_output_invalid"))?;
                let thought_signature = thought_signatures.get(&call.id).cloned();
                entries.push(TranscriptEntry::ToolCall {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                    thought_signature,
                });
                self.publish(entries)?;
                if let Some(recorder) = recorder.as_mut() {
                    recorder
                        .finish()
                        .map_err(|error| AgentEngineError::new(error.to_string()))?;
                }
                return Ok(AgentTurnResult::Answer { text });
            }
            if tool_calls.is_empty() {
                self.publish(entries)?;
                if let Some(recorder) = recorder.as_mut() {
                    recorder
                        .finish()
                        .map_err(|error| AgentEngineError::new(error.to_string()))?;
                }
                return Ok(AgentTurnResult::Answer { text: answer });
            }
            for call in &tool_calls {
                entries.push(TranscriptEntry::ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    thought_signature: thought_signatures.get(&call.id).cloned(),
                });
            }
            let batch = self
                .dispatcher
                .dispatch_batch(tool_calls, &mut seen_tool_ids, input.forbid_tools)
                .await
                .map_err(AgentEngineError::new)?;
            self.emit_tool_results(&batch.results)?;
            for result in batch.results {
                entries.push(TranscriptEntry::ToolResult {
                    id: result.id,
                    name: result.name,
                    result: result.result,
                    is_error: result.is_error,
                });
            }
            if batch.stop_for_write_transition {
                self.publish(entries)?;
                return Ok(AgentTurnResult::Answer { text: answer });
            }
        }
        Err(AgentEngineError::new(
            "provider tool loop exceeded its round limit",
        ))
    }

    fn publish(&mut self, entries: Vec<TranscriptEntry>) -> Result<(), AgentEngineError> {
        if !self.persist_context_usage {
            return Ok(());
        }
        let generation = self
            .transcripts
            .publish(
                ProviderId::Antigravity,
                &self.session_id,
                self.revision,
                entries,
            )
            .map_err(AgentEngineError::new)?;
        self.revision = generation.revision;
        Ok(())
    }

    fn emit_step(&self, step: &NormalizedAssistantStep) -> Result<(), AgentEngineError> {
        if !step.reasoning.is_empty() {
            self.sink.emit(EngineEvent::Agent(crate::ipc::AgentEvent {
                kind: "reasoning_delta".to_string(),
                detail: step.reasoning.clone(),
                data: None,
            }))?;
        }
        if !step.text.is_empty() {
            self.sink.emit(EngineEvent::Agent(crate::ipc::AgentEvent {
                kind: "message_delta".to_string(),
                detail: step.text.clone(),
                data: None,
            }))?;
        }
        for call in &step.tool_calls {
            self.sink.emit(EngineEvent::Agent(crate::ipc::AgentEvent {
                kind: "tool_call".to_string(),
                detail: call.name.clone(),
                data: Some(crate::ipc::AgentEventData {
                    args: serde_json::to_string(&call.arguments).ok(),
                    result: None,
                    status: None,
                }),
            }))?;
        }
        Ok(())
    }

    fn emit_tool_results(&self, results: &[DirectToolResult]) -> Result<(), AgentEngineError> {
        for result in results {
            self.sink.emit(EngineEvent::Agent(crate::ipc::AgentEvent {
                kind: "tool_result".to_string(),
                detail: result.name.clone(),
                data: Some(crate::ipc::AgentEventData {
                    args: None,
                    result: serde_json::to_string(&result.result).ok(),
                    status: Some(if result.is_error { "error" } else { "ok" }.to_string()),
                }),
            }))?;
        }
        Ok(())
    }

    fn emit_usage(&self, usage: crate::ipc::ContextUsage) -> Result<(), AgentEngineError> {
        if !self.persist_context_usage {
            return Ok(());
        }
        self.sink
            .emit(EngineEvent::ContextUsage(crate::ipc::ContextUsageEvent {
                turn_id: uuid::Uuid::new_v4().to_string(),
                token_usage: usage,
            }))
    }
}

impl AgentDriver for ProductionAntigravityDriver {
    async fn run_turn(
        &mut self,
        input: AgentTurnInput,
    ) -> Result<AgentTurnResult, AgentEngineError> {
        self.run_direct_turn(input).await
    }

    async fn compile_task_state(
        &mut self,
        input: AgentTurnInput,
    ) -> Result<Option<String>, AgentEngineError> {
        let revision = self.revision;
        let persist = self.persist_context_usage;
        self.revision = 0;
        self.persist_context_usage = false;
        let result = self.run_direct_turn(input).await;
        self.revision = revision;
        self.persist_context_usage = persist;
        match result? {
            AgentTurnResult::Answer { text } => Ok(Some(text)),
            AgentTurnResult::Cancelled => Err(AgentEngineError::new("provider_cancelled")),
            AgentTurnResult::Plan { .. } => {
                Err(AgentEngineError::new("provider_structured_output_invalid"))
            }
        }
    }

    async fn compact_conversation(&mut self) -> Result<(), AgentEngineError> {
        let current = self
            .transcripts
            .load_current(ProviderId::Antigravity, &self.session_id)
            .map_err(AgentEngineError::new)?;
        let result = self
            .run_direct_turn(
                AgentTurnInput::text("Summarize the conversation for exact continuation. Preserve system/developer invariants, accepted plan, current task state, unresolved ASK/review, decisions, evidence, and file identities.")
                    .without_tools(),
            )
            .await?;
        let AgentTurnResult::Answer { text: summary } = result else {
            return Err(AgentEngineError::new("provider_structured_output_invalid"));
        };
        let generation = self
            .transcripts
            .publish(
                ProviderId::Antigravity,
                &self.session_id,
                self.revision,
                vec![TranscriptEntry::Compaction {
                    summary,
                    previous_revision: current.revision,
                }],
            )
            .map_err(AgentEngineError::new)?;
        self.revision = generation.revision;
        Ok(())
    }

    async fn reset_conversation(&mut self) -> Result<(), AgentEngineError> {
        self.transcripts
            .delete_session(&self.session_id)
            .map_err(AgentEngineError::new)?;
        self.revision = 0;
        Ok(())
    }

    async fn conversation_state(&self) -> ProviderConversationState {
        ProviderConversationState::Antigravity {
            transcript_revision: self.revision,
        }
    }

    async fn seed_conversation(
        &mut self,
        state: ProviderConversationState,
    ) -> Result<(), AgentEngineError> {
        let ProviderConversationState::Antigravity {
            transcript_revision,
        } = state
        else {
            return Err(AgentEngineError::new(
                "Antigravity driver received incompatible conversation state",
            ));
        };
        if transcript_revision > 0 {
            let current = self
                .transcripts
                .load_current(ProviderId::Antigravity, &self.session_id)
                .map_err(AgentEngineError::new)?;
            if current.revision != transcript_revision {
                return Err(AgentEngineError::new(
                    "provider transcript revision mismatch",
                ));
            }
        }
        self.revision = transcript_revision;
        Ok(())
    }

    fn current_workspace(&self) -> Option<PreparedWorkspace> {
        self.active_workspace.clone()
    }
}

fn structured_declaration(schema: &Value) -> Value {
    json!({
        "name": STRUCTURED_TOOL,
        "description": "Submit the final structured result.",
        "parameters": normalize_cca_parameters(schema)
    })
}

fn function_declarations(descriptors: &[Value]) -> Vec<Value> {
    descriptors
        .iter()
        .filter_map(|descriptor| {
            let parameters = descriptor
                .get("inputSchema")
                .map(normalize_cca_parameters)
                .unwrap_or_else(|| json!({"type":"object","properties":{}}));
            Some(json!({
                "name": descriptor.get("name")?.as_str()?,
                "description": descriptor.get("description").and_then(Value::as_str).unwrap_or(""),
                "parameters": parameters
            }))
        })
        .collect()
}

fn normalize_cca_parameters(schema: &Value) -> Value {
    let mut normalized = normalize_cca_schema(schema);
    let Some(object) = normalized.as_object_mut() else {
        return json!({"type":"object","properties":{}});
    };
    if object.get("type").and_then(Value::as_str) != Some("object") {
        return json!({"type":"object","properties":{}});
    }
    object
        .entry("properties")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    normalized
}

fn normalize_cca_schema(schema: &Value) -> Value {
    let Some(source) = schema.as_object() else {
        return Value::Object(serde_json::Map::new());
    };
    let mut normalized = serde_json::Map::new();

    if let Some(schema_type) = normalized_cca_type(source.get("type")) {
        normalized.insert("type".to_string(), Value::String(schema_type.to_string()));
    }
    if let Some(description) = source.get("description").and_then(Value::as_str) {
        normalized.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
    }
    if let Some(values) = source.get("enum").and_then(Value::as_array) {
        let values = values
            .iter()
            .filter(|value| value.is_string())
            .cloned()
            .collect::<Vec<_>>();
        if !values.is_empty() {
            normalized.insert("enum".to_string(), Value::Array(values));
        }
    } else if let Some(value) = source.get("const").filter(|value| value.is_string()) {
        normalized.insert("enum".to_string(), Value::Array(vec![value.clone()]));
    }
    if let Some(properties) = source.get("properties").and_then(Value::as_object) {
        normalized.insert(
            "properties".to_string(),
            Value::Object(
                properties
                    .iter()
                    .map(|(name, property)| (name.clone(), normalize_cca_schema(property)))
                    .collect(),
            ),
        );
    }
    if let Some(required) = source.get("required").and_then(Value::as_array) {
        normalized.insert(
            "required".to_string(),
            Value::Array(
                required
                    .iter()
                    .filter(|name| name.is_string())
                    .cloned()
                    .collect(),
            ),
        );
    }
    if let Some(items) = source.get("items") {
        normalized.insert("items".to_string(), normalize_cca_schema(items));
    }

    for combiner in ["oneOf", "anyOf", "allOf"] {
        if let Some(variants) = source.get(combiner).and_then(Value::as_array) {
            merge_cca_variants(&mut normalized, variants);
        }
    }

    if normalized.get("type").and_then(Value::as_str) == Some("object") {
        normalized
            .entry("properties")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(Value::Array(mut required)) = normalized.remove("required") {
            let properties = normalized
                .get("properties")
                .and_then(Value::as_object)
                .expect("object schemas always have properties");
            required.retain(|name| {
                name.as_str()
                    .is_some_and(|name| properties.contains_key(name))
            });
            required.dedup();
            if !required.is_empty() {
                normalized.insert("required".to_string(), Value::Array(required));
            }
        }
    }

    Value::Object(normalized)
}

fn normalized_cca_type(value: Option<&Value>) -> Option<&str> {
    let supported = |value| {
        matches!(
            value,
            "object" | "array" | "string" | "number" | "integer" | "boolean"
        )
    };
    match value {
        Some(Value::String(value)) if supported(value) => Some(value),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .find(|value| *value != "null" && supported(value)),
        _ => None,
    }
}

fn merge_cca_variants(target: &mut serde_json::Map<String, Value>, variants: &[Value]) {
    let variants = variants
        .iter()
        .map(normalize_cca_schema)
        .collect::<Vec<_>>();
    if target.get("type").is_none() {
        let mut types = variants
            .iter()
            .filter_map(|variant| variant.get("type").and_then(Value::as_str));
        if let Some(first) = types.next() {
            if types.all(|candidate| candidate == first) {
                target.insert("type".to_string(), Value::String(first.to_string()));
            }
        }
    }

    let target_properties = target
        .entry("properties")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(target_properties) = target_properties.as_object_mut() else {
        return;
    };
    for variant in &variants {
        if let Some(properties) = variant.get("properties").and_then(Value::as_object) {
            for (name, property) in properties {
                target_properties
                    .entry(name.clone())
                    .or_insert_with(|| property.clone());
            }
        }
    }

    let Some(first_required) = variants
        .first()
        .and_then(|variant| variant.get("required"))
        .and_then(Value::as_array)
    else {
        return;
    };
    let shared_required = first_required
        .iter()
        .filter(|name| {
            variants.iter().skip(1).all(|variant| {
                variant
                    .get("required")
                    .and_then(Value::as_array)
                    .is_some_and(|required| required.contains(name))
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    if shared_required.is_empty() {
        return;
    }
    let required = target
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("required is always an array");
    for name in shared_required {
        if !required.contains(&name) {
            required.push(name);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn request_body(
    session_id: &str,
    revision: u64,
    round: usize,
    project_id: &str,
    model: &LiveAntigravityModel,
    entries: &[TranscriptEntry],
    tools: Vec<Value>,
    structured: bool,
) -> Value {
    let step = revision.saturating_add(round as u64).saturating_add(2);
    let trajectory = stable_uuid(&format!("trajectory:{session_id}"));
    let agent = stable_uuid(&format!("agent:{session_id}"));
    let mut request = json!({
        "contents": gemini_history(entries),
        "sessionId": signed_session_id(session_id),
        "labels": {
            "last_step_index": step.saturating_sub(1).to_string(),
            "trajectory_id": trajectory
        }
    });
    if !tools.is_empty() {
        request["tools"] = json!([{"functionDeclarations": tools}]);
        request["toolConfig"] = if structured {
            json!({
                "functionCallingConfig": {
                    "mode": "ANY",
                    "allowedFunctionNames": [STRUCTURED_TOOL]
                }
            })
        } else {
            json!({"functionCallingConfig": {"mode": "VALIDATED"}})
        };
    }
    let mut generation_config = serde_json::Map::new();
    if model.supports_thinking {
        if let Some(thinking_budget) = model.thinking_budget {
            generation_config.insert(
                "thinkingConfig".to_string(),
                json!({
                    "includeThoughts": true,
                    "thinkingBudget": thinking_budget
                }),
            );
        }
    }
    if let Some(max_output_tokens) = model.max_output_tokens {
        generation_config.insert(
            "maxOutputTokens".to_string(),
            Value::Number(max_output_tokens.into()),
        );
    }
    if !generation_config.is_empty() {
        request["generationConfig"] = Value::Object(generation_config);
    }
    json!({
        "project": project_id,
        "requestId": format!("agent/{agent}/{}/{trajectory}/{step}", crate::session::now_unix_millis()),
        "request": request,
        "model": model.id.as_str(),
        "userAgent": "antigravity",
        "requestType": "agent"
    })
}

fn gemini_history(entries: &[TranscriptEntry]) -> Vec<Value> {
    entries
        .iter()
        .map(|entry| match entry {
            TranscriptEntry::User { text, images } => {
                let mut parts = vec![json!({"text":text})];
                parts.extend(images.iter().map(|image| json!({
                    "inlineData": {"mimeType":image.mime_type,"data":image.data_base64}
                })));
                json!({"role":"user","parts":parts})
            }
            TranscriptEntry::AssistantText { text } => json!({"role":"model","parts":[{"text":text}]}),
            TranscriptEntry::AssistantReasoning { text } => json!({"role":"model","parts":[{"text":format!("[reasoning summary]\n{text}")}]}),
            TranscriptEntry::ToolCall {
                id,
                name,
                arguments,
                thought_signature,
            } => {
                let mut part = json!({"functionCall":{"id":id,"name":name,"args":arguments}});
                if let Some(signature) = thought_signature {
                    part["thoughtSignature"] = Value::String(signature.clone());
                }
                json!({"role":"model","parts":[part]})
            }
            TranscriptEntry::ToolResult { id, name, result, is_error } => json!({"role":"user","parts":[{"functionResponse":{"id":id,"name":name,"response":if *is_error {json!({"error":result})} else {json!({"output":result})}}}]}),
            TranscriptEntry::Compaction { summary, .. } => json!({"role":"user","parts":[{"text":format!("[compacted conversation]\n{summary}")}]}),
        })
        .collect()
}

struct AntigravityAssistantStep {
    step: NormalizedAssistantStep,
    thought_signatures: HashMap<String, String>,
}

async fn parse_stream(
    mut response: reqwest::Response,
    cancellation: &mut tokio::sync::watch::Receiver<u64>,
    generation: u64,
    round: usize,
) -> Result<AntigravityAssistantStep, String> {
    let mut decoder = SseDecoder::default();
    let mut step = NormalizedAssistantStep::default();
    let mut thought_signatures = HashMap::new();
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(|_| "provider_transport_closed".to_string())?,
            changed = cancellation.changed() => {
                if changed.is_ok() && *cancellation.borrow() != generation {
                    return Err("provider_cancelled".to_string());
                }
                continue;
            }
        };
        let Some(chunk) = chunk else { break };
        for event in decoder.push(&chunk)? {
            let value: Value = serde_json::from_str(&event.data)
                .map_err(|_| "provider_protocol_changed".to_string())?;
            apply_chunk(&mut step, &mut thought_signatures, &value, round)?;
        }
    }
    decoder.finish()?;
    if step.text.is_empty() && step.tool_calls.is_empty() {
        return Err("provider returned an empty response".to_string());
    }
    Ok(AntigravityAssistantStep {
        step,
        thought_signatures,
    })
}

fn apply_chunk(
    step: &mut NormalizedAssistantStep,
    thought_signatures: &mut HashMap<String, String>,
    value: &Value,
    round: usize,
) -> Result<(), String> {
    if value.get("error").is_some() {
        return Err("provider_transport_closed".to_string());
    }
    let Some(response) = value.get("response") else {
        return Ok(());
    };
    if let Some(usage) = response.get("usageMetadata") {
        step.usage = parse_usage(usage);
    }
    if let Some(reason) = response
        .pointer("/candidates/0/finishReason")
        .and_then(Value::as_str)
    {
        step.finish_reason = Some(reason.to_string());
    }
    let parts = response
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (index, part) in parts.into_iter().enumerate() {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            if part
                .get("thought")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                step.reasoning.push_str(text);
            } else {
                step.text.push_str(text);
            }
        }
        if let Some(call) = part.get("functionCall") {
            let name = call
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            let arguments = call
                .get("args")
                .cloned()
                .filter(Value::is_object)
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("call-{round}-{index}"));
            if let Some(signature) = part.get("thoughtSignature").and_then(Value::as_str) {
                if signature.is_empty() || signature.len() > 64 * 1024 {
                    return Err("provider_protocol_changed".to_string());
                }
                thought_signatures.insert(id.clone(), signature.to_string());
            }
            step.tool_calls.push(DirectToolCall {
                id,
                name: name.to_string(),
                arguments,
            });
        }
    }
    Ok(())
}

fn parse_usage(value: &Value) -> Option<crate::ipc::ContextUsage> {
    let input = value.get("promptTokenCount")?.as_i64()?;
    let output = value
        .get("candidatesTokenCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let reasoning = value
        .get("thoughtsTokenCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cached = value
        .get("cachedContentTokenCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let total = value
        .get("totalTokenCount")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| input.saturating_add(output));
    let last = crate::ipc::TokenUsageBreakdown {
        input_tokens: input,
        cached_input_tokens: cached,
        cache_write_input_tokens: 0,
        output_tokens: output,
        reasoning_output_tokens: reasoning,
        total_tokens: total,
    };
    Some(crate::ipc::ContextUsage {
        last: last.clone(),
        total: last,
        model_context_window: None,
    })
}

fn signed_session_id(session_id: &str) -> String {
    let digest = Sha256::digest(session_id.as_bytes());
    i64::from_be_bytes(digest[..8].try_into().expect("slice length")).to_string()
}

fn stable_uuid(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    format!(
        "{:08x}-{:04x}-4{:03x}-a{:03x}-{:012x}",
        u32::from_be_bytes(digest[0..4].try_into().expect("slice")),
        u16::from_be_bytes(digest[4..6].try_into().expect("slice")),
        u16::from_be_bytes(digest[6..8].try_into().expect("slice")) & 0x0fff,
        u16::from_be_bytes(digest[8..10].try_into().expect("slice")) & 0x0fff,
        u64::from_be_bytes(digest[8..16].try_into().expect("slice")) & 0x0000_ffff_ffff_ffff,
    )
}

fn status_error(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        401 => "provider_cloud_code_unauthorized",
        403 => "provider_quota_exhausted",
        404 => "provider_protocol_changed",
        429 => "provider_rate_limited",
        500..=599 => "provider_transport_closed",
        _ => "provider_protocol_changed",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn future_live_model() -> LiveAntigravityModel {
        parse_live_model(
            "provider-future-a",
            &json!({
                "displayName": "Provider Future A",
                "supportsImages": true,
                "supportsThinking": true,
                "thinkingBudget": 12345,
                "maxTokens": 987654,
                "maxOutputTokens": 54321,
                "apiProvider": "provider-api",
                "modelProvider": "provider-model"
            }),
        )
        .unwrap()
    }

    #[test]
    fn catalog_uses_provider_models_without_a_local_allowlist() {
        let catalog = json!({
            "models": {
                "provider-future-a": {
                    "displayName": "Provider Future A",
                    "supportsImages": true,
                    "supportsThinking": true,
                    "thinkingBudget": 12345,
                    "maxTokens": 987654,
                    "maxOutputTokens": 54321,
                    "apiProvider": "provider-api",
                    "modelProvider": "provider-model"
                },
                "provider-future-b": {
                    "displayName": "Provider Future B",
                    "supportsImages": false,
                    "supportsThinking": false
                },
                "provider-internal": {
                    "displayName": "Internal",
                    "isInternal": true
                }
            },
            "agentModelSorts": [{
                "groups": [{"modelIds": [
                    "provider-future-b",
                    "provider-future-a",
                    "provider-internal"
                ]}]
            }]
        });
        let models = parse_live_catalog(&catalog).unwrap();
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["provider-future-b", "provider-future-a"]
        );
        let dynamic = models
            .iter()
            .find(|model| model.id == "provider-future-a")
            .unwrap();
        assert_eq!(dynamic.display_name, "Provider Future A");
        assert!(dynamic.supports_images);
        assert!(dynamic.supports_thinking);
        assert_eq!(dynamic.thinking_budget, Some(12_345));
        assert_eq!(dynamic.context_window, Some(987_654));
        assert_eq!(dynamic.max_output_tokens, Some(54_321));

        let view = dynamic.provider_model(Some("provider-future-a"));
        assert!(view.is_default);
        assert!(view.capabilities.vision);
        assert!(view.capabilities.reasoning_levels.is_empty());
        assert_eq!(view.capabilities.context_window, Some(987_654));
        assert_eq!(view.description, "provider-model · provider-api");
    }

    #[test]
    fn request_uses_only_the_selected_live_model_metadata() {
        let model = future_live_model();
        let request = request_body("session-a", 0, 0, "project", &model, &[], vec![], false);
        assert_eq!(request["model"], "provider-future-a");
        assert_eq!(
            request["request"]["generationConfig"]["thinkingConfig"],
            json!({"includeThoughts":true,"thinkingBudget":12345})
        );
        assert_eq!(
            request["request"]["generationConfig"]["maxOutputTokens"],
            54_321
        );
        assert!(request["request"]["labels"].get("model_enum").is_none());
        assert!(request["request"]["labels"].get("used_claude").is_none());
    }

    #[test]
    fn function_declarations_normalize_tool_schemas_for_cloud_code_assist() {
        let declarations = function_declarations(&[json!({
            "name": "file_edit",
            "description": "Edit a file.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "value": {
                        "type": ["integer", "string"],
                        "minimum": 0,
                        "x-eud-allowAnyString": true
                    },
                    "change": {
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {"code": {"type": "string"}},
                                "required": ["code"]
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "edits": {
                                        "type": "array",
                                        "items": {"type": "string"}
                                    }
                                },
                                "required": ["edits"]
                            }
                        ]
                    }
                },
                "required": ["value"]
            }
        })]);
        let schema = &declarations[0]["parameters"];
        assert_eq!(schema["type"], "object");
        assert!(schema.get("additionalProperties").is_none());
        assert_eq!(schema["properties"]["value"]["type"], "integer");
        assert!(schema["properties"]["value"].get("minimum").is_none());
        assert!(schema["properties"]["value"]
            .get("x-eud-allowAnyString")
            .is_none());
        let change = &schema["properties"]["change"];
        assert_eq!(change["type"], "object");
        assert!(change.get("oneOf").is_none());
        assert_eq!(change["properties"]["code"]["type"], "string");
        assert_eq!(change["properties"]["edits"]["type"], "array");
        let structured = structured_declaration(&json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {"ok": {"type": "boolean"}},
            "required": ["ok"]
        }));
        assert!(structured["parameters"]
            .get("additionalProperties")
            .is_none());
        assert_eq!(
            structured["parameters"]["properties"]["ok"]["type"],
            "boolean"
        );
    }

    #[test]
    fn stream_chunk_normalizes_text_thought_function_and_usage() {
        let mut step = NormalizedAssistantStep::default();
        let mut thought_signatures = HashMap::new();
        apply_chunk(&mut step, &mut thought_signatures, &json!({"response":{
            "candidates":[{"content":{"parts":[
                {"text":"thinking","thought":true},
                {"text":"answer"},
                {
                    "thoughtSignature":"signature-c1",
                    "functionCall":{"id":"c1","name":"read_file","args":{"path":"a.eps"}}
                }
            ]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3,"thoughtsTokenCount":2,"totalTokenCount":15}
        }}), 0).unwrap();
        assert_eq!(step.reasoning, "thinking");
        assert_eq!(step.text, "answer");
        assert_eq!(step.tool_calls[0].arguments, json!({"path":"a.eps"}));
        assert_eq!(
            thought_signatures.get("c1").map(String::as_str),
            Some("signature-c1")
        );
        let history = gemini_history(&[TranscriptEntry::ToolCall {
            id: "c1".to_string(),
            name: "read_file".to_string(),
            arguments: json!({"path":"a.eps"}),
            thought_signature: thought_signatures.remove("c1"),
        }]);
        assert_eq!(history[0]["parts"][0]["thoughtSignature"], "signature-c1");
        assert_eq!(step.usage.unwrap().last.total_tokens, 15);
    }

    #[test]
    fn request_identity_matches_antigravity_and_is_stable_per_session() {
        let model = future_live_model();
        let first = request_body("session-a", 0, 0, "project", &model, &[], vec![], false);
        let second = request_body("session-a", 0, 1, "project", &model, &[], vec![], false);
        assert_eq!(first["userAgent"], "antigravity");
        assert_eq!(
            first["request"]["sessionId"],
            second["request"]["sessionId"]
        );
        assert_ne!(first["requestId"], second["requestId"]);
    }
}
