use super::*;

impl OpenCodeGoAdapter {
    pub(crate) fn new(secrets: ProviderSecretStore) -> Result<Self, ProviderRuntimeError> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(600))
            .user_agent(concat!("eud-agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| {
                ProviderRuntimeError::Transport("provider_catalog_unavailable".to_string())
            })?;
        Ok(Self {
            client,
            credential: OpenCodeGoCredential::Stored(secrets),
            inference_base_url: BASE_URL.to_string(),
            live_models_base_url: BASE_URL.to_string(),
            metadata_url: MODELS_DEV_URL.to_string(),
            models_by_run: HashMap::new(),
        })
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(
        client: reqwest::Client,
        inference_base_url: String,
        live_models_base_url: String,
        metadata_url: String,
        credential: String,
    ) -> Self {
        Self {
            client,
            credential: OpenCodeGoCredential::Fixture(Zeroizing::new(credential)),
            inference_base_url,
            live_models_base_url,
            metadata_url,
            models_by_run: HashMap::new(),
        }
    }

    pub(super) async fn resolve_model(
        &mut self,
        run_id: crate::provider_runtime::RunId,
        model_id: &str,
    ) -> Result<(Zeroizing<String>, LiveOpenCodeGoModel), ProviderRuntimeError> {
        if model_id.is_empty() || model_id.len() > 256 || model_id.chars().any(char::is_control) {
            return Err(ProviderRuntimeError::Protocol(
                "provider_model_unavailable".to_string(),
            ));
        }
        let api_key = self.credential.read()?;
        if let Some(model) = self.models_by_run.get(&run_id) {
            if model.id != model_id {
                return Err(ProviderRuntimeError::InvalidBinding(
                    "provider model changed during a run".to_string(),
                ));
            }
            return Ok((api_key, model.clone()));
        }
        let live_ids = fetch_live_ids_at(&self.client, &api_key, &self.live_models_base_url)
            .await
            .map_err(ProviderRuntimeError::Transport)?;
        let metadata = fetch_models_dev_at(&self.client, &self.metadata_url)
            .await
            .map_err(ProviderRuntimeError::Transport)?;
        let model = join_live_catalog(&live_ids, &metadata)
            .into_iter()
            .find(|candidate| candidate.id == model_id)
            .ok_or_else(|| {
                ProviderRuntimeError::Protocol("provider_model_unavailable".to_string())
            })?;
        self.models_by_run.insert(run_id, model.clone());
        Ok((api_key, model))
    }
}

impl ProviderAdapter for OpenCodeGoAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::OpencodeGo
    }

    fn loop_kind(&self) -> AdapterLoopKind {
        AdapterLoopKind::DirectSteps
    }

    fn run_step<'a>(
        &'a mut self,
        request: AdapterStepRequest,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<AdapterStepOutcome, ProviderRuntimeError>> {
        Box::pin(async move {
            let run_id = request.identity.run_id;
            let result = self.run_one_step(request, events).await;
            if !matches!(result, Ok(AdapterStepOutcome::NeedsTools { .. })) {
                self.models_by_run.remove(&run_id);
            }
            result
        })
    }

    fn interrupt(
        &mut self,
        identity: &RunIdentity,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        self.models_by_run.remove(&identity.run_id);
        Box::pin(async { Ok(()) })
    }

    fn reset(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        self.models_by_run.clear();
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
                "direct provider compaction is runtime-owned".to_string(),
            ))
        })
    }

    fn seed(
        &mut self,
        state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            match state {
                ProviderConversationState::OpencodeGo { .. } => Ok(()),
                ProviderConversationState::Codex { .. }
                | ProviderConversationState::ClaudeCode { .. }
                | ProviderConversationState::Antigravity { .. }
                | ProviderConversationState::Ollama { .. } => {
                    Err(ProviderRuntimeError::InvalidBinding(
                        "OpenCode Go adapter received incompatible conversation state".to_string(),
                    ))
                }
            }
        })
    }
}
