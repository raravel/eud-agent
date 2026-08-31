//! Direct Ollama OpenAI-compatible provider with a pinned endpoint and local transcript.

use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;

use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::codex_client::{AgentTurnInput, WorkspaceAccess};
use crate::engine::{
    AgentDriver, AgentEngineError, AgentTurnResult, EngineEvent, EventSink, SessionEventSink,
};
use crate::opencode_go::{transcript_image, SseDecoder};
use crate::provider::{
    ModelCapabilities, ProviderConversationState, ProviderId, ProviderModel, ReasoningLevel,
    ReasoningSelection,
};
use crate::provider_secrets::ProviderSecretStore;
use crate::provider_tool_loop::{
    validate_structured_output, DirectToolCall, DirectToolResult, NormalizedAssistantStep,
    ProviderToolDispatcher,
};
use crate::provider_transcript::{ProviderTranscriptStore, TranscriptEntry};
use crate::tool_exec::SessionToolRuntime;
use crate::workspace::{PreparedWorkspace, WorkspaceManager, WorkspaceTurnRecorder};

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOOL_ROUNDS: usize = 64;
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

pub struct ProductionOllamaDriver {
    session_id: String,
    model: String,
    reasoning: Option<ReasoningSelection>,
    base_url: String,
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

impl ProductionOllamaDriver {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        session_id: String,
        model: String,
        reasoning: Option<ReasoningSelection>,
        base_url: String,
        dirs: crate::config::DataDirs,
        runtime: SessionToolRuntime,
        sink: SessionEventSink,
        revision: u64,
        cancellation: tokio::sync::watch::Receiver<u64>,
    ) -> Result<Self, AgentEngineError> {
        let model = validate_model(&model)
            .map_err(AgentEngineError::new)?
            .to_string();
        validate_reasoning(reasoning.as_ref()).map_err(AgentEngineError::new)?;
        let base_url = normalize_base_url(&base_url).map_err(AgentEngineError::new)?;
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(600))
            .user_agent(concat!("eud-agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| AgentEngineError::new("provider_transport_closed"))?;
        let secrets = ProviderSecretStore::new(dirs.clone()).map_err(AgentEngineError::new)?;
        Ok(Self {
            session_id,
            model,
            reasoning,
            base_url,
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
        let api_key = self
            .secrets
            .read_secret(ProviderId::Ollama, "api-key")
            .map_err(AgentEngineError::new)?
            .map(Zeroizing::new);
        self.sink
            .emit(EngineEvent::Progress(crate::ipc::ProgressEvent {
                stage: crate::ipc::ProgressStage::Provider,
                detail: Some("Ollama turn started".to_string()),
                provider: Some(ProviderId::Ollama),
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
                .load_current(ProviderId::Ollama, &self.session_id)
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
            let tools = if input.forbid_tools || structured {
                Vec::new()
            } else {
                chat_tools(&self.dispatcher.descriptors())
            };
            let body = build_chat_request(
                &self.model,
                self.reasoning.as_ref(),
                &entries,
                tools,
                selected_output_schema(structured, output_schema.as_ref()),
            );
            let mut request = self
                .client
                .post(format!("{}/chat/completions", self.base_url));
            if let Some(api_key) = api_key
                .as_deref()
                .map(|key| key.trim())
                .filter(|key| !key.is_empty())
            {
                request = request.bearer_auth(api_key);
            }
            let response = request
                .json(&body)
                .send()
                .await
                .map_err(|_| AgentEngineError::new("provider_transport_closed"))?;
            if !response.status().is_success() {
                return Err(AgentEngineError::new(status_error(response.status())));
            }
            let step = parse_chat_stream(response, &mut self.cancellation, generation)
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

            if structured {
                if !step.tool_calls.is_empty() {
                    return Err(AgentEngineError::new("provider_structured_output_invalid"));
                }
                let value: Value = serde_json::from_str(&step.text)
                    .map_err(|_| AgentEngineError::new("provider_structured_output_invalid"))?;
                validate_structured_output(output_schema.as_ref().expect("checked"), &value)
                    .map_err(AgentEngineError::new)?;
                let text = serde_json::to_string(&value)
                    .map_err(|_| AgentEngineError::new("provider_structured_output_invalid"))?;
                self.publish(entries)?;
                if let Some(recorder) = recorder.as_mut() {
                    recorder
                        .finish()
                        .map_err(|error| AgentEngineError::new(error.to_string()))?;
                }
                return Ok(AgentTurnResult::Answer { text });
            }

            let tool_calls = step.tool_calls;
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
            .publish(ProviderId::Ollama, &self.session_id, self.revision, entries)
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

impl AgentDriver for ProductionOllamaDriver {
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
            .load_current(ProviderId::Ollama, &self.session_id)
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
                ProviderId::Ollama,
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
        ProviderConversationState::Ollama {
            transcript_revision: self.revision,
        }
    }

    async fn seed_conversation(
        &mut self,
        state: ProviderConversationState,
    ) -> Result<(), AgentEngineError> {
        let ProviderConversationState::Ollama {
            transcript_revision,
        } = state
        else {
            return Err(AgentEngineError::new(
                "Ollama driver received incompatible conversation state",
            ));
        };
        if transcript_revision > 0 {
            let loaded = self
                .transcripts
                .load_current(ProviderId::Ollama, &self.session_id)
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

fn chat_tools(descriptors: &[Value]) -> Vec<Value> {
    descriptors
        .iter()
        .filter_map(|descriptor| {
            let name = descriptor.get("name")?.as_str()?;
            let description = descriptor
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            let schema = descriptor
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({"type":"object"}));
            Some(json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": description,
                    "parameters": schema,
                    "strict": true
                }
            }))
        })
        .collect()
}

fn selected_output_schema(structured: bool, output_schema: Option<&Value>) -> Option<&Value> {
    // `bool::then_some` evaluates eagerly; normal turns intentionally have no schema.
    if structured {
        output_schema
    } else {
        None
    }
}

fn build_chat_request(
    model: &str,
    reasoning: Option<&ReasoningSelection>,
    entries: &[TranscriptEntry],
    tools: Vec<Value>,
    output_schema: Option<&Value>,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": chat_history(entries),
        "stream": true,
        "stream_options": {"include_usage": true}
    });
    if let Some(reasoning) = reasoning {
        body["reasoning_effort"] = Value::String(reasoning.level.clone());
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(schema) = output_schema {
        body["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {
                "name": STRUCTURED_OUTPUT_NAME,
                "strict": true,
                "schema": schema
            }
        });
    }
    body
}

fn chat_history(entries: &[TranscriptEntry]) -> Vec<Value> {
    entries
        .iter()
        .map(|entry| match entry {
            TranscriptEntry::User { text, images } if images.is_empty() => {
                json!({"role":"user","content":text})
            }
            TranscriptEntry::User { text, images } => {
                let mut content = vec![json!({"type":"text","text":text})];
                content.extend(images.iter().map(|image| json!({
                    "type":"image_url",
                    "image_url":{"url":format!("data:{};base64,{}", image.mime_type, image.data_base64)}
                })));
                json!({"role":"user","content":content})
            }
            TranscriptEntry::AssistantText { text } => {
                json!({"role":"assistant","content":text})
            }
            TranscriptEntry::AssistantReasoning { text } => {
                json!({"role":"assistant","content":format!("[reasoning summary]\n{text}")})
            }
            TranscriptEntry::ToolCall {
                id,
                name,
                arguments,
                ..
            } => json!({
                "role":"assistant",
                "content":Value::Null,
                "tool_calls":[{
                    "id":id,
                    "type":"function",
                    "function":{
                        "name":name,
                        "arguments":serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_string())
                    }
                }]
            }),
            TranscriptEntry::ToolResult { id, result, .. } => json!({
                "role":"tool",
                "tool_call_id":id,
                "content":serde_json::to_string(result).unwrap_or_else(|_| "null".to_string())
            }),
            TranscriptEntry::Compaction { summary, .. } => {
                json!({"role":"user","content":format!("[compacted conversation]\n{summary}")})
            }
        })
        .collect()
}

async fn parse_chat_stream(
    mut response: reqwest::Response,
    cancellation: &mut tokio::sync::watch::Receiver<u64>,
    generation: u64,
) -> Result<NormalizedAssistantStep, String> {
    let mut decoder = SseDecoder::default();
    let mut parser = ChatParser::default();
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
            parser.apply(&value)?;
        }
    }
    decoder.finish()?;
    parser.finish()
}

#[derive(Default)]
struct ChatParser {
    step: NormalizedAssistantStep,
    calls: BTreeMap<u64, (String, String, String)>,
}

impl ChatParser {
    fn apply(&mut self, value: &Value) -> Result<(), String> {
        if value.get("error").is_some() {
            return Err("provider_protocol_changed".to_string());
        }
        if let Some(usage) = value.get("usage") {
            self.step.usage = parse_usage(usage);
        }
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return Ok(());
        };
        if let Some(finish) = choice.get("finish_reason").and_then(Value::as_str) {
            self.step.finish_reason = Some(finish.to_string());
        }
        let delta = choice
            .get("delta")
            .ok_or_else(|| "provider_protocol_changed".to_string())?;
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            self.step.text.push_str(text);
        }
        if let Some(reasoning) = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(Value::as_str)
        {
            self.step.reasoning.push_str(reasoning);
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in tool_calls {
                let index = call
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "provider_protocol_changed".to_string())?;
                let entry = self
                    .calls
                    .entry(index)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    entry.0 = id.to_string();
                }
                if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                    entry.1 = name.to_string();
                }
                if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str)
                {
                    entry.2.push_str(arguments);
                }
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<NormalizedAssistantStep, String> {
        for (id, name, arguments) in self.calls.into_values() {
            let arguments = serde_json::from_str(if arguments.trim().is_empty() {
                "{}"
            } else {
                &arguments
            })
            .map_err(|_| "provider returned invalid tool arguments".to_string())?;
            self.step.tool_calls.push(DirectToolCall {
                id,
                name,
                arguments,
            });
        }
        if self.step.text.is_empty() && self.step.tool_calls.is_empty() {
            return Err("provider returned an empty response".to_string());
        }
        Ok(self.step)
    }
}

fn parse_usage(value: &Value) -> Option<crate::ipc::ContextUsage> {
    let input = value.get("prompt_tokens").and_then(Value::as_i64)?;
    let output = value
        .get("completion_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cached = value
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let reasoning = value
        .pointer("/completion_tokens_details/reasoning_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let total = value
        .get("total_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(input.saturating_add(output));
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
mod tests {
    use super::*;

    #[test]
    fn base_url_normalization_accepts_local_http_and_requires_remote_tls() {
        assert_eq!(
            normalize_base_url("http://localhost:11434").unwrap(),
            crate::provider::DEFAULT_OLLAMA_BASE_URL
        );
        assert_eq!(
            normalize_base_url("https://ollama.example.test/openai/v1/").unwrap(),
            "https://ollama.example.test/openai/v1"
        );
        assert_eq!(
            normalize_base_url("http://192.168.0.5:11434/v1"),
            Err("provider_endpoint_invalid".to_string())
        );
        assert!(normalize_base_url("https://user:secret@example.test/v1").is_err());
        assert!(normalize_base_url("http://[::1]:11434/v1").is_ok());
    }

    #[test]
    fn chat_request_uses_tools_reasoning_and_json_schema_without_unsupported_choice_fields() {
        let entries = vec![TranscriptEntry::User {
            text: "inspect".to_string(),
            images: Vec::new(),
        }];
        let tools = chat_tools(&[json!({
            "name": "read_file",
            "description": "Read one file",
            "inputSchema": {"type":"object","required":["path"]}
        })]);
        let body = build_chat_request(
            "qwen3:8b",
            Some(&ReasoningSelection {
                level: "high".to_string(),
            }),
            &entries,
            tools,
            Some(&json!({"type":"object","required":["ok"]})),
        );
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("parallel_tool_calls").is_none());
    }

    #[test]
    fn unstructured_chat_request_does_not_require_an_output_schema() {
        let entries = vec![TranscriptEntry::User {
            text: "hi".to_string(),
            images: Vec::new(),
        }];
        let body = build_chat_request(
            "gemma4:e4b",
            None,
            &entries,
            Vec::new(),
            selected_output_schema(false, None),
        );
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn streamed_chat_parser_assembles_text_reasoning_tools_and_usage() {
        let mut parser = ChatParser::default();
        parser
            .apply(&json!({"choices":[{"delta":{"reasoning":"why","content":"done","tool_calls":[{"index":0,"id":"call-1","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}))
            .unwrap();
        parser
            .apply(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a.eps\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4,"completion_tokens":3,"total_tokens":7}}))
            .unwrap();
        let step = parser.finish().unwrap();
        assert_eq!(step.reasoning, "why");
        assert_eq!(step.text, "done");
        assert_eq!(step.tool_calls[0].id, "call-1");
        assert_eq!(step.tool_calls[0].arguments, json!({"path":"a.eps"}));
        assert_eq!(step.usage.unwrap().last.total_tokens, 7);
    }

    #[tokio::test]
    async fn probe_uses_the_configured_v1_path_and_optional_bearer_key() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 4_096];
            let count = stream.read(&mut buffer).unwrap();
            sender
                .send(String::from_utf8_lossy(&buffer[..count]).into_owned())
                .unwrap();
            let body = r#"{"object":"list","data":[]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let client = reqwest::Client::builder().build().unwrap();
        probe(&client, &format!("http://{address}/v1"), Some("proxy-key"))
            .await
            .unwrap();
        let request = receiver.recv().unwrap();
        assert!(request.starts_with("GET /v1/models HTTP/1.1\r\n"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer proxy-key\r\n"));
        server.join().unwrap();
    }
}
