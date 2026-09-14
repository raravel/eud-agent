use serde_json::Value;

use crate::{
    antigravity_auth::{
        AntigravityAuthHandle, AntigravityCredential, ANTIGRAVITY_USER_AGENT, CLOUD_CODE_ENDPOINT,
    },
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{
        AdapterEvent, AdapterEventKind, AdapterFuture, AdapterLoopKind, AdapterOutput,
        AdapterRequestKind, AdapterStepOutcome, AdapterStepRequest, ProviderAdapter,
        ProviderRuntimeError, RunId, RunIdentity,
    },
};

use super::{
    catalog::{fetch_live_catalog_at, status_error},
    codec::{decode_stream, request_body},
    LiveAntigravityModel,
};

pub(crate) struct AntigravityAdapter {
    model: String,
    auth: AntigravityAuthHandle,
    client: reqwest::Client,
    endpoint: String,
    active_run: Option<RunId>,
    active_model: Option<LiveAntigravityModel>,
    active_credential: Option<AntigravityCredential>,
}

impl AntigravityAdapter {
    pub(crate) fn new(
        model: String,
        auth: AntigravityAuthHandle,
    ) -> Result<Self, ProviderRuntimeError> {
        if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
            return Err(ProviderRuntimeError::Protocol(
                "provider_model_unavailable".into(),
            ));
        }
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(600))
            .user_agent(ANTIGRAVITY_USER_AGENT)
            .build()
            .map_err(|_| ProviderRuntimeError::Transport("provider_transport_closed".into()))?;
        Ok(Self {
            model,
            auth,
            client,
            endpoint: CLOUD_CODE_ENDPOINT.to_string(),
            active_run: None,
            active_model: None,
            active_credential: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_transport(
        model: String,
        auth: AntigravityAuthHandle,
        client: reqwest::Client,
        endpoint: String,
    ) -> Result<Self, ProviderRuntimeError> {
        if model.is_empty() {
            return Err(ProviderRuntimeError::Protocol(
                "provider_model_unavailable".into(),
            ));
        }
        Ok(Self {
            model,
            auth,
            client,
            endpoint,
            active_run: None,
            active_model: None,
            active_credential: None,
        })
    }

    async fn execute_step(
        &mut self,
        mut request: AdapterStepRequest,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> Result<AdapterStepOutcome, ProviderRuntimeError> {
        if *request.cancellation.borrow() != request.identity.cancellation_generation {
            return Ok(AdapterStepOutcome::Cancelled);
        }
        if request.binding.model != self.model {
            return Err(ProviderRuntimeError::Protocol(
                "provider model binding changed".into(),
            ));
        }
        let same_run = self.active_run == Some(request.identity.run_id);
        let mut credential = if same_run {
            self.active_credential.clone().ok_or_else(|| {
                ProviderRuntimeError::Protocol("provider credential snapshot unavailable".into())
            })?
        } else {
            self.auth
                .current()
                .await
                .map_err(ProviderRuntimeError::Transport)?
        };
        let live_model = if same_run {
            self.active_model.clone().ok_or_else(|| {
                ProviderRuntimeError::Protocol("provider model snapshot unavailable".into())
            })?
        } else {
            let model = fetch_live_catalog_at(&self.client, &credential, &self.endpoint)
                .await?
                .into_iter()
                .find(|model| model.id == self.model)
                .ok_or_else(|| {
                    ProviderRuntimeError::Protocol("provider_model_unavailable".into())
                })?;
            self.active_run = Some(request.identity.run_id);
            self.active_model = Some(model.clone());
            self.active_credential = Some(credential.clone());
            model
        };
        if matches!(&request.kind, AdapterRequestKind::Foreground(turn) if !turn.image_paths.is_empty() && !live_model.supports_images)
        {
            return Err(ProviderRuntimeError::Protocol(
                "provider_capability_unsupported".into(),
            ));
        }
        let body = request_body(&request, &live_model, &credential.project_id).await?;
        let response_id = format!(
            "antigravity-{}-{}",
            request.identity.run_id.get(),
            request.history.len()
        );
        events
            .send(AdapterEvent {
                identity: request.identity.clone(),
                kind: AdapterEventKind::ResponseStarted {
                    response_id: response_id.clone(),
                },
            })
            .await
            .map_err(|_| ProviderRuntimeError::Cancelled)?;
        let mut response = self.send(&credential, &body).await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            credential = self
                .auth
                .refresh()
                .await
                .map_err(ProviderRuntimeError::Transport)?;
            self.active_credential = Some(credential.clone());
            response = self.send(&credential, &body).await?;
        }
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        let decoded = decode_stream(response, &mut request, &events, &response_id).await?;
        let continuation = decoded.continuations.iter().flatten().last().cloned();
        match (decoded.structured, decoded.calls.is_empty()) {
            (Some(value), _) => Ok(AdapterStepOutcome::Completed {
                output: AdapterOutput::Structured(value),
                continuation,
                native_conversation: None,
            }),
            (None, true) => Ok(AdapterStepOutcome::Completed {
                output: AdapterOutput::Text(decoded.text),
                continuation,
                native_conversation: None,
            }),
            (None, false) => Ok(AdapterStepOutcome::NeedsTools {
                calls: decoded.calls,
                continuation,
            }),
        }
    }

    async fn send(
        &self,
        credential: &AntigravityCredential,
        body: &Value,
    ) -> Result<reqwest::Response, ProviderRuntimeError> {
        self.client
            .post(format!(
                "{}/v1internal:streamGenerateContent?alt=sse",
                self.endpoint
            ))
            .bearer_auth(&credential.access_token)
            .json(body)
            .send()
            .await
            .map_err(|_| ProviderRuntimeError::Transport("provider_transport_closed".into()))
    }
}

impl ProviderAdapter for AntigravityAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Antigravity
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
        self.active_run = None;
        self.active_model = None;
        self.active_credential = None;
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
                "native compaction unsupported".into(),
            ))
        })
    }
    fn seed(
        &mut self,
        state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            match state {
                ProviderConversationState::Antigravity { .. } => Ok(()),
                ProviderConversationState::Codex { .. }
                | ProviderConversationState::ClaudeCode { .. }
                | ProviderConversationState::OpencodeGo { .. }
                | ProviderConversationState::Ollama { .. } => Err(ProviderRuntimeError::Protocol(
                    "incompatible conversation state".into(),
                )),
            }
        })
    }
}
