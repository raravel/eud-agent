use std::path::PathBuf;

use serde_json::{json, Value};

use crate::provider::{ProviderConversationState, ProviderId};
use crate::provider_runtime::{
    AdapterEvent, AdapterFuture, AdapterLoopKind, AdapterRequestKind, AdapterStepOutcome,
    AdapterStepRequest, ProviderAdapter, ProviderRuntimeError, RunIdentity,
};

use super::foreground::ensure_expected_session;
use super::process::StreamProcessRequest;
use super::request::{stream_args, validate_workspace_boundary, MAX_STDOUT_BYTES};

pub(super) const CLAUDE_PROVIDER_DEFAULT: &str = "provider-default";

pub(super) struct PreparedClaudeProcess {
    pub(super) child: tokio::process::Child,
    pub(super) job: crate::provider_process::WindowsJob,
}

pub struct ProductionClaudeCodeAdapter {
    pub(super) model: String,
    pub(super) executable: PathBuf,
    pub(super) executable_prefix_args: Vec<String>,
    pub(super) profile_dir: PathBuf,
    pub(super) conversation_id: Option<String>,
    pub(super) last_cwd: Option<PathBuf>,
    pub(super) continuation_unknown: bool,
    pub(super) prepared_compaction: Option<PreparedClaudeProcess>,
}

impl ProductionClaudeCodeAdapter {
    pub(crate) fn new(
        model: String,
        executable: PathBuf,
        profile_dir: PathBuf,
    ) -> Result<Self, ProviderRuntimeError> {
        if model != CLAUDE_PROVIDER_DEFAULT {
            return Err(ProviderRuntimeError::Protocol(
                "provider_model_unavailable".to_string(),
            ));
        }
        Ok(Self {
            model,
            executable,
            executable_prefix_args: Vec::new(),
            profile_dir,
            conversation_id: None,
            last_cwd: None,
            continuation_unknown: false,
            prepared_compaction: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_prefix_args(mut self, args: Vec<String>) -> Self {
        self.executable_prefix_args = args;
        self
    }
}

impl ProviderAdapter for ProductionClaudeCodeAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::ClaudeCode
    }

    fn loop_kind(&self) -> AdapterLoopKind {
        AdapterLoopKind::NativeSession
    }

    fn run_step<'a>(
        &'a mut self,
        request: AdapterStepRequest,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<AdapterStepOutcome, ProviderRuntimeError>> {
        Box::pin(async move {
            if matches!(&request.kind, AdapterRequestKind::Foreground(_)) {
                self.run_foreground(request, events).await
            } else {
                self.run_structured(request, events).await
            }
        })
    }

    fn interrupt(
        &mut self,
        _identity: &RunIdentity,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            self.prepared_compaction = None;
            Ok(())
        })
    }

    fn reset(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            self.conversation_id = None;
            self.continuation_unknown = false;
            self.prepared_compaction = None;
            Ok(())
        })
    }

    fn compact<'a>(
        &'a mut self,
        identity: RunIdentity,
        cancellation: tokio::sync::watch::Receiver<u64>,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<ProviderConversationState, ProviderRuntimeError>> {
        Box::pin(async move {
            let session_id = self.conversation_id.clone().ok_or_else(|| {
                ProviderRuntimeError::Protocol("provider conversation is empty".to_string())
            })?;
            if self.continuation_unknown {
                return Err(ProviderRuntimeError::Protocol(
                    "provider native continuation is unknown".to_string(),
                ));
            }
            let cwd = self.last_cwd.clone().ok_or_else(|| {
                ProviderRuntimeError::Protocol("provider workspace is unavailable".to_string())
            })?;
            self.continuation_unknown = true;
            let result = self
                .run_stream_process(StreamProcessRequest {
                    identity: &identity,
                    cwd: &cwd,
                    args: stream_args(None, Some(&session_id), true),
                    message: json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"/compact"}]},"parent_tool_use_id":Value::Null}),
                    require_mcp: false,
                    max_output_bytes: MAX_STDOUT_BYTES,
                    cancellation,
                    events: Some(events),
                })
                .await;
            if let Err(error) = ensure_expected_session(&result, Some(&session_id)) {
                let _ = self.finish_native_result(Err(error.clone()));
                return Err(error);
            }
            match self.finish_native_result(result)? {
                AdapterStepOutcome::Completed {
                    native_conversation: Some(state),
                    ..
                } => Ok(state),
                AdapterStepOutcome::Completed {
                    native_conversation: None,
                    ..
                }
                | AdapterStepOutcome::NeedsTools { .. } => Err(ProviderRuntimeError::Protocol(
                    "provider_protocol_changed".to_string(),
                )),
                AdapterStepOutcome::Cancelled => Err(ProviderRuntimeError::Cancelled),
            }
        })
    }

    fn seed(
        &mut self,
        state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            let ProviderConversationState::ClaudeCode { session_id } = state else {
                return Err(ProviderRuntimeError::Protocol(
                    "provider conversation state is incompatible".to_string(),
                ));
            };
            self.conversation_id = session_id;
            self.continuation_unknown = false;
            self.prepared_compaction = None;
            Ok(())
        })
    }

    fn prepare_compaction<'a>(
        &'a mut self,
        request: &'a crate::provider_runtime::CompactionRequest,
    ) -> AdapterFuture<'a, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            self.prepared_compaction = None;
            if self.continuation_unknown {
                return Err(ProviderRuntimeError::Protocol(
                    "provider native continuation is unknown".into(),
                ));
            }
            let session_id = self.conversation_id.as_deref().ok_or_else(|| {
                ProviderRuntimeError::Protocol("provider conversation is empty".into())
            })?;
            if !request.workspace_root.is_dir() {
                return Err(ProviderRuntimeError::Protocol(
                    "provider workspace is unavailable".into(),
                ));
            }
            validate_workspace_boundary(&request.workspace_root)
                .map_err(ProviderRuntimeError::Protocol)?;
            let prepared = self.spawn_stream_process(
                &request.workspace_root,
                stream_args(None, Some(session_id), true),
            )?;
            self.last_cwd = Some(request.workspace_root.clone());
            self.prepared_compaction = Some(prepared);
            Ok(())
        })
    }
}
