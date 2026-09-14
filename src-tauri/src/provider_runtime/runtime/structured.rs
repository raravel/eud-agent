use super::ProviderRuntime;
use crate::{
    provider::ProviderConversationState,
    provider_runtime::{
        AdapterOutput, AdapterRequestKind, AdapterStepOutcome, AdapterStepRequest, ProviderAdapter,
        ProviderRuntimeError, RunOutcome, StructuredJobRequest,
    },
    provider_tool_loop::validate_structured_output,
};
use std::sync::Arc;

pub struct StructuredJobExecutor {
    adapter: Box<dyn ProviderAdapter>,
    cancellation: tokio::sync::watch::Receiver<u64>,
}

impl StructuredJobExecutor {
    pub fn new(
        adapter: Box<dyn ProviderAdapter>,
        cancellation: tokio::sync::watch::Receiver<u64>,
    ) -> Self {
        Self {
            adapter,
            cancellation,
        }
    }

    pub async fn run(&mut self, request: StructuredJobRequest) -> RunOutcome {
        if *self.cancellation.borrow() != request.identity.cancellation_generation {
            return RunOutcome::Cancelled;
        }
        let mut binding = request.binding.clone();
        binding.conversation = ProviderConversationState::empty(binding.provider);
        if self.adapter.provider() != binding.provider {
            return RunOutcome::Failed(ProviderRuntimeError::InvalidBinding(
                "structured request does not match its adapter".into(),
            ));
        }
        let step_request = AdapterStepRequest {
            identity: request.identity.clone(),
            binding,
            kind: AdapterRequestKind::Structured {
                kind: request.kind,
                prompt: request.prompt.clone(),
                workspace_root: request.workspace_root.clone(),
                output_schema: request.output_schema.clone(),
            },
            policy: request.policy.clone(),
            continuation: None,
            history: Arc::from([]),
            prior_tool_results: Arc::from([]),
            tool_descriptors: Arc::from([]),
            native_mcp_endpoint: None,
            cancellation: self.cancellation.clone(),
        };
        let (_ask_sender, ask_waiting) = tokio::sync::watch::channel(false);
        let step = ProviderRuntime::run_adapter_step(
            self.adapter.as_mut(),
            step_request,
            None,
            None,
            ask_waiting,
        )
        .await;
        let needs_interrupt = match &step {
            Err(_) => true,
            Ok(step) => matches!(step.outcome, AdapterStepOutcome::Cancelled),
        };
        if needs_interrupt {
            let _ = tokio::time::timeout(
                request.policy.shutdown_grace,
                self.adapter.interrupt(&request.identity),
            )
            .await;
        }
        let step = match step {
            Ok(step) => step,
            Err(ProviderRuntimeError::Cancelled) => return RunOutcome::Cancelled,
            Err(error) => return RunOutcome::Failed(error),
        };
        if matches!(&step.outcome, AdapterStepOutcome::Cancelled) {
            return RunOutcome::Cancelled;
        }
        if !step
            .finished
            .as_ref()
            .is_some_and(|(_, complete)| *complete)
        {
            return RunOutcome::Failed(ProviderRuntimeError::Protocol(
                "structured response ended without a complete boundary".into(),
            ));
        }
        match step.outcome {
            AdapterStepOutcome::Completed {
                output: AdapterOutput::Structured(value),
                ..
            } => {
                let output_size =
                    serde_json::to_vec(&value).map_or(usize::MAX, |bytes| bytes.len());
                if output_size > request.policy.max_output_bytes
                    || validate_structured_output(&request.output_schema, &value).is_err()
                {
                    RunOutcome::Failed(ProviderRuntimeError::StructuredOutputInvalid)
                } else {
                    RunOutcome::Structured {
                        value,
                        base: request.base,
                    }
                }
            }
            AdapterStepOutcome::Cancelled => RunOutcome::Cancelled,
            AdapterStepOutcome::Completed {
                output: AdapterOutput::Text(_),
                ..
            }
            | AdapterStepOutcome::NeedsTools { .. } => {
                RunOutcome::Failed(ProviderRuntimeError::StructuredOutputInvalid)
            }
        }
    }
}
