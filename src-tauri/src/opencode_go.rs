//! Direct OpenCode Go catalog, three exact wire adapters, and sequential tool loop.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use base64::Engine as _;
use serde_json::{json, Value};
use sha2::Digest as _;
use zeroize::Zeroizing;

use crate::codex_client::{AgentTurnInput, WorkspaceAccess};
use crate::engine::{
    AgentDriver, AgentEngineError, AgentTurnResult, EngineEvent, EventSink, SessionEventSink,
};
use crate::provider::{
    ModelCapabilities, ProviderConversationState, ProviderId, ProviderModel, ReasoningSelection,
};
use crate::provider_secrets::ProviderSecretStore;
use crate::provider_tool_loop::{
    validate_structured_output, DirectToolCall, DirectToolResult, NormalizedAssistantStep,
    ProviderToolDispatcher,
};
use crate::provider_transcript::{ProviderTranscriptStore, TranscriptEntry, TranscriptImage};
use crate::tool_exec::SessionToolRuntime;
use crate::workspace::{PreparedWorkspace, WorkspaceManager, WorkspaceTurnRecorder};

const BASE_URL: &str = "https://opencode.ai/zen/go/v1";
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_TOOL_ROUNDS: usize = 64;
const STRUCTURED_TOOL: &str = "submit_structured_result";

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

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const MODELS_DEV_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);
static MODELS_DEV_CACHE: tokio::sync::Mutex<
    Option<(tokio::time::Instant, Vec<LiveOpenCodeGoModel>)>,
> = tokio::sync::Mutex::const_new(None);

fn wire_from_npm(npm: &str) -> Option<OpenCodeGoWire> {
    match npm {
        "@ai-sdk/openai" => Some(OpenCodeGoWire::Responses),
        "@ai-sdk/openai-compatible" => Some(OpenCodeGoWire::ChatCompletions),
        "@ai-sdk/anthropic" => Some(OpenCodeGoWire::AnthropicMessages),
        _ => None,
    }
}

fn parse_models_dev(value: &Value) -> Result<Vec<LiveOpenCodeGoModel>, String> {
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

async fn fetch_models_dev_at(
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

async fn fetch_models_dev(client: &reqwest::Client) -> Result<Vec<LiveOpenCodeGoModel>, String> {
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

async fn fetch_live_ids_at(
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

fn join_live_catalog(
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

async fn fetch_live_catalog(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<Vec<LiveOpenCodeGoModel>, String> {
    let (live_ids, metadata) = tokio::try_join!(
        fetch_live_ids_at(client, api_key, BASE_URL),
        fetch_models_dev(client)
    )?;
    Ok(join_live_catalog(&live_ids, &metadata))
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

pub struct ProductionOpenCodeGoDriver {
    session_id: String,
    model: String,
    client: reqwest::Client,
    secrets: ProviderSecretStore,
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

impl ProductionOpenCodeGoDriver {
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
            .user_agent(concat!("eud-agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| AgentEngineError::new("provider_catalog_unavailable"))?;
        let secrets = ProviderSecretStore::new(dirs.clone()).map_err(AgentEngineError::new)?;
        Ok(Self {
            session_id,
            model,
            client,
            secrets,
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

    pub fn use_workspace(&mut self, workspace: PreparedWorkspace) {
        self.workspace_override = Some(workspace);
    }

    pub fn disable_session_persistence(&mut self) {
        self.persist_context_usage = false;
    }

    async fn run_direct_turn(
        &mut self,
        mut input: AgentTurnInput,
    ) -> Result<AgentTurnResult, AgentEngineError> {
        let api_key = Zeroizing::new(
            self.secrets
                .read_secret(ProviderId::OpencodeGo, "api-key")
                .map_err(AgentEngineError::new)?
                .ok_or_else(|| AgentEngineError::new("provider_credential_missing"))?,
        );
        let live_model = fetch_live_catalog(&self.client, &api_key)
            .await
            .map_err(AgentEngineError::new)?
            .into_iter()
            .find(|model| model.id == self.model)
            .ok_or_else(|| AgentEngineError::new("provider_model_unavailable"))?;
        if !input.image_paths.is_empty() && !live_model.vision {
            return Err(AgentEngineError::new("provider_capability_unsupported"));
        }
        if !input.forbid_tools && !live_model.tool_calls {
            return Err(AgentEngineError::new("provider_capability_unsupported"));
        }
        if input.output_schema.is_some() && !live_model.structured_output {
            return Err(AgentEngineError::new("provider_capability_unsupported"));
        }
        let wire = live_model.wire;
        self.sink
            .emit(EngineEvent::Progress(crate::ipc::ProgressEvent {
                stage: crate::ipc::ProgressStage::Provider,
                detail: Some("OpenCode Go turn started".to_string()),
                provider: Some(ProviderId::OpencodeGo),
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
        let workspace_manager = self.workspace.clone();
        let workspace_override = self.workspace_override.clone();
        let session_id = self.session_id.clone();
        let baseline_request = request_id.clone();
        let access = input.workspace_access;
        let (workspace, baseline) = tokio::task::spawn_blocking(move || {
            let workspace = match workspace_override {
                Some(workspace) => workspace,
                None => workspace_manager.prepare_session_current(&session_id)?,
            };
            let baseline = if access == WorkspaceAccess::Write {
                Some(
                    workspace_manager
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

        let generation = *self.cancellation.borrow_and_update();
        if *self.cancellation.borrow() != generation {
            return Ok(AgentTurnResult::Cancelled);
        }
        let mut entries = if self.revision == 0 {
            Vec::new()
        } else {
            let current = self
                .transcripts
                .load_current(ProviderId::OpencodeGo, &self.session_id)
                .map_err(AgentEngineError::new)?;
            if current.revision != self.revision {
                return Err(AgentEngineError::new(
                    "provider transcript revision mismatch",
                ));
            }
            current.entries
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

        let output_schema = input.output_schema.clone();
        let structured = input.forbid_tools && output_schema.is_some();
        let mut seen_tool_ids = HashSet::new();
        let mut answer = String::new();
        for _round in 0..MAX_TOOL_ROUNDS {
            if *self.cancellation.borrow() != generation {
                return Ok(AgentTurnResult::Cancelled);
            }
            let tools = if structured {
                vec![structured_tool(
                    output_schema.as_ref().expect("checked"),
                    wire,
                )]
            } else if input.forbid_tools {
                Vec::new()
            } else {
                wire_tools(&self.dispatcher.descriptors(), wire)
            };
            let body = build_request(
                wire,
                live_model.max_output_tokens,
                &self.model,
                &entries,
                tools,
                structured,
            )?;
            let request = authenticated_request(
                &self.client,
                format!("{BASE_URL}/{}", wire_path(wire)),
                wire,
                &api_key,
                &self.session_id,
            );
            let response = request
                .json(&body)
                .send()
                .await
                .map_err(|_| AgentEngineError::new("provider_transport_closed"))?;
            if !response.status().is_success() {
                return Err(AgentEngineError::new(status_error(response.status())));
            }
            let step = parse_stream(response, wire, &mut self.cancellation, generation)
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
                let schema = output_schema.as_ref().expect("checked");
                validate_structured_output(schema, &call.arguments)
                    .map_err(AgentEngineError::new)?;
                let text = serde_json::to_string(&call.arguments)
                    .map_err(|_| AgentEngineError::new("provider_structured_output_invalid"))?;
                entries.push(TranscriptEntry::ToolCall {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                    thought_signature: None,
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
                    thought_signature: None,
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
                ProviderId::OpencodeGo,
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

impl AgentDriver for ProductionOpenCodeGoDriver {
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
            .load_current(ProviderId::OpencodeGo, &self.session_id)
            .map_err(AgentEngineError::new)?;
        let prompt = "Summarize the conversation for exact continuation. Preserve system/developer invariants, accepted plan, current task state, unresolved ASK/review, decisions, evidence, and file identities.";
        let result = self
            .run_direct_turn(AgentTurnInput::text(prompt).without_tools())
            .await?;
        let AgentTurnResult::Answer { text: summary } = result else {
            return Err(AgentEngineError::new("provider_structured_output_invalid"));
        };
        let generation = self
            .transcripts
            .publish(
                ProviderId::OpencodeGo,
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
        ProviderConversationState::OpencodeGo {
            transcript_revision: self.revision,
        }
    }

    async fn seed_conversation(
        &mut self,
        state: ProviderConversationState,
    ) -> Result<(), AgentEngineError> {
        let ProviderConversationState::OpencodeGo {
            transcript_revision,
        } = state
        else {
            return Err(AgentEngineError::new(
                "OpenCode Go driver received incompatible conversation state",
            ));
        };
        if transcript_revision > 0 {
            let loaded = self
                .transcripts
                .load_current(ProviderId::OpencodeGo, &self.session_id)
                .map_err(AgentEngineError::new)?;
            if loaded.revision != transcript_revision {
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

fn wire_path(wire: OpenCodeGoWire) -> &'static str {
    match wire {
        OpenCodeGoWire::Responses => "responses",
        OpenCodeGoWire::ChatCompletions => "chat/completions",
        OpenCodeGoWire::AnthropicMessages => "messages",
    }
}

fn authenticated_request(
    client: &reqwest::Client,
    url: String,
    wire: OpenCodeGoWire,
    api_key: &str,
    session_id: &str,
) -> reqwest::RequestBuilder {
    let request = match wire {
        OpenCodeGoWire::AnthropicMessages => client
            .post(url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        OpenCodeGoWire::Responses | OpenCodeGoWire::ChatCompletions => {
            client.post(url).bearer_auth(api_key)
        }
    };
    request.header("x-opencode-session", session_id)
}

fn structured_tool(schema: &Value, wire: OpenCodeGoWire) -> Value {
    match wire {
        OpenCodeGoWire::Responses => json!({
            "type": "function",
            "name": STRUCTURED_TOOL,
            "description": "Submit the final structured result.",
            "parameters": schema,
            "strict": true
        }),
        OpenCodeGoWire::ChatCompletions => json!({
            "type": "function",
            "function": {
                "name": STRUCTURED_TOOL,
                "description": "Submit the final structured result.",
                "parameters": schema,
                "strict": true
            }
        }),
        OpenCodeGoWire::AnthropicMessages => json!({
            "name": STRUCTURED_TOOL,
            "description": "Submit the final structured result.",
            "input_schema": schema
        }),
    }
}

fn wire_tools(descriptors: &[Value], wire: OpenCodeGoWire) -> Vec<Value> {
    descriptors
        .iter()
        .filter_map(|descriptor| {
            let name = descriptor.get("name")?.as_str()?;
            let description = descriptor
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            let schema = descriptor.get("inputSchema").cloned().unwrap_or_else(|| json!({"type":"object"}));
            Some(match wire {
                OpenCodeGoWire::Responses => json!({
                    "type": "function", "name": name, "description": description,
                    "parameters": schema, "strict": true
                }),
                OpenCodeGoWire::ChatCompletions => json!({
                    "type": "function", "function": {"name": name, "description": description, "parameters": schema, "strict": true}
                }),
                OpenCodeGoWire::AnthropicMessages => json!({
                    "name": name, "description": description, "input_schema": schema
                }),
            })
        })
        .collect()
}

fn build_request(
    wire: OpenCodeGoWire,
    max_output_tokens: Option<u64>,
    model: &str,
    entries: &[TranscriptEntry],
    tools: Vec<Value>,
    structured: bool,
) -> Result<Value, AgentEngineError> {
    let mut body = match wire {
        OpenCodeGoWire::Responses => json!({
            "model": model,
            "input": responses_history(entries),
            "tools": tools,
            "stream": true,
            "store": false
        }),
        OpenCodeGoWire::ChatCompletions => json!({
            "model": model,
            "messages": chat_history(entries),
            "tools": tools,
            "stream": true,
            "stream_options": {"include_usage": true}
        }),
        OpenCodeGoWire::AnthropicMessages => json!({
            "model": model,
            "max_tokens": max_output_tokens.ok_or_else(|| {
                AgentEngineError::new("provider_capability_unsupported")
            })?,
            "messages": anthropic_history(entries),
            "tools": tools,
            "stream": true
        }),
    };
    if let Some(max_output_tokens) = max_output_tokens {
        match wire {
            OpenCodeGoWire::Responses => {
                body["max_output_tokens"] = Value::Number(max_output_tokens.into())
            }
            OpenCodeGoWire::ChatCompletions => {
                body["max_tokens"] = Value::Number(max_output_tokens.into())
            }
            OpenCodeGoWire::AnthropicMessages => {}
        }
    }
    if tools.is_empty() {
        body.as_object_mut().expect("object").remove("tools");
    } else if structured {
        match wire {
            OpenCodeGoWire::Responses => {
                body["tool_choice"] = json!({"type":"function","name":STRUCTURED_TOOL})
            }
            OpenCodeGoWire::ChatCompletions => {
                body["tool_choice"] = json!({"type":"function","function":{"name":STRUCTURED_TOOL}})
            }
            OpenCodeGoWire::AnthropicMessages => {
                body["tool_choice"] =
                    json!({"type":"tool","name":STRUCTURED_TOOL,"disable_parallel_tool_use":true})
            }
        }
    } else if wire == OpenCodeGoWire::AnthropicMessages {
        body["tool_choice"] = json!({"type":"auto","disable_parallel_tool_use":true});
    } else {
        body["parallel_tool_calls"] = Value::Bool(false);
    }
    Ok(body)
}

fn responses_history(entries: &[TranscriptEntry]) -> Vec<Value> {
    entries
        .iter()
        .map(|entry| match entry {
            TranscriptEntry::User { text, images } => {
                let mut content = vec![json!({"type":"input_text","text":text})];
                content.extend(images.iter().map(|image| json!({
                    "type":"input_image",
                    "image_url": format!("data:{};base64,{}", image.mime_type, image.data_base64)
                })));
                json!({"role":"user","content":content})
            }
            TranscriptEntry::AssistantText { text } => json!({"role":"assistant","content":[{"type":"output_text","text":text}]}),
            TranscriptEntry::AssistantReasoning { text } => json!({"role":"assistant","content":[{"type":"output_text","text":format!("[reasoning summary]\n{text}")}]}),
            TranscriptEntry::ToolCall { id, name, arguments, .. } => json!({"type":"function_call","call_id":id,"name":name,"arguments":serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_string())}),
            TranscriptEntry::ToolResult { id, result, .. } => json!({"type":"function_call_output","call_id":id,"output":serde_json::to_string(result).unwrap_or_else(|_| "null".to_string())}),
            TranscriptEntry::Compaction { summary, .. } => json!({"role":"user","content":[{"type":"input_text","text":format!("[compacted conversation]\n{summary}")}]}),
        })
        .collect()
}

fn chat_history(entries: &[TranscriptEntry]) -> Vec<Value> {
    entries
        .iter()
        .map(|entry| match entry {
            TranscriptEntry::User { text, images } if images.is_empty() => json!({"role":"user","content":text}),
            TranscriptEntry::User { text, images } => {
                let mut content = vec![json!({"type":"text","text":text})];
                content.extend(images.iter().map(|image| json!({"type":"image_url","image_url":{"url":format!("data:{};base64,{}",image.mime_type,image.data_base64)}})));
                json!({"role":"user","content":content})
            }
            TranscriptEntry::AssistantText { text } => json!({"role":"assistant","content":text}),
            TranscriptEntry::AssistantReasoning { text } => json!({"role":"assistant","content":format!("[reasoning summary]\n{text}")}),
            TranscriptEntry::ToolCall { id, name, arguments, .. } => json!({"role":"assistant","content":Value::Null,"tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_string())}}]}),
            TranscriptEntry::ToolResult { id, result, .. } => json!({"role":"tool","tool_call_id":id,"content":serde_json::to_string(result).unwrap_or_else(|_| "null".to_string())}),
            TranscriptEntry::Compaction { summary, .. } => json!({"role":"user","content":format!("[compacted conversation]\n{summary}")}),
        })
        .collect()
}

fn anthropic_history(entries: &[TranscriptEntry]) -> Vec<Value> {
    entries
        .iter()
        .map(|entry| match entry {
            TranscriptEntry::User { text, images } => {
                let mut content = vec![json!({"type":"text","text":text})];
                content.extend(images.iter().map(|image| json!({"type":"image","source":{"type":"base64","media_type":image.mime_type,"data":image.data_base64}})));
                json!({"role":"user","content":content})
            }
            TranscriptEntry::AssistantText { text } => json!({"role":"assistant","content":[{"type":"text","text":text}]}),
            TranscriptEntry::AssistantReasoning { text } => json!({"role":"assistant","content":[{"type":"thinking","thinking":text,"signature":""}]}),
            TranscriptEntry::ToolCall { id, name, arguments, .. } => json!({"role":"assistant","content":[{"type":"tool_use","id":id,"name":name,"input":arguments}]}),
            TranscriptEntry::ToolResult { id, result, is_error, .. } => json!({"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":serde_json::to_string(result).unwrap_or_else(|_| "null".to_string()),"is_error":is_error}]}),
            TranscriptEntry::Compaction { summary, .. } => json!({"role":"user","content":[{"type":"text","text":format!("[compacted conversation]\n{summary}")}]}),
        })
        .collect()
}

pub(crate) fn transcript_image(path: &Path) -> Result<TranscriptImage, String> {
    let bytes = std::fs::read(path).map_err(|_| "provider image cannot be read".to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err("provider image size is unsupported".to_string());
    }
    let mime_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else {
        return Err("provider image format is unsupported".to_string());
    };
    let digest = sha2::Sha256::digest(&bytes);
    let id = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(TranscriptImage {
        id,
        mime_type: mime_type.to_string(),
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

async fn parse_stream(
    mut response: reqwest::Response,
    wire: OpenCodeGoWire,
    cancellation: &mut tokio::sync::watch::Receiver<u64>,
    generation: u64,
) -> Result<NormalizedAssistantStep, String> {
    let mut decoder = SseDecoder::default();
    let mut parser = WireParser::new(wire);
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
            if event.data == "[DONE]" {
                return parser.finish();
            }
            let value: Value = serde_json::from_str(&event.data)
                .map_err(|_| "provider_protocol_changed".to_string())?;
            parser.apply(event.event.as_deref(), value)?;
        }
    }
    decoder.finish()?;
    parser.finish()
}

#[derive(Default)]
pub(crate) struct SseDecoder {
    buffer: Vec<u8>,
}

pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

impl SseDecoder {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, String> {
        if self.buffer.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err("provider response is too large".to_string());
        }
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some((separator, delimiter_len)) = sse_separator(&self.buffer) {
            let frame = self.buffer[..separator].to_vec();
            self.buffer.drain(..separator + delimiter_len);
            let frame = std::str::from_utf8(&frame)
                .map_err(|_| "provider stream is not UTF-8".to_string())?;
            let mut event = None;
            let mut data = Vec::new();
            for line in frame.lines() {
                let line = line.trim_end_matches('\r');
                if let Some(value) = line.strip_prefix("event:") {
                    event = Some(value.trim().to_string());
                } else if let Some(value) = line.strip_prefix("data:") {
                    data.push(value.trim_start());
                }
            }
            if !data.is_empty() {
                events.push(SseEvent {
                    event,
                    data: data.join("\n"),
                });
            }
        }
        Ok(events)
    }

    pub(crate) fn finish(&self) -> Result<(), String> {
        if self.buffer.iter().all(u8::is_ascii_whitespace) {
            Ok(())
        } else {
            Err("provider stream ended with an incomplete event".to_string())
        }
    }
}

fn sse_separator(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (Some(_), Some(right)) => Some((right, 4)),
        (Some(left), None) => Some((left, 2)),
        (None, Some(right)) => Some((right, 4)),
        (None, None) => None,
    }
}

enum WireParser {
    Responses {
        step: NormalizedAssistantStep,
        calls: BTreeMap<String, (String, String)>,
    },
    Chat {
        step: NormalizedAssistantStep,
        calls: BTreeMap<u64, (String, String, String)>,
    },
    Anthropic {
        step: NormalizedAssistantStep,
        calls: BTreeMap<u64, (String, String, String)>,
    },
}

impl WireParser {
    fn new(wire: OpenCodeGoWire) -> Self {
        match wire {
            OpenCodeGoWire::Responses => Self::Responses {
                step: NormalizedAssistantStep::default(),
                calls: BTreeMap::new(),
            },
            OpenCodeGoWire::ChatCompletions => Self::Chat {
                step: NormalizedAssistantStep::default(),
                calls: BTreeMap::new(),
            },
            OpenCodeGoWire::AnthropicMessages => Self::Anthropic {
                step: NormalizedAssistantStep::default(),
                calls: BTreeMap::new(),
            },
        }
    }

    fn apply(&mut self, event: Option<&str>, value: Value) -> Result<(), String> {
        match self {
            Self::Responses { step, calls } => parse_responses_event(step, calls, event, &value),
            Self::Chat { step, calls } => parse_chat_event(step, calls, &value),
            Self::Anthropic { step, calls } => parse_anthropic_event(step, calls, event, &value),
        }
    }

    fn finish(self) -> Result<NormalizedAssistantStep, String> {
        let (mut step, calls) = match self {
            Self::Responses { step, calls } => {
                let calls = calls
                    .into_iter()
                    .map(|(id, (name, arguments))| (id, name, arguments))
                    .collect::<Vec<_>>();
                (step, calls)
            }
            Self::Chat { step, calls } | Self::Anthropic { step, calls } => {
                (step, calls.into_values().collect::<Vec<_>>())
            }
        };
        for (id, name, arguments) in calls {
            let arguments = serde_json::from_str(if arguments.trim().is_empty() {
                "{}"
            } else {
                &arguments
            })
            .map_err(|_| "provider returned invalid tool arguments".to_string())?;
            step.tool_calls.push(DirectToolCall {
                id,
                name,
                arguments,
            });
        }
        if step.text.is_empty() && step.tool_calls.is_empty() {
            return Err("provider returned an empty response".to_string());
        }
        Ok(step)
    }
}

fn parse_responses_event(
    step: &mut NormalizedAssistantStep,
    calls: &mut BTreeMap<String, (String, String)>,
    event: Option<&str>,
    value: &Value,
) -> Result<(), String> {
    let kind = event.or_else(|| value.get("type").and_then(Value::as_str));
    match kind {
        Some("response.output_text.delta") => {
            step.text.push_str(
                value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?,
            );
        }
        Some("response.reasoning_summary_text.delta") => {
            step.reasoning.push_str(
                value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?,
            );
        }
        Some("response.output_item.added" | "response.output_item.done") => {
            let item = value
                .get("item")
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                let id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?;
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?;
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let entry = calls
                    .entry(id.to_string())
                    .or_insert((name.to_string(), String::new()));
                if entry.0.is_empty() {
                    entry.0 = name.to_string();
                }
                if !arguments.is_empty() {
                    entry.1 = arguments.to_string();
                }
            }
        }
        Some("response.function_call_arguments.delta") => {
            let id = value
                .get("call_id")
                .or_else(|| value.get("item_id"))
                .and_then(Value::as_str)
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            let delta = value
                .get("delta")
                .and_then(Value::as_str)
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            calls
                .entry(id.to_string())
                .or_insert((String::new(), String::new()))
                .1
                .push_str(delta);
        }
        Some("response.completed") => {
            if let Some(usage) = value.pointer("/response/usage") {
                step.usage = parse_usage(usage, None);
            }
        }
        Some("error") | Some("response.failed") => {
            return Err("provider_transport_closed".to_string())
        }
        _ => {}
    }
    Ok(())
}

fn parse_chat_event(
    step: &mut NormalizedAssistantStep,
    calls: &mut BTreeMap<u64, (String, String, String)>,
    value: &Value,
) -> Result<(), String> {
    if let Some(usage) = value.get("usage") {
        step.usage = parse_usage(usage, None);
    }
    let Some(choice) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return Ok(());
    };
    if let Some(finish) = choice.get("finish_reason").and_then(Value::as_str) {
        step.finish_reason = Some(finish.to_string());
    }
    let delta = choice
        .get("delta")
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    if let Some(text) = delta.get("content").and_then(Value::as_str) {
        step.text.push_str(text);
    }
    if let Some(reasoning) = delta
        .get("reasoning_content")
        .or_else(|| delta.get("reasoning"))
        .and_then(Value::as_str)
    {
        step.reasoning.push_str(reasoning);
    }
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for call in tool_calls {
            let index = call
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            let entry = calls
                .entry(index)
                .or_insert_with(|| (String::new(), String::new(), String::new()));
            if let Some(id) = call.get("id").and_then(Value::as_str) {
                entry.0 = id.to_string();
            }
            if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                entry.1 = name.to_string();
            }
            if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
                entry.2.push_str(arguments);
            }
        }
    }
    Ok(())
}

fn parse_anthropic_event(
    step: &mut NormalizedAssistantStep,
    calls: &mut BTreeMap<u64, (String, String, String)>,
    event: Option<&str>,
    value: &Value,
) -> Result<(), String> {
    let kind = event.or_else(|| value.get("type").and_then(Value::as_str));
    match kind {
        Some("message_start") => {
            step.usage = value
                .pointer("/message/usage")
                .and_then(|usage| parse_usage(usage, None))
        }
        Some("content_block_start") => {
            let index = value
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            let block = value
                .get("content_block")
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?;
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?;
                let arguments = block
                    .get("input")
                    .filter(|input| input.as_object().is_some_and(|input| !input.is_empty()))
                    .map(|input| serde_json::to_string(input).unwrap_or_default())
                    .unwrap_or_default();
                calls.insert(index, (id.to_string(), name.to_string(), arguments));
            }
        }
        Some("content_block_delta") => {
            let index = value
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            let delta = value
                .get("delta")
                .ok_or_else(|| "provider_protocol_changed".to_string())?;
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => step.text.push_str(
                    delta
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "provider_protocol_changed".to_string())?,
                ),
                Some("thinking_delta") => step.reasoning.push_str(
                    delta
                        .get("thinking")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "provider_protocol_changed".to_string())?,
                ),
                Some("input_json_delta") => calls
                    .entry(index)
                    .or_insert_with(|| (String::new(), String::new(), String::new()))
                    .2
                    .push_str(
                        delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .ok_or_else(|| "provider_protocol_changed".to_string())?,
                    ),
                _ => {}
            }
        }
        Some("message_delta") => {
            step.finish_reason = value
                .pointer("/delta/stop_reason")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(usage) = value.get("usage") {
                step.usage = parse_usage(usage, step.usage.as_ref());
            }
        }
        Some("error") => return Err("provider_transport_closed".to_string()),
        _ => {}
    }
    Ok(())
}

fn parse_usage(
    value: &Value,
    prior: Option<&crate::ipc::ContextUsage>,
) -> Option<crate::ipc::ContextUsage> {
    let input = value
        .get("input_tokens")
        .or_else(|| value.get("prompt_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| prior.map(|usage| usage.last.input_tokens).unwrap_or(0));
    let output = value
        .get("output_tokens")
        .or_else(|| value.get("completion_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cached = value
        .get("cached_tokens")
        .or_else(|| value.pointer("/input_tokens_details/cached_tokens"))
        .or_else(|| value.get("cache_read_input_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cache_write = value
        .get("cache_creation_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let reasoning = value
        .get("reasoning_tokens")
        .or_else(|| value.pointer("/output_tokens_details/reasoning_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let total = value
        .get("total_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(input.saturating_add(output));
    let last = crate::ipc::TokenUsageBreakdown {
        input_tokens: input,
        cached_input_tokens: cached,
        cache_write_input_tokens: cache_write,
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
mod tests {
    use super::*;

    fn sse(lines: &[&str]) -> Vec<u8> {
        format!("{}\n\n", lines.join("\n\n")).into_bytes()
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

    #[test]
    fn responses_stream_assembles_text_reasoning_and_tool_arguments() {
        let bytes = sse(&[
            "event: response.output_text.delta\ndata: {\"delta\":\"answer\"}",
            "event: response.reasoning_summary_text.delta\ndata: {\"delta\":\"think\"}",
            "event: response.output_item.added\ndata: {\"item\":{\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"read_file\"}}",
            "event: response.function_call_arguments.delta\ndata: {\"call_id\":\"c1\",\"delta\":\"{\\\"path\\\":\\\"a.eps\\\"}\"}",
        ]);
        let mut decoder = SseDecoder::default();
        let mut parser = WireParser::new(OpenCodeGoWire::Responses);
        for event in decoder.push(&bytes).unwrap() {
            let value = serde_json::from_str(&event.data).unwrap();
            parser.apply(event.event.as_deref(), value).unwrap();
        }
        let step = parser.finish().unwrap();
        assert_eq!(step.text, "answer");
        assert_eq!(step.reasoning, "think");
        assert_eq!(step.tool_calls[0].arguments, json!({"path":"a.eps"}));
    }

    #[test]
    fn chat_and_anthropic_streams_preserve_tool_order() {
        let mut chat = WireParser::new(OpenCodeGoWire::ChatCompletions);
        chat.apply(
            None,
            json!({"choices":[{"delta":{"tool_calls":[
                {"index":0,"id":"a","function":{"name":"read_file","arguments":"{}"}},
                {"index":1,"id":"b","function":{"name":"search_docs","arguments":"{}"}}
            ]}}]}),
        )
        .unwrap();
        assert_eq!(
            chat.finish()
                .unwrap()
                .tool_calls
                .iter()
                .map(|call| call.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );

        let mut anthropic = WireParser::new(OpenCodeGoWire::AnthropicMessages);
        anthropic.apply(Some("content_block_start"), json!({"index":0,"content_block":{"type":"tool_use","id":"a","name":"read_file","input":{}}})).unwrap();
        anthropic.apply(Some("content_block_start"), json!({"index":1,"content_block":{"type":"tool_use","id":"b","name":"search_docs","input":{}}})).unwrap();
        assert_eq!(
            anthropic
                .finish()
                .unwrap()
                .tool_calls
                .iter()
                .map(|call| call.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn malformed_or_incomplete_sse_fails_closed() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(b"data: {bad}\n\n").is_ok());
        let mut incomplete = SseDecoder::default();
        incomplete.push(b"data: {\"x\":1}").unwrap();
        assert!(incomplete.finish().is_err());
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
}
