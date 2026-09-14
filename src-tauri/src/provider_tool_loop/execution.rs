use std::sync::Arc;

use super::{
    completion::tool_result, gate_state::record_completion, DirectToolCall, DirectToolResult,
    RunGate,
};

impl RunGate {
    pub(super) async fn execute_call(
        &self,
        call: DirectToolCall,
    ) -> Result<DirectToolResult, String> {
        let admission = self.admit()?;
        self.publish_started(&call)?;
        if call.name == crate::tools::ASK_TOOL {
            let outcome = {
                let mut closed = self.inner.closed.subscribe();
                let ask = self
                    .inner
                    .runtime
                    .ask_for_run(&self.inner.identity, &call.arguments);
                tokio::pin!(ask);
                let already_closed = *closed.borrow();
                if already_closed {
                    return Err("provider tool gate closed while ASK was pending".to_string());
                } else {
                    tokio::select! {
                        biased;
                        changed = closed.changed() => {
                            let _ = changed;
                            return Err("provider tool gate closed while ASK was pending".to_string());
                        }
                        outcome = &mut ask => outcome,
                    }
                }
            };
            if let Some(error) = stale_scope_error(&outcome) {
                return Err(error);
            }
            let result = tool_result(&call, outcome);
            let completion_inner = Arc::clone(&self.inner);
            let completion_gate = self.clone();
            let persisted_result = result.clone();
            tokio::task::spawn_blocking(move || {
                let recorded = record_completion(&completion_inner, &call, &persisted_result);
                let published =
                    recorded.and_then(|()| completion_gate.publish_completed(&persisted_result));
                drop(admission);
                published
            })
            .await
            .map_err(|error| format!("provider ASK completion task failed: {error}"))??;
            return Ok(result);
        }

        let runtime = self.inner.runtime.clone();
        let identity = self.inner.identity.clone();
        let completion_inner = Arc::clone(&self.inner);
        let completion_gate = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let outcome = completion_gate.execute_outcome(&call, || {
                runtime.execute_for_run(&identity, &call.name, &call.arguments)
            });
            if let Some(error) = stale_scope_error(&outcome) {
                drop(admission);
                return Err(error);
            }
            let result = tool_result(&call, outcome);
            let recorded = record_completion(&completion_inner, &call, &result);
            let published = recorded.and_then(|()| completion_gate.publish_completed(&result));
            drop(admission);
            published.map(|()| result)
        })
        .await
        .map_err(|error| format!("provider tool execution task failed: {error}"))?;
        result
    }
}

fn stale_scope_error(outcome: &Result<serde_json::Value, String>) -> Option<String> {
    outcome
        .as_ref()
        .err()
        .filter(|error| error.starts_with("stale provider run"))
        .cloned()
}
