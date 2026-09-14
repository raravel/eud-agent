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
            // A failed update leaves the durable Pending marker, which also blocks resume.
            let _ = gate.mark_native_run_unknown();
        }
        let _ = tokio::time::timeout(grace, self.adapter.interrupt(identity)).await;
        if let Some(server) = mcp.as_mut() {
            let _ = server.close_and_drain(grace).await;
        }
        let _ = tool_events.drain(grace).await;
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
