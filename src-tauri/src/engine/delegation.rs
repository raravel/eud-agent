//! Model-invoked read delegation: the `delegate_read` tool's executor.
//!
//! The tool runtime admits the call and owns the child's lifecycle event; this
//! module turns one admitted [`ReadDelegation`] into an isolated
//! [`DelegatedRunExecutor`] run over the session's read tools and folds the
//! child's provider usage into the session total. It is injected into the
//! runtime by the session manager, exactly like the ask emitter, so the tool
//! runtime never owns provider execution.

use std::{path::PathBuf, sync::Arc};

use super::{runtime_events::SessionRuntimeEventSink, EngineEvent, EventSink};
use crate::{
    config::DataDirs,
    ipc,
    provider_runtime::{
        production_adapter, BindingSnapshot, DelegatedRunExecutor, DelegatedRunKind,
        DelegatedRunOutcome, DelegatedRunRequest, ProviderRuntimeError,
    },
    provider_tool_loop::DelegatedToolProfile,
    session::SessionStore,
    tool_exec::{ReadDelegation, SessionToolRuntime},
    workflow,
    workspace::WorkspaceManager,
};

/// Everything one session's `delegate_read` executor needs beyond the admitted
/// call. Cloned per delegation.
#[derive(Clone)]
pub(super) struct ReadDelegationContext<S> {
    pub dirs: DataDirs,
    pub fallback_cwd: PathBuf,
    /// The session's immutable provider binding; the child always starts with
    /// an empty conversation.
    pub binding: BindingSnapshot,
    pub tools: SessionToolRuntime,
    pub sink: S,
    pub sessions: SessionStore,
    pub session_id: String,
    pub cancellation: tokio::sync::watch::Receiver<u64>,
}

/// Build the child's request from the admitted call. Pure, so its contract is
/// testable without a provider.
pub(crate) fn read_delegation_request(
    delegation: &ReadDelegation,
    binding: &BindingSnapshot,
    workspace_root: PathBuf,
    workspace_temp: Option<PathBuf>,
) -> Result<DelegatedRunRequest, String> {
    let kind = DelegatedRunKind::Read;
    let schema = workflow::stage_schema(kind);
    let profile = DelegatedToolProfile::new(workflow::stage_tools(kind).iter().copied(), &schema)?;
    let mut binding = binding.clone();
    binding.conversation = crate::provider::ProviderConversationState::empty(binding.provider);
    Ok(DelegatedRunRequest {
        identity: delegation.identity.clone(),
        parent_run_id: Some(delegation.parent_run_id),
        binding,
        kind,
        prompt: workflow::read_delegation_prompt(&delegation.goal, &delegation.focus),
        workspace_root,
        workspace_temp,
        output_schema: schema,
        profile,
        allow_live_write_ticket: false,
        policy: workflow::stage_policy(kind),
    })
}

impl<S: EventSink + Clone + Send + Sync + 'static> ReadDelegationContext<S> {
    /// Run one admitted delegation to its outcome. Failures are returned as the
    /// outcome (never panics or propagates), so the tool runtime can complete
    /// the parent's call as a usage error.
    pub(super) async fn run(self, delegation: ReadDelegation) -> DelegatedRunOutcome {
        let workspace = {
            let manager = WorkspaceManager::new(self.dirs.clone());
            let session_id = self.session_id.clone();
            match tokio::task::spawn_blocking(move || manager.prepare_session_current(&session_id))
                .await
            {
                Ok(Ok(workspace)) => workspace,
                Ok(Err(error)) => {
                    return DelegatedRunOutcome::Failed(ProviderRuntimeError::Transport(error))
                }
                Err(error) => {
                    return DelegatedRunOutcome::Failed(ProviderRuntimeError::Transport(
                        error.to_string(),
                    ))
                }
            }
        };
        let request = match read_delegation_request(
            &delegation,
            &self.binding,
            workspace.root,
            Some(workspace.temp_dir),
        ) {
            Ok(request) => request,
            Err(error) => {
                return DelegatedRunOutcome::Failed(ProviderRuntimeError::Protocol(error))
            }
        };
        let adapter = match production_adapter(
            &request.binding.to_binding(),
            &self.dirs,
            self.fallback_cwd.clone(),
        ) {
            Ok(adapter) => adapter,
            Err(error) => return DelegatedRunOutcome::Failed(error),
        };
        let events = Arc::new(SessionRuntimeEventSink::delegated(
            self.sink.clone(),
            self.sessions.clone(),
            self.session_id.clone(),
            delegation.identity.run_id.get(),
        ));
        let outcome = DelegatedRunExecutor::new(adapter, self.tools, events, self.cancellation)
            .run(request)
            .await;
        if let DelegatedRunOutcome::Result {
            usage: Some(usage), ..
        } = &outcome
        {
            // The child's tokens count toward the session total only; `last`
            // stays the parent's own active context.
            match self
                .sessions
                .add_delegated_context_usage(&self.session_id, usage)
            {
                Ok(token_usage) => {
                    if let Err(error) = self.sink.emit(EngineEvent::ContextUsage(
                        ipc::ContextUsageEvent {
                            turn_id: delegation.identity.request_id.clone(),
                            token_usage,
                        },
                    )) {
                        eprintln!(
                            "eud-agent: delegated context usage event failed: session={} error={}",
                            self.session_id, error.message
                        );
                    }
                }
                Err(error) => eprintln!(
                    "eud-agent: delegated context usage persistence failed: session={} error={error}",
                    self.session_id
                ),
            }
        }
        outcome
    }
}
