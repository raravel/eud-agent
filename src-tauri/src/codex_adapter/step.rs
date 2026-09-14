use super::turn::{execute_native_turn, transport_error};
use super::{ClientKey, CodexAdapter, NativeOperationGuard};
use crate::{
    codex_client::{CodexAppServerClient, CodexModelSelection},
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{
        AdapterEvent, AdapterFuture, AdapterRequestKind, AdapterStepOutcome, AdapterStepRequest,
        ProviderRuntimeError, WorkspaceAccess,
    },
};
impl CodexAdapter {
    pub(super) fn execute_step<'a>(
        &'a mut self,
        request: AdapterStepRequest,
        event_tx: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<AdapterStepOutcome, ProviderRuntimeError>> {
        Box::pin(async move {
            if request.binding.provider != ProviderId::Codex {
                return Err(ProviderRuntimeError::InvalidBinding(
                    "Codex adapter received a non-Codex binding".to_string(),
                ));
            }
            let model = CodexModelSelection {
                model: request.binding.model.clone(),
                reasoning_effort: request
                    .binding
                    .reasoning
                    .as_ref()
                    .map(|value| value.level.clone())
                    .unwrap_or_else(|| "medium".to_string()),
            };
            let large_context = self
                .launch
                .large_context_models
                .contains(&request.binding.model);

            match request.kind.clone() {
                AdapterRequestKind::Foreground(turn) => {
                    let key = ClientKey {
                        cwd: turn
                            .workspace_root
                            .clone()
                            .unwrap_or_else(|| self.fallback_cwd.clone()),
                        access: turn.workspace_access,
                        mcp_endpoint: request.native_mcp_endpoint.clone(),
                        native_tools_enabled: true,
                    };
                    self.ensure_foreground_client(key.clone(), model, large_context)
                        .await?;
                    self.native_state_trusted = false;
                    let result = {
                        let mut guard = NativeOperationGuard {
                            client: &mut self.client,
                            events: &mut self.events,
                            completed: false,
                        };
                        let client = guard.client.as_mut().ok_or_else(|| {
                            ProviderRuntimeError::Transport(
                                "Codex app-server client is unavailable".to_string(),
                            )
                        })?;
                        client
                            .ensure_workspace_sandbox(&key.cwd)
                            .await
                            .map_err(transport_error)?;
                        let events = guard.events.as_mut().ok_or_else(|| {
                            ProviderRuntimeError::Transport(
                                "Codex app-server event stream is unavailable".to_string(),
                            )
                        })?;
                        let result =
                            execute_native_turn(client, events, turn, &request, &event_tx).await;
                        guard.completed = result.is_ok();
                        result
                    };
                    match result {
                        Ok((output, thread_id)) => {
                            self.committed_thread_id = thread_id.clone();
                            self.native_state_trusted = true;
                            Ok(AdapterStepOutcome::Completed {
                                output,
                                continuation: None,
                                native_conversation: Some(ProviderConversationState::Codex {
                                    thread_id,
                                }),
                            })
                        }
                        Err(ProviderRuntimeError::Cancelled) => {
                            self.invalidate_native_state();
                            Ok(AdapterStepOutcome::Cancelled)
                        }
                        Err(error) => {
                            self.invalidate_native_state();
                            Err(error)
                        }
                    }
                }
                AdapterRequestKind::Structured {
                    prompt,
                    workspace_root,
                    output_schema,
                    ..
                } => {
                    let (mut client, mut events) = CodexAppServerClient::spawn_app_server(
                        &workspace_root,
                        &self.launch,
                        None,
                        WorkspaceAccess::Read,
                        false,
                    )
                    .await
                    .map_err(transport_error)?;
                    client.set_model_selection(Some(model));
                    client.set_large_context_enabled(large_context);
                    client
                        .ensure_workspace_sandbox(&workspace_root)
                        .await
                        .map_err(transport_error)?;
                    let mut turn = crate::provider_runtime::AgentTurnInput::text(prompt)
                        .with_output_schema(output_schema)
                        .without_tools();
                    turn.workspace_root = Some(workspace_root);
                    let (output, _) = match execute_native_turn(
                        &mut client,
                        &mut events,
                        turn,
                        &request,
                        &event_tx,
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(ProviderRuntimeError::Cancelled) => {
                            return Ok(AdapterStepOutcome::Cancelled)
                        }
                        Err(error) => return Err(error),
                    };
                    Ok(AdapterStepOutcome::Completed {
                        output,
                        continuation: None,
                        native_conversation: None,
                    })
                }
            }
        })
    }
}
