use super::{ProviderRuntime, RuntimeEventSink};
use crate::{
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{BindingSnapshot, ForegroundRequest, ProviderAdapter, ProviderRuntimeError},
    provider_transcript::{ProviderTranscriptStore, RunCheckpointWriter, TranscriptBranch},
    tool_exec::SessionToolRuntime,
    workspace::{PreparedWorkspace, WorkspaceManager},
};
use std::{path::PathBuf, sync::Arc};

impl ProviderRuntime {
    pub fn new(
        adapter: Box<dyn ProviderAdapter>,
        binding: BindingSnapshot,
        dirs: crate::config::DataDirs,
        fallback_cwd: PathBuf,
        tools: SessionToolRuntime,
        cancellation: tokio::sync::watch::Receiver<u64>,
        events: Arc<dyn RuntimeEventSink>,
    ) -> Result<Self, ProviderRuntimeError> {
        if adapter.provider() != binding.provider
            || binding.conversation.provider() != binding.provider
        {
            return Err(ProviderRuntimeError::InvalidBinding(
                "provider binding conversation variant mismatch".to_string(),
            ));
        }
        Ok(Self {
            adapter,
            conversation: binding.conversation.clone(),
            binding,
            workspace: WorkspaceManager::new(dirs.clone()),
            dirs,
            fallback_cwd,
            active_workspace: None,
            tools,
            cancellation,
            events,
            pending_acknowledgements: Vec::new(),
        })
    }

    pub(super) async fn prepare_foreground_workspace(
        &mut self,
        request: &mut ForegroundRequest,
    ) -> Result<PreparedWorkspace, ProviderRuntimeError> {
        let workspace = self.workspace.clone();
        let session_id = request.identity.session_id.clone();
        let prepared =
            tokio::task::spawn_blocking(move || workspace.prepare_session_current(&session_id))
                .await
                .map_err(|error| ProviderRuntimeError::Transport(error.to_string()))?
                .map_err(ProviderRuntimeError::Transport)?;
        self.tools
            .bind_workspace_root(&request.identity.request_id, prepared.root.clone())
            .map_err(ProviderRuntimeError::Protocol)?;
        request.turn.workspace_root = Some(prepared.root.clone());
        self.active_workspace = Some(prepared.clone());
        Ok(prepared)
    }

    pub(super) fn direct_revision(&self) -> Result<u64, ProviderRuntimeError> {
        match self.conversation {
            ProviderConversationState::Antigravity {
                transcript_revision,
            }
            | ProviderConversationState::OpencodeGo {
                transcript_revision,
            }
            | ProviderConversationState::Ollama {
                transcript_revision,
            } => Ok(transcript_revision),
            ProviderConversationState::Codex { .. }
            | ProviderConversationState::ClaudeCode { .. } => Err(ProviderRuntimeError::Protocol(
                "native provider has no direct transcript".into(),
            )),
        }
    }

    pub(super) fn direct_conversation(&self, revision: u64) -> ProviderConversationState {
        match self.binding.provider {
            ProviderId::Antigravity => ProviderConversationState::Antigravity {
                transcript_revision: revision,
            },
            ProviderId::OpencodeGo => ProviderConversationState::OpencodeGo {
                transcript_revision: revision,
            },
            ProviderId::Ollama => ProviderConversationState::Ollama {
                transcript_revision: revision,
            },
            ProviderId::Codex => ProviderConversationState::Codex { thread_id: None },
            ProviderId::ClaudeCode => ProviderConversationState::ClaudeCode { session_id: None },
        }
    }

    pub(super) fn direct_writer(
        &self,
        request: &ForegroundRequest,
    ) -> Result<
        (
            Arc<RunCheckpointWriter>,
            Option<super::ProviderContinuation>,
        ),
        ProviderRuntimeError,
    > {
        let store = ProviderTranscriptStore::new(&self.dirs);
        let revision = self.direct_revision()?;
        let branch = TranscriptBranch {
            instruction_epoch: request.checkpoint.instruction_epoch,
            task_leaf_id: None,
        };
        let restored = store
            .restore(
                self.binding.provider,
                &request.identity.session_id,
                revision,
                &branch,
            )
            .map_err(ProviderRuntimeError::Protocol)?;
        let continuation = restored
            .as_ref()
            .and_then(|restored| restored.generation.checkpoint.continuation.clone());
        let (revision, blocks) = restored.map_or((0, Vec::new()), |restored| {
            (
                restored.generation.revision,
                restored.generation.checkpoint.blocks,
            )
        });
        let writer = store
            .checkpoint_writer(
                self.binding.provider,
                &request.identity.session_id,
                revision,
                branch,
                blocks,
            )
            .map(Arc::new)
            .map_err(ProviderRuntimeError::Protocol)?;
        Ok((writer, continuation))
    }
}
