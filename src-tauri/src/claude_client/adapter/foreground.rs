use serde_json::json;

use crate::provider::{ProviderConversationState, ProviderId};
use crate::provider_runtime::{
    AdapterEvent, AdapterRequestKind, AdapterStepOutcome, AdapterStepRequest, ProviderRuntimeError,
};

use super::process::StreamProcessRequest;
use super::request::{
    claude_user_message, stream_args, validate_workspace_boundary, MAX_STDOUT_BYTES,
};
use super::state::ProductionClaudeCodeAdapter;

pub(super) enum NativeRunResult {
    Completed { session_id: String, output: String },
    Cancelled,
}

impl ProductionClaudeCodeAdapter {
    pub(super) async fn run_foreground(
        &mut self,
        request: AdapterStepRequest,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> Result<AdapterStepOutcome, ProviderRuntimeError> {
        self.prepared_compaction = None;
        let AdapterRequestKind::Foreground(turn) = &request.kind else {
            return Err(ProviderRuntimeError::Protocol(
                "provider_protocol_changed".to_string(),
            ));
        };
        if request.binding.model != self.model || request.binding.provider != ProviderId::ClaudeCode
        {
            return Err(ProviderRuntimeError::Protocol(
                "provider_model_unavailable".to_string(),
            ));
        }
        if !request.prior_tool_results.is_empty() || !request.tool_descriptors.is_empty() {
            return Err(ProviderRuntimeError::Protocol(
                "native provider owns its internal tool loop".to_string(),
            ));
        }
        let workspace_root = turn.workspace_root.as_deref().ok_or_else(|| {
            ProviderRuntimeError::Protocol("provider workspace is unavailable".to_string())
        })?;
        validate_workspace_boundary(workspace_root).map_err(ProviderRuntimeError::Protocol)?;
        let resume = match &request.binding.conversation {
            ProviderConversationState::ClaudeCode { session_id } => session_id.as_deref(),
            _ => {
                return Err(ProviderRuntimeError::Protocol(
                    "provider conversation state is incompatible".to_string(),
                ));
            }
        };
        if self.continuation_unknown && resume.is_some() {
            return Err(ProviderRuntimeError::Protocol(
                "provider native continuation is unknown".to_string(),
            ));
        }
        let endpoint = request.native_mcp_endpoint.as_deref().ok_or_else(|| {
            ProviderRuntimeError::Protocol("provider eud-tools MCP is unavailable".to_string())
        })?;
        let mcp_config = json!({
            "mcpServers": {
                "eud-tools": {"type": "http", "url": endpoint}
            }
        })
        .to_string();
        let message = claude_user_message(turn).map_err(ProviderRuntimeError::Protocol)?;
        let args = stream_args(
            Some(&mcp_config),
            resume,
            false,
            &self.model,
            request
                .binding
                .reasoning
                .as_ref()
                .map(|selection| selection.level.as_str()),
        )
        .map_err(ProviderRuntimeError::Protocol)?;
        self.last_cwd = Some(workspace_root.to_path_buf());
        self.continuation_unknown = true;
        let result = self
            .run_stream_process(StreamProcessRequest {
                identity: &request.identity,
                cwd: workspace_root,
                args,
                message,
                require_mcp: true,
                max_output_bytes: MAX_STDOUT_BYTES,
                cancellation: request.cancellation,
                events: Some(events),
            })
            .await;
        if let Err(error) = ensure_expected_session(&result, resume) {
            return self.finish_native_result(Err(error));
        }
        self.finish_native_result(result)
    }

    pub(super) fn finish_native_result(
        &mut self,
        result: Result<NativeRunResult, ProviderRuntimeError>,
    ) -> Result<AdapterStepOutcome, ProviderRuntimeError> {
        match result {
            Ok(NativeRunResult::Completed { session_id, output }) => {
                self.conversation_id = Some(session_id.clone());
                self.continuation_unknown = false;
                Ok(AdapterStepOutcome::Completed {
                    output: crate::provider_runtime::AdapterOutput::Text(output),
                    continuation: None,
                    native_conversation: Some(ProviderConversationState::ClaudeCode {
                        session_id: Some(session_id),
                    }),
                })
            }
            Ok(NativeRunResult::Cancelled) => {
                self.conversation_id = None;
                self.continuation_unknown = true;
                Ok(AdapterStepOutcome::Cancelled)
            }
            Err(error) => {
                self.conversation_id = None;
                self.continuation_unknown = true;
                Err(error)
            }
        }
    }
}

pub(super) fn ensure_expected_session(
    result: &Result<NativeRunResult, ProviderRuntimeError>,
    expected: Option<&str>,
) -> Result<(), ProviderRuntimeError> {
    if let (Some(expected), Ok(NativeRunResult::Completed { session_id, .. })) = (expected, result)
    {
        if expected != session_id.as_str() {
            return Err(ProviderRuntimeError::Protocol(
                "provider_protocol_changed".to_string(),
            ));
        }
    }
    Ok(())
}
