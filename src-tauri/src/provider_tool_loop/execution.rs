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
        // `ask` and `delegate_read` wait on the runtime asynchronously (a user
        // answer, a child run) instead of blocking a worker thread.
        let waiting = match call.name.as_str() {
            crate::tools::ASK_TOOL => Some("ASK"),
            crate::tools::DELEGATE_READ_TOOL => Some("delegate_read"),
            crate::tools::MAP_TASK_REQUEST_TOOL => Some("map_task_request"),
            _ => None,
        };
        if let Some(label) = waiting {
            let outcome = {
                let mut closed = self.inner.closed.subscribe();
                let runtime = &self.inner.runtime;
                let identity = &self.inner.identity;
                let wait: std::pin::Pin<
                    Box<
                        dyn std::future::Future<Output = Result<serde_json::Value, String>>
                            + Send
                            + '_,
                    >,
                > = match call.name.as_str() {
                    crate::tools::ASK_TOOL => {
                        Box::pin(runtime.ask_for_run(identity, &call.arguments))
                    }
                    crate::tools::DELEGATE_READ_TOOL => {
                        Box::pin(runtime.delegate_read_for_run(identity, &call.arguments))
                    }
                    _ => Box::pin(runtime.map_task_request_for_run(identity, &call.arguments)),
                };
                tokio::pin!(wait);
                let already_closed = *closed.borrow();
                if already_closed {
                    return Err(format!(
                        "provider tool gate closed while {label} was pending"
                    ));
                } else {
                    tokio::select! {
                        biased;
                        changed = closed.changed() => {
                            let _ = changed;
                            return Err(format!(
                                "provider tool gate closed while {label} was pending"
                            ));
                        }
                        outcome = &mut wait => outcome,
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
            .map_err(|error| format!("provider {label} completion task failed: {error}"))??;
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

    /// Complete one admitted call with a model-correctable usage error without
    /// executing it. The result is durable (journal/checkpoint/receipt) and
    /// published exactly like an executed completion, so the model receives the
    /// guidance and the transcript stays paired.
    pub(super) async fn complete_usage_error(
        &self,
        call: &DirectToolCall,
        message: String,
    ) -> Result<DirectToolResult, String> {
        let admission = self.admit()?;
        self.publish_started(call)?;
        let completion_inner = Arc::clone(&self.inner);
        let completion_gate = self.clone();
        let call = call.clone();
        let result = tokio::task::spawn_blocking(move || {
            let result = tool_result(&call, Err(message));
            let recorded = record_completion(&completion_inner, &call, &result);
            let published = recorded.and_then(|()| completion_gate.publish_completed(&result));
            drop(admission);
            published.map(|()| result)
        })
        .await
        .map_err(|error| format!("provider tool usage completion task failed: {error}"))?;
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
