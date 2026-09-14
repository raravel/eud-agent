use crate::{
    codex_client::{AppServerEvent, CodexAppServerClient},
    provider_runtime::{
        AdapterEvent, AdapterEventKind, AdapterOutput, AdapterRequestKind, AdapterStepRequest,
        NormalizedBlock, NormalizedUsage, ProviderRuntimeError,
    },
};

pub(super) async fn execute_native_turn<R, W>(
    client: &mut CodexAppServerClient<R, W>,
    events: &mut tokio::sync::mpsc::Receiver<AppServerEvent>,
    turn: crate::provider_runtime::AgentTurnInput,
    request: &AdapterStepRequest,
    event_tx: &tokio::sync::mpsc::Sender<AdapterEvent>,
) -> Result<(AdapterOutput, Option<String>), ProviderRuntimeError>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let structured = matches!(request.kind, AdapterRequestKind::Structured { .. });
    let response_id = request.identity.request_id.clone();
    let answer = {
        let run = client.run_turn_cancellable(
            turn,
            request.cancellation.clone(),
            request.identity.cancellation_generation,
        );
        tokio::pin!(run);
        let mut answer = String::new();
        let mut run_result = None;
        let mut completed = false;
        let mut active_native_turn_id = None;
        while run_result.is_none() || !completed {
            tokio::select! {
            result = &mut run, if run_result.is_none() => {
                let interrupted = result.map_err(transport_error)?;
                if interrupted {
                    return Err(ProviderRuntimeError::Cancelled);
                }
                run_result = Some(false);
            }
            event = events.recv(), if !completed => {
                let event = event.ok_or_else(|| ProviderRuntimeError::Transport(
                    "Codex app-server event stream closed".to_string()
                ))?;
                match event {
                    AppServerEvent::TurnStarted { turn_id } => {
                        active_native_turn_id = Some(turn_id);
                        emit(event_tx, &request.identity, AdapterEventKind::ResponseStarted { response_id: response_id.clone() }).await?;
                    }
                    AppServerEvent::AnswerDelta(text) => {
                        answer.push_str(&text);
                        emit(event_tx, &request.identity, AdapterEventKind::Block(NormalizedBlock::Text { response_id: response_id.clone(), text })).await?;
                    }
                    AppServerEvent::ReasoningDelta(text) => emit(event_tx, &request.identity, AdapterEventKind::Block(NormalizedBlock::Reasoning { response_id: response_id.clone(), text, continuation: None })).await?,
                    AppServerEvent::ToolCallStarted { name, .. } if structured => return Err(ProviderRuntimeError::Protocol(format!("structured Codex job attempted forbidden native tool `{name}`"))),
                    AppServerEvent::ToolCallStarted { item_id, mcp_server, name, args } => emit(event_tx, &request.identity, AdapterEventKind::NativeToolObservation {
                        call_id: item_id,
                        mcp_server,
                        name,
                        arguments: args.map(observation_value),
                        result: None,
                        status: Some("started".to_string()),
                    }).await?,
                    AppServerEvent::ToolCallCompleted { name, .. } if structured => return Err(ProviderRuntimeError::Protocol(format!("structured Codex job completed forbidden native tool `{name}`"))),
                    AppServerEvent::ToolCallCompleted { item_id, mcp_server, name, result, status } => emit(event_tx, &request.identity, AdapterEventKind::NativeToolObservation {
                        call_id: item_id,
                        mcp_server,
                        name,
                        arguments: None,
                        result: result.map(observation_value),
                        status: Some(completed_tool_status(status.as_deref())?),
                    }).await?,
                    AppServerEvent::TokenUsageUpdated { turn_id, token_usage }
                        if active_native_turn_id.as_deref() == Some(turn_id.as_str()) =>
                    {
                        emit(event_tx, &request.identity, AdapterEventKind::Usage(normalize_usage(&token_usage))).await?;
                    }
                    AppServerEvent::TokenUsageUpdated { .. } => {}
                    AppServerEvent::TurnComplete => completed = true,
                    AppServerEvent::Error(message) => return Err(ProviderRuntimeError::Transport(message)),
                    AppServerEvent::ThreadStarted { .. }
                    | AppServerEvent::ItemStarted { .. }
                    | AppServerEvent::ItemCompleted { .. }
                    | AppServerEvent::ContextCompactionStarted
                    | AppServerEvent::ContextCompactionCompleted => {}
                }
            }
            }
        }
        answer
    };
    emit(
        event_tx,
        &request.identity,
        AdapterEventKind::ResponseFinished {
            response_id,
            finish_reason: Some("completed".to_string()),
            complete: true,
        },
    )
    .await?;
    let output = if structured {
        AdapterOutput::Structured(
            serde_json::from_str(&answer)
                .map_err(|_| ProviderRuntimeError::StructuredOutputInvalid)?,
        )
    } else {
        AdapterOutput::Text(answer)
    };
    Ok((output, client.current_thread_id().await))
}

pub(super) async fn emit(
    event_tx: &tokio::sync::mpsc::Sender<AdapterEvent>,
    identity: &crate::provider_runtime::RunIdentity,
    kind: AdapterEventKind,
) -> Result<(), ProviderRuntimeError> {
    event_tx
        .send(AdapterEvent {
            identity: identity.clone(),
            kind,
        })
        .await
        .map_err(|_| ProviderRuntimeError::Transport("provider event receiver closed".to_string()))
}

pub(super) fn normalize_usage(usage: &crate::ipc::ContextUsage) -> NormalizedUsage {
    NormalizedUsage {
        input_tokens: u64::try_from(usage.last.input_tokens).ok(),
        cached_input_tokens: u64::try_from(usage.last.cached_input_tokens).ok(),
        output_tokens: u64::try_from(usage.last.output_tokens).ok(),
        total_tokens: u64::try_from(usage.last.total_tokens).ok(),
        context_usage: Some(usage.clone()),
        provider_details: serde_json::to_value(usage).ok(),
    }
}

pub(super) fn transport_error(error: impl std::fmt::Display) -> ProviderRuntimeError {
    ProviderRuntimeError::Transport(error.to_string())
}

fn completed_tool_status(status: Option<&str>) -> Result<String, ProviderRuntimeError> {
    match status {
        None | Some("completed") => Ok("completed".into()),
        Some("failed" | "cancelled" | "interrupted" | "error") => Ok("failed".into()),
        Some("declined") => Ok("declined".into()),
        Some(_) => Err(ProviderRuntimeError::Protocol(
            "Codex tool completed with an unknown status".into(),
        )),
    }
}

fn observation_value(text: String) -> serde_json::Value {
    serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text))
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
