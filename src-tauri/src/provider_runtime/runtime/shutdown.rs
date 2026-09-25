use super::{tool_events::RunToolEvents, ProviderRuntime};
use crate::{
    mcp,
    provider_runtime::{AdapterLoopKind, ProviderRuntimeError},
    provider_tool_loop::RunGate,
};
use std::time::Duration;

pub(super) struct RunLifetimeGuard(pub(super) RunGate);

impl Drop for RunLifetimeGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl ProviderRuntime {
    pub(super) async fn stop_run(
        &mut self,
        identity: &super::RunIdentity,
        gate: &RunGate,
        mcp: &mut Option<mcp::McpServerHandle>,
        tool_events: &mut RunToolEvents,
        grace: Duration,
    ) {
        gate.cancel();
        if self.adapter.loop_kind() == AdapterLoopKind::NativeSession {
            self.settle_interrupted_native(gate).await;
        }
        let _ = tokio::time::timeout(grace, self.adapter.interrupt(identity)).await;
        if let Some(server) = mcp.as_mut() {
            let _ = server.close_and_drain(grace).await;
        }
        let _ = tool_events.drain(grace).await;
    }

    /// Settle a native run that ended without its own boundary.
    pub(super) async fn settle_interrupted_native(&mut self, gate: &RunGate) {
        match self
            .adapter
            .observed_conversation()
            .filter(|conversation| conversation.is_started())
        {
            // The cut run still names a resumable native session: keep it as
            // the boundary the next run continues from, so an interruption
            // costs the unfinished turn and not the whole conversation. A
            // cancelled or timed-out step drops the adapter future before the
            // adapter settles itself, so the adapter is seeded onto the same
            // boundary or it would refuse the next run.
            Some(conversation) => {
                if let Some(session) = conversation.conversation_key() {
                    let _ = gate.mark_native_run_interrupted(&session);
                }
                let _ = self.adapter.seed(conversation.clone()).await;
                self.conversation = conversation.clone();
                self.binding.conversation = conversation;
            }
            // A failed update leaves the durable Pending marker, which also blocks resume.
            None => {
                let _ = gate.mark_native_run_unknown();
            }
        }
    }

    pub(super) async fn finish_run(
        gate: &RunGate,
        mcp: &mut Option<mcp::McpServerHandle>,
        tool_events: &mut RunToolEvents,
        grace: Duration,
        native_completion_in_flight: Option<usize>,
    ) -> Result<(), ProviderRuntimeError> {
        let shutdown = match mcp.as_mut() {
            Some(server) => server
                .close_and_drain(grace)
                .await
                .map_err(ProviderRuntimeError::Transport),
            None => gate
                .cancel_and_drain(grace)
                .await
                .map_err(ProviderRuntimeError::Transport),
        };
        let published = tool_events.drain(grace).await;
        shutdown?;
        published?;
        tool_events.check_fatal()?;
        if native_completion_in_flight.is_some_and(|in_flight| in_flight > 0) {
            return Err(ProviderRuntimeError::Protocol(
                "native provider completed while a tool call was still in flight".into(),
            ));
        }
        Ok(())
    }
}
