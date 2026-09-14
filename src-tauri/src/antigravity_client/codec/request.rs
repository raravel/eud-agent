use super::metadata::{signed_session_id, stable_uuid};
use std::collections::HashSet;

use serde_json::{json, Value};

use crate::{
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{
        AdapterRequestKind, AdapterStepRequest, ConversationItem, NormalizedBlock,
        ProviderContinuation, ProviderRuntimeError,
    },
    provider_tool_loop::DirectToolResult,
};

use super::super::{normalize_cca_parameters, LiveAntigravityModel, STRUCTURED_TOOL};

pub(in crate::antigravity_client) async fn request_body(
    request: &AdapterStepRequest,
    model: &LiveAntigravityModel,
    project_id: &str,
) -> Result<Value, ProviderRuntimeError> {
    let schema = match &request.kind {
        AdapterRequestKind::Foreground(_) => None,
        AdapterRequestKind::Structured { output_schema, .. } => Some(output_schema),
    };
    let tools = schema.map_or_else(
        || function_declarations(&request.tool_descriptors),
        |schema| {
            vec![json!({
                "name":STRUCTURED_TOOL,
                "description":"Submit the final structured result.",
                "parameters":normalize_cca_parameters(schema)
            })]
        },
    );
    let history = gemini_history(request)?;
    let revision = match &request.binding.conversation {
        ProviderConversationState::Antigravity {
            transcript_revision,
        } => *transcript_revision,
        ProviderConversationState::Codex { .. }
        | ProviderConversationState::ClaudeCode { .. }
        | ProviderConversationState::OpencodeGo { .. }
        | ProviderConversationState::Ollama { .. } => {
            return Err(ProviderRuntimeError::Protocol(
                "incompatible conversation state".into(),
            ))
        }
    };
    let round = request
        .history
        .iter()
        .filter(|item| {
            matches!(
                item,
                ConversationItem::Assistant(NormalizedBlock::ToolCall { .. })
            )
        })
        .count();
    let step = revision
        .saturating_add(u64::try_from(round).unwrap_or(u64::MAX))
        .saturating_add(2);
    let session_id = &request.identity.session_id;
    let trajectory = stable_uuid(&format!("trajectory:{session_id}"));
    let agent = stable_uuid(&format!("agent:{session_id}"));
    let mut inner = json!({
        "contents":history,
        "sessionId":signed_session_id(session_id),
        "labels":{"last_step_index":step.saturating_sub(1).to_string(),"trajectory_id":trajectory}
    });
    if !tools.is_empty() {
        inner["tools"] = json!([{"functionDeclarations":tools}]);
        inner["toolConfig"] = if schema.is_some() {
            json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":[STRUCTURED_TOOL]}})
        } else {
            json!({"functionCallingConfig":{"mode":"VALIDATED"}})
        };
    }
    let output_limit = match (model.max_output_tokens, request.policy.max_output_tokens) {
        (Some(model), Some(policy)) => Some(model.min(policy)),
        (Some(model), None) => Some(model),
        (None, policy) => policy,
    };
    let mut generation = serde_json::Map::new();
    if let (true, Some(budget)) = (model.supports_thinking, model.thinking_budget) {
        generation.insert(
            "thinkingConfig".into(),
            json!({"includeThoughts":true,"thinkingBudget":budget}),
        );
    }
    if let Some(limit) = output_limit {
        generation.insert("maxOutputTokens".into(), Value::Number(limit.into()));
    }
    if !generation.is_empty() {
        inner["generationConfig"] = Value::Object(generation);
    }
    Ok(json!({
        "project":project_id,
        "requestId":format!("agent/{agent}/{}/{trajectory}/{step}",crate::session::now_unix_millis()),
        "request":inner,
        "model":model.id,
        "userAgent":"antigravity",
        "requestType":"agent"
    }))
}

fn gemini_history(request: &AdapterStepRequest) -> Result<Vec<Value>, ProviderRuntimeError> {
    let mut history = Vec::with_capacity(request.history.len() + request.prior_tool_results.len());
    let mut result_ids = HashSet::new();
    let mut active = None;
    let mut latest_batch = "";
    for item in request.history.iter() {
        match item {
            ConversationItem::User { text, images, .. } => {
                let mut parts = vec![json!({"text":text})];
                parts.extend(images.iter().map(|image| {
                    json!({
                        "inlineData":{"mimeType":image.mime,"data":image.data_base64}
                    })
                }));
                history.push(json!({"role":"user","parts":parts}));
                active = None;
            }
            ConversationItem::Assistant(NormalizedBlock::Text { response_id, text }) => {
                push_part(
                    &mut history,
                    &mut active,
                    "model",
                    response_id,
                    json!({"text":text}),
                );
            }
            ConversationItem::Assistant(NormalizedBlock::Reasoning {
                response_id,
                text,
                continuation,
            }) => {
                let mut part = json!({"text":text,"thought":true});
                if let Some(signature) = continuation.as_ref().and_then(thought_signature) {
                    part["thoughtSignature"] = Value::String(signature.to_string());
                }
                push_part(&mut history, &mut active, "model", response_id, part);
            }
            ConversationItem::Assistant(NormalizedBlock::ToolCall {
                response_id,
                batch_id,
                call,
                continuation,
            }) => {
                latest_batch = batch_id;
                let mut part =
                    json!({"functionCall":{"id":call.id,"name":call.name,"args":call.arguments}});
                if let Some(signature) = continuation.as_ref().and_then(thought_signature) {
                    part["thoughtSignature"] = Value::String(signature.to_string());
                }
                push_part(&mut history, &mut active, "model", response_id, part);
            }
            ConversationItem::Assistant(NormalizedBlock::ToolResult {
                batch_id, result, ..
            }) => {
                result_ids.insert(result.id.as_str());
                push_part(
                    &mut history,
                    &mut active,
                    "user",
                    batch_id,
                    tool_result_part(result),
                );
            }
            ConversationItem::Compaction { summary, .. } => {
                history.push(json!({"role":"user","parts":[{"text":summary}]}));
                active = None;
            }
        }
    }
    for result in request
        .prior_tool_results
        .iter()
        .filter(|result| !result_ids.contains(result.id.as_str()))
    {
        push_part(
            &mut history,
            &mut active,
            "user",
            latest_batch,
            tool_result_part(result),
        );
    }
    if history.is_empty() {
        if let AdapterRequestKind::Structured { prompt, .. } = &request.kind {
            history.push(json!({"role":"user","parts":[{"text":prompt}]}));
        }
    }
    Ok(history)
}

fn push_part<'a>(
    history: &mut Vec<Value>,
    active: &mut Option<(&'static str, &'a str)>,
    role: &'static str,
    identity: &'a str,
    part: Value,
) {
    if *active == Some((role, identity)) {
        if let Some(parts) = history
            .last_mut()
            .and_then(|item| item["parts"].as_array_mut())
        {
            parts.push(part);
            return;
        }
    }
    history.push(json!({"role":role,"parts":[part]}));
    *active = Some((role, identity));
}

fn tool_result_part(result: &DirectToolResult) -> Value {
    let response = if result.is_error {
        json!({"error":result.result})
    } else {
        json!({"output":result.result})
    };
    json!({"functionResponse":{
        "id":result.id,"name":result.name,"response":response
    }})
}
fn thought_signature(continuation: &ProviderContinuation) -> Option<&str> {
    (continuation.provider == ProviderId::Antigravity)
        .then(|| {
            continuation
                .data
                .get("thoughtSignature")
                .and_then(Value::as_str)
        })
        .flatten()
}

fn function_declarations(descriptors: &[Value]) -> Vec<Value> {
    descriptors
        .iter()
        .filter_map(|descriptor| {
            Some(json!({
                "name":descriptor.get("name")?.as_str()?,
                "description":descriptor.get("description").and_then(Value::as_str).unwrap_or(""),
                "parameters":descriptor.get("inputSchema").map(normalize_cca_parameters)
                    .unwrap_or_else(||json!({"type":"object","properties":{}}))
            }))
        })
        .collect()
}
