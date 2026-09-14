use super::turn::{emit, normalize_usage, transport_error};
use super::{ClientKey, CodexAdapter, NativeOperationGuard};
use crate::{
    codex_client::{AppServerEvent, CodexModelSelection},
    provider::ProviderConversationState,
    provider_runtime::{
        AdapterEvent, AdapterFuture, ProviderRuntimeError, RunIdentity, WorkspaceAccess,
    },
};
impl CodexAdapter {
    pub(super) fn compact_native<'a>(
        &'a mut self,
        identity: RunIdentity,
        mut cancellation: tokio::sync::watch::Receiver<u64>,
        event_tx: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<ProviderConversationState, ProviderRuntimeError>> {
        Box::pin(async move {
            if *cancellation.borrow_and_update() != identity.cancellation_generation {
                self.invalidate_native_state();
                return Err(ProviderRuntimeError::Cancelled);
            }
            self.native_state_trusted = false;
            let mut guard = NativeOperationGuard {
                client: &mut self.client,
                events: &mut self.events,
                completed: false,
            };
            let compacted = async {
                let client = guard.client.as_mut().ok_or_else(|| {
                    ProviderRuntimeError::Protocol("no Codex conversation to compact".to_string())
                })?;
                client.start_compaction().await.map_err(transport_error)?;
                let native_events = guard.events.as_mut().ok_or_else(|| {
                    ProviderRuntimeError::Transport(
                        "Codex app-server event stream is unavailable".to_string(),
                    )
                })?;
                let request_id = identity.request_id.clone();
                loop {
                    let event = tokio::select! {
                        changed = cancellation.changed() => {
                            if changed.is_err() || *cancellation.borrow_and_update() != identity.cancellation_generation {
                                return Err(ProviderRuntimeError::Cancelled);
                            }
                            continue;
                        }
                        event = native_events.recv() => event,
                    };
                    let Some(event) = event else {
                        return Err(ProviderRuntimeError::Transport(
                            "Codex app-server event stream closed during compaction".to_string(),
                        ));
                    };
                    match event {
                        AppServerEvent::ContextCompactionStarted => {
                            emit(
                                &event_tx,
                                &identity,
                                crate::provider_runtime::AdapterEventKind::ResponseStarted {
                                    response_id: request_id.clone(),
                                },
                            )
                            .await?;
                        }
                        AppServerEvent::ContextCompactionCompleted => {
                            emit(
                                &event_tx,
                                &identity,
                                crate::provider_runtime::AdapterEventKind::ResponseFinished {
                                    response_id: request_id,
                                    finish_reason: Some("compacted".to_string()),
                                    complete: true,
                                },
                            )
                            .await?;
                            return Ok(());
                        }
                        AppServerEvent::TokenUsageUpdated { token_usage, .. } => {
                            emit(
                                &event_tx,
                                &identity,
                                crate::provider_runtime::AdapterEventKind::Usage(normalize_usage(
                                    &token_usage,
                                )),
                            )
                            .await?;
                        }
                        AppServerEvent::Error(message) => {
                            return Err(ProviderRuntimeError::Transport(message));
                        }
                        _ => {}
                    }
                }
            }
            .await;
            guard.completed = compacted.is_ok();
            drop(guard);
            if let Err(error) = compacted {
                self.invalidate_native_state();
                return Err(error);
            }
            self.native_state_trusted = true;
            Ok(ProviderConversationState::Codex {
                thread_id: self.committed_thread_id.clone(),
            })
        })
    }

    pub(super) fn prepare_native_compaction<'a>(
        &'a mut self,
        request: &'a crate::provider_runtime::CompactionRequest,
    ) -> AdapterFuture<'a, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            if !request.workspace_root.is_dir() {
                return Err(ProviderRuntimeError::Protocol(
                    "provider workspace is unavailable".into(),
                ));
            }
            if !self.native_state_trusted || self.committed_thread_id.is_none() {
                return Err(ProviderRuntimeError::Protocol(
                    "no confirmed Codex conversation to compact".into(),
                ));
            }
            let key = ClientKey {
                cwd: request.workspace_root.clone(),
                access: WorkspaceAccess::Read,
                mcp_endpoint: None,
                native_tools_enabled: false,
            };
            let model = CodexModelSelection {
                model: request.binding.model.clone(),
                reasoning_effort: request
                    .binding
                    .reasoning
                    .as_ref()
                    .map(|value| value.level.clone())
                    .unwrap_or_else(|| "medium".into()),
            };
            let large_context = self
                .launch
                .large_context_models
                .contains(&request.binding.model);
            self.ensure_foreground_client(key, model, large_context)
                .await?;
            let mut guard = NativeOperationGuard {
                client: &mut self.client,
                events: &mut self.events,
                completed: false,
            };
            let client = guard.client.as_mut().ok_or_else(|| {
                ProviderRuntimeError::Transport("Codex app-server client is unavailable".into())
            })?;
            client
                .ensure_workspace_sandbox(&request.workspace_root)
                .await
                .map_err(transport_error)?;
            client
                .resume_for_compaction(&request.workspace_root)
                .await
                .map_err(transport_error)?;
            guard.completed = true;
            Ok(())
        })
    }
}
