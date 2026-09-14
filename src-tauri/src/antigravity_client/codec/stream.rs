use serde_json::Value;

use crate::{
    opencode_go::SseDecoder,
    provider_runtime::{
        AdapterEvent, AdapterEventKind, AdapterRequestKind, AdapterStepRequest, NormalizedBlock,
        ProviderContinuation, ProviderRuntimeError,
    },
    provider_tool_loop::{validate_structured_output, DirectToolCall},
};

use super::{
    super::STRUCTURED_TOOL,
    metadata::{parse_usage, thought_continuation},
};

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub(in crate::antigravity_client) struct DecodedStep {
    pub(in crate::antigravity_client) text: String,
    pub(in crate::antigravity_client) calls: Vec<DirectToolCall>,
    pub(in crate::antigravity_client) structured: Option<Value>,
    pub(in crate::antigravity_client) continuations: Vec<Option<ProviderContinuation>>,
}

pub(in crate::antigravity_client) async fn decode_stream(
    mut response: reqwest::Response,
    request: &mut AdapterStepRequest,
    events: &tokio::sync::mpsc::Sender<AdapterEvent>,
    response_id: &str,
) -> Result<DecodedStep, ProviderRuntimeError> {
    if *request.cancellation.borrow_and_update() != request.identity.cancellation_generation {
        return Err(ProviderRuntimeError::Cancelled);
    }
    let mut state = StreamState::default();
    let mut decoder = SseDecoder::default();
    let mut received_bytes = 0usize;
    let mut cancellation_open = true;
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(|_| ProviderRuntimeError::Transport("provider_transport_closed".into()))?,
            changed = request.cancellation.changed(), if cancellation_open => {
                match changed {
                    Ok(()) if *request.cancellation.borrow_and_update() != request.identity.cancellation_generation => {
                        return Err(ProviderRuntimeError::Cancelled);
                    }
                    Ok(()) => {}
                    Err(_) => cancellation_open = false,
                }
                continue;
            }
        };
        let Some(chunk) = chunk else { break };
        received_bytes = received_bytes.checked_add(chunk.len()).ok_or_else(|| {
            ProviderRuntimeError::Protocol("provider response exceeded its size limit".into())
        })?;
        if received_bytes > MAX_RESPONSE_BYTES {
            return Err(ProviderRuntimeError::Protocol(
                "provider response exceeded its size limit".into(),
            ));
        }
        for event in decoder
            .push(&chunk)
            .map_err(ProviderRuntimeError::Protocol)?
        {
            let value: Value = serde_json::from_str(&event.data)
                .map_err(|_| ProviderRuntimeError::Protocol("provider_protocol_changed".into()))?;
            state.apply(&value, request, events, response_id).await?;
        }
    }
    decoder.finish().map_err(ProviderRuntimeError::Protocol)?;
    let reason = state.finish_reason.ok_or_else(|| {
        ProviderRuntimeError::Protocol("provider stream closed before completion".into())
    })?;
    let complete = reason == "STOP";
    send_event(
        request,
        events,
        AdapterEventKind::ResponseFinished {
            response_id: response_id.to_string(),
            finish_reason: Some(reason),
            complete,
        },
    )
    .await?;
    if !complete {
        return Err(ProviderRuntimeError::Protocol(
            "provider response was incomplete".into(),
        ));
    }
    let structured = structured_result(request, &state.calls)?;
    if state.text.is_empty() && state.calls.is_empty() {
        return Err(ProviderRuntimeError::Protocol(
            "provider returned an empty response".into(),
        ));
    }
    Ok(DecodedStep {
        text: state.text,
        calls: state.calls,
        structured,
        continuations: state.continuations,
    })
}

#[derive(Default)]
struct StreamState {
    text: String,
    calls: Vec<DirectToolCall>,
    continuations: Vec<Option<ProviderContinuation>>,
    finish_reason: Option<String>,
}

impl StreamState {
    async fn apply(
        &mut self,
        value: &Value,
        request: &AdapterStepRequest,
        events: &tokio::sync::mpsc::Sender<AdapterEvent>,
        response_id: &str,
    ) -> Result<(), ProviderRuntimeError> {
        if value.get("error").is_some() {
            return Err(ProviderRuntimeError::Transport(
                "provider_transport_closed".into(),
            ));
        }
        let Some(response) = value.get("response") else {
            return Ok(());
        };
        let usage = response.get("usageMetadata").and_then(parse_usage);
        if let Some(reason) = response
            .pointer("/candidates/0/finishReason")
            .and_then(Value::as_str)
        {
            self.finish_reason = Some(reason.to_string());
        }
        let parts = response
            .pointer("/candidates/0/content/parts")
            .and_then(Value::as_array);
        for part in parts.into_iter().flatten() {
            let continuation = thought_continuation(part)?;
            if let Some(delta) = part.get("text").and_then(Value::as_str) {
                let block = if part
                    .get("thought")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    NormalizedBlock::Reasoning {
                        response_id: response_id.to_string(),
                        text: delta.to_string(),
                        continuation: continuation.clone(),
                    }
                } else {
                    self.text.push_str(delta);
                    NormalizedBlock::Text {
                        response_id: response_id.to_string(),
                        text: delta.to_string(),
                    }
                };
                send_event(request, events, AdapterEventKind::Block(block)).await?;
            }
            if let Some(call) = part.get("functionCall") {
                let name = call.get("name").and_then(Value::as_str).ok_or_else(|| {
                    ProviderRuntimeError::Protocol("provider_protocol_changed".into())
                })?;
                let arguments = call
                    .get("args")
                    .cloned()
                    .filter(Value::is_object)
                    .ok_or_else(|| {
                        ProviderRuntimeError::Protocol("provider_protocol_changed".into())
                    })?;
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{response_id}-call-{}", self.calls.len()));
                let direct = DirectToolCall {
                    id,
                    name: name.to_string(),
                    arguments,
                };
                send_event(
                    request,
                    events,
                    AdapterEventKind::Block(NormalizedBlock::ToolCall {
                        response_id: response_id.to_string(),
                        batch_id: response_id.to_string(),
                        call: direct.clone(),
                        continuation: continuation.clone(),
                    }),
                )
                .await?;
                self.calls.push(direct);
            }
            if continuation.is_some() {
                self.continuations.push(continuation);
            }
        }
        if let Some(mut usage) = usage {
            if let Some(context) = usage.context_usage.as_mut() {
                context.model_context_window = request
                    .binding
                    .capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.context_window)
                    .and_then(|window| i64::try_from(window).ok());
            }
            send_event(request, events, AdapterEventKind::Usage(usage)).await?;
        }
        Ok(())
    }
}

async fn send_event(
    request: &AdapterStepRequest,
    events: &tokio::sync::mpsc::Sender<AdapterEvent>,
    kind: AdapterEventKind,
) -> Result<(), ProviderRuntimeError> {
    events
        .send(AdapterEvent {
            identity: request.identity.clone(),
            kind,
        })
        .await
        .map_err(|_| ProviderRuntimeError::Cancelled)
}

fn structured_result(
    request: &AdapterStepRequest,
    calls: &[DirectToolCall],
) -> Result<Option<Value>, ProviderRuntimeError> {
    match &request.kind {
        AdapterRequestKind::Foreground(_) => Ok(None),
        AdapterRequestKind::Structured { output_schema, .. } => {
            let [call] = calls else {
                return Err(ProviderRuntimeError::StructuredOutputInvalid);
            };
            if call.name != STRUCTURED_TOOL {
                return Err(ProviderRuntimeError::StructuredOutputInvalid);
            }
            validate_structured_output(output_schema, &call.arguments)
                .map_err(|_| ProviderRuntimeError::StructuredOutputInvalid)?;
            Ok(Some(call.arguments.clone()))
        }
    }
}
