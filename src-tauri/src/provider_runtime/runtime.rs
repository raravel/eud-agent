use std::{path::PathBuf, sync::Arc};

use crate::{
    provider::ProviderConversationState,
    provider_transcript::ProviderTranscriptStore,
    tool_exec::SessionToolRuntime,
    workspace::{PreparedWorkspace, WorkspaceManager},
};

use super::{
    AdapterEventKind, AdapterLoopKind, BindingSnapshot, CompactionRequest,
    ForegroundRequest, ProviderAdapter, ProviderRuntimeError, RunOutcome,
    RuntimeExecutor, StructuredJobRequest,
};

mod compaction;
mod events;
mod foreground;
mod native_compaction;
mod native_recovery;
mod shutdown;
mod state;
mod step;
mod structured;
mod tool_batch;
mod tool_events;

use super::{AdapterFuture, ProviderContinuation, RunIdentity, WorkspaceAccess};
pub use structured::StructuredJobExecutor;

pub trait RuntimeEventSink: Send + Sync {
    fn emit(&self, event: &AdapterEventKind) -> Result<(), ProviderRuntimeError>;
}

pub struct ProviderRuntime {
    adapter: Box<dyn ProviderAdapter>,
    binding: BindingSnapshot,
    conversation: ProviderConversationState,
    dirs: crate::config::DataDirs,
    fallback_cwd: PathBuf,
    workspace: WorkspaceManager,
    active_workspace: Option<PreparedWorkspace>,
    tools: SessionToolRuntime,
    cancellation: tokio::sync::watch::Receiver<u64>,
    events: Arc<dyn RuntimeEventSink>,
    pending_acknowledgements: Vec<super::RunIdentity>,
}

impl RuntimeExecutor for ProviderRuntime {
    fn run_foreground(
        &mut self,
        request: ForegroundRequest,
    ) -> super::AdapterFuture<'_, RunOutcome> {
        self.foreground(request)
    }

    fn compact(
        &mut self,
        request: CompactionRequest,
    ) -> super::AdapterFuture<'_, Result<ProviderConversationState, ProviderRuntimeError>> {
        self.compact_conversation(request)
    }
    fn run_structured(
        &mut self,
        request: StructuredJobRequest,
    ) -> super::AdapterFuture<'_, RunOutcome> {
        Box::pin(async move {
            let adapter = match super::production_adapter(
                &request.binding.to_binding(),
                &self.dirs,
                self.fallback_cwd.clone(),
            ) {
                Ok(adapter) => adapter,
                Err(error) => return RunOutcome::Failed(error),
            };
            StructuredJobExecutor::new(adapter, self.cancellation.clone())
                .run(request)
                .await
        })
    }

    fn reset(&mut self) -> super::AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            self.adapter.reset().await?;
            if self.adapter.loop_kind() == AdapterLoopKind::DirectSteps {
                ProviderTranscriptStore::new(&self.dirs)
                    .clear_current(self.tools.session_id())
                    .map_err(ProviderRuntimeError::Protocol)?;
            } else {
                self.clear_native_recovery()?;
            }
            self.conversation = ProviderConversationState::empty(self.binding.provider);
            self.binding.conversation = self.conversation.clone();
            Ok(())
        })
    }

    fn seed(
        &mut self,
        state: ProviderConversationState,
    ) -> super::AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        if state.provider() != self.binding.provider {
            return Box::pin(async {
                Err(ProviderRuntimeError::InvalidBinding(
                    "seed conversation provider mismatch".into(),
                ))
            });
        }
        Box::pin(async move {
            let (state, acknowledgements) = match self.adapter.loop_kind() {
                AdapterLoopKind::NativeSession => self.resolve_native_conversation(state, None)?,
                AdapterLoopKind::DirectSteps => (state, Vec::new()),
            };
            self.adapter.seed(state.clone()).await?;
            self.conversation = state.clone();
            self.binding.conversation = state;
            self.remember_native_acknowledgements(acknowledgements);
            Ok(())
        })
    }

    fn conversation_state(&self) -> ProviderConversationState {
        self.conversation.clone()
    }

    fn acknowledge_persisted(
        &mut self,
    ) -> super::AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            while let Some(identity) = self.pending_acknowledgements.last() {
                crate::provider_tool_loop::acknowledge_native_run(
                    &self.dirs.journal_dir(),
                    identity,
                )
                .map_err(ProviderRuntimeError::Transport)?;
                self.pending_acknowledgements.pop();
            }
            Ok(())
        })
    }

    fn current_workspace(&self) -> Option<PreparedWorkspace> {
        self.active_workspace.clone()
    }
}
