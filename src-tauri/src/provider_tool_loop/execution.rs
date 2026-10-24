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
        // A write to an agent-instruction file asks the user first. Only
        // that wait ends when the gate closes: once the user allowed it, the
        // write runs to completion below with its admission and the card
        // slot held, so a cancel never leaves it running unaccounted.
        let mut confirmation = None;
        if self
            .inner
            .runtime
            .instruction_write_needs_confirmation(&call.name, &call.arguments)
        {
            let closed_error =
                || "provider tool gate closed while instruction write confirmation was pending";
            let mut closed = self.inner.closed.subscribe();
            if *closed.borrow() {
                return Err(closed_error().to_string());
            }
            let confirmed = tokio::select! {
                biased;
                changed = closed.changed() => {
                    let _ = changed;
                    return Err(closed_error().to_string());
                }
                confirmed = self.inner.runtime.confirm_instruction_write(
                    &self.inner.identity,
                    &call.name,
                    &call.arguments,
                ) => confirmed,
            };
            if confirmed.is_ok() && *closed.borrow() {
                // Closed as the user answered: nothing has run yet.
                self.inner.runtime.clear_instruction_write_approval();
                return Err(closed_error().to_string());
            }
            confirmation = Some(confirmed);
        }
        // `ask`, `map_task_request` and the browser tools wait on the runtime
        // asynchronously (a user answer, a team session, the browser
        // window's UI thread) instead of blocking a worker thread.
        let waiting = match call.name.as_str() {
            _ if confirmation.is_some() => None,
            crate::tools::ASK_TOOL => Some("ASK"),
            crate::tools::MAP_TASK_REQUEST_TOOL => Some("map_task_request"),
            name if crate::tools::is_browser_tool(name) => Some("browser"),
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
                    crate::tools::MAP_TASK_REQUEST_TOOL => {
                        Box::pin(runtime.map_task_request_for_run(identity, &call.arguments))
                    }
                    name => Box::pin(runtime.browser_for_run(identity, name, &call.arguments)),
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
            // An allowed instruction write keeps the session's card slot
            // until it ran: a queued card shown before it would refuse the
            // write the user allowed. A refusal completes without running.
            let (outcome, _shown) = match confirmation {
                Some(Err(refused)) => (Err(refused), None),
                Some(Ok(shown)) => {
                    let outcome = completion_gate.execute_outcome(&call, || {
                        runtime.execute_for_run(&identity, &call.name, &call.arguments)
                    });
                    runtime.clear_instruction_write_approval();
                    (outcome, shown)
                }
                None => (
                    completion_gate.execute_outcome(&call, || {
                        runtime.execute_for_run(&identity, &call.name, &call.arguments)
                    }),
                    None,
                ),
            };
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
