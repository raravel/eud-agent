use super::*;

impl ProviderAdapter for ProductionOllamaAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Ollama
    }

    fn loop_kind(&self) -> AdapterLoopKind {
        AdapterLoopKind::DirectSteps
    }

    fn run_step<'a>(
        &'a mut self,
        request: AdapterStepRequest,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<AdapterStepOutcome, ProviderRuntimeError>> {
        Box::pin(self.execute_step(request, events))
    }

    fn interrupt(
        &mut self,
        _identity: &RunIdentity,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }

    fn reset(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }

    fn compact<'a>(
        &'a mut self,
        _identity: RunIdentity,
        _cancellation: tokio::sync::watch::Receiver<u64>,
        _events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<ProviderConversationState, ProviderRuntimeError>> {
        Box::pin(async {
            Err(ProviderRuntimeError::Protocol(
                "provider_capability_unsupported".to_string(),
            ))
        })
    }

    fn seed(
        &mut self,
        state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            match state {
                ProviderConversationState::Ollama { .. } => Ok(()),
                _ => Err(ProviderRuntimeError::InvalidBinding(
                    "Ollama adapter received incompatible conversation state".to_string(),
                )),
            }
        })
    }
}
