use super::{EngineEvent, EventSink};
use crate::{
    ipc,
    provider_runtime::{
        AdapterEventKind, NormalizedBlock, NormalizedUsage, ProviderRuntimeError, RuntimeEventSink,
    },
    session::SessionStore,
};

pub(crate) struct SessionRuntimeEventSink<S> {
    sink: S,
    sessions: SessionStore,
    session_id: String,
    response_id: parking_lot::Mutex<Option<String>>,
    /// Set for a `delegate_read` child: its tool events carry this run id so
    /// the panel nests them, and its usage never replaces the session's `last`.
    delegation_run_id: Option<u64>,
}

impl<S: EventSink> SessionRuntimeEventSink<S> {
    pub(crate) fn new(sink: S, sessions: SessionStore, session_id: String) -> Self {
        Self {
            sink,
            sessions,
            session_id,
            response_id: parking_lot::Mutex::new(None),
            delegation_run_id: None,
        }
    }

    pub(crate) fn delegated(
        sink: S,
        sessions: SessionStore,
        session_id: String,
        child_run_id: u64,
    ) -> Self {
        Self {
            delegation_run_id: Some(child_run_id),
            ..Self::new(sink, sessions, session_id)
        }
    }

    fn emit_usage(&self, usage: &NormalizedUsage) -> Result<(), ProviderRuntimeError> {
        if self.delegation_run_id.is_some() {
            return Ok(());
        }
        let Some(token_usage) = &usage.context_usage else {
            return Ok(());
        };
        let turn_id = self.response_id.lock().clone().ok_or_else(|| {
            ProviderRuntimeError::Protocol("context usage has no active response".to_string())
        })?;
        if let Err(error) = self
            .sessions
            .update_context_usage(&self.session_id, token_usage.clone())
        {
            eprintln!(
                "eud-agent: context usage persistence failed: session={} error={error}",
                self.session_id
            );
        }
        self.sink
            .emit(EngineEvent::ContextUsage(ipc::ContextUsageEvent {
                turn_id,
                token_usage: token_usage.clone(),
            }))
            .map_err(|error| ProviderRuntimeError::Transport(error.to_string()))
    }
}

impl<S: EventSink + Send + Sync> RuntimeEventSink for SessionRuntimeEventSink<S> {
    fn emit(&self, event: &AdapterEventKind) -> Result<(), ProviderRuntimeError> {
        let agent = match event {
            AdapterEventKind::ResponseStarted { response_id } => {
                *self.response_id.lock() = Some(response_id.clone());
                ipc::AgentEvent {
                    kind: "response_started".to_string(),
                    detail: response_id.clone(),
                    data: None,
                }
            }
            AdapterEventKind::Block(NormalizedBlock::Text { text, .. }) => ipc::AgentEvent {
                kind: "delta".to_string(),
                detail: text.clone(),
                data: None,
            },
            AdapterEventKind::Block(NormalizedBlock::Reasoning { text, .. }) => ipc::AgentEvent {
                kind: "reasoning".to_string(),
                detail: text.clone(),
                data: None,
            },
            AdapterEventKind::Block(NormalizedBlock::ToolCall { call, .. }) => ipc::AgentEvent {
                kind: "tool_call".to_string(),
                detail: call.name.clone(),
                data: Some(ipc::AgentEventData {
                    call_id: Some(call.id.clone()),
                    args: Some(call.arguments.to_string()),
                    result: None,
                    status: None,
                    delegation_run_id: self.delegation_run_id,
                }),
            },
            AdapterEventKind::Block(NormalizedBlock::ToolResult { result, .. }) => {
                ipc::AgentEvent {
                    kind: "tool_result".to_string(),
                    detail: result.name.clone(),
                    data: Some(ipc::AgentEventData {
                        call_id: Some(result.id.clone()),
                        args: None,
                        result: Some(result.result.to_string()),
                        status: Some(
                            if result.is_error {
                                "failed"
                            } else {
                                "completed"
                            }
                            .to_string(),
                        ),
                        delegation_run_id: self.delegation_run_id,
                    }),
                }
            }
            AdapterEventKind::NativeToolObservation {
                call_id,
                name,
                arguments,
                result,
                status,
                ..
            } => {
                let kind = match status.as_deref() {
                    Some("started") => "tool_call",
                    Some("completed" | "failed" | "declined") => "tool_result",
                    Some(_) | None => {
                        return Err(ProviderRuntimeError::Protocol(
                            "native tool observation has an invalid lifecycle status".to_string(),
                        ))
                    }
                };
                ipc::AgentEvent {
                    kind: kind.to_string(),
                    detail: name.clone(),
                    data: Some(ipc::AgentEventData {
                        call_id: call_id.clone(),
                        args: arguments.as_ref().map(serde_json::Value::to_string),
                        result: result.as_ref().map(serde_json::Value::to_string),
                        status: if kind == "tool_result" {
                            status.clone()
                        } else {
                            None
                        },
                        delegation_run_id: self.delegation_run_id,
                    }),
                }
            }
            AdapterEventKind::Usage(usage) => return self.emit_usage(usage),
            AdapterEventKind::ResponseFinished { response_id, .. } => {
                self.response_id.lock().take();
                ipc::AgentEvent {
                    kind: "response_finished".to_string(),
                    detail: response_id.clone(),
                    data: None,
                }
            }
            AdapterEventKind::TransportClosed => {
                self.response_id.lock().take();
                ipc::AgentEvent {
                    kind: "transport_closed".to_string(),
                    detail: String::new(),
                    data: None,
                }
            }
        };
        self.sink
            .emit(EngineEvent::Agent(agent))
            .map_err(|error| ProviderRuntimeError::Transport(error.to_string()))
    }
}
