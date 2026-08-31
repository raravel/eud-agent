//! Process-wide provider install/auth/catalog/default service and generic Tauri commands.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex as SyncMutex;
use serde::{Deserialize, Serialize};
use tauri::Emitter as _;
use zeroize::Zeroize;

use crate::provider::{
    ModelCapabilities, ProviderAvailability, ProviderId, ProviderModel, ProviderSettingsView,
    ProviderStatus, ProviderStatusCode, ReasoningLevel, ReasoningSelection,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderProgressEvent {
    pub provider: ProviderId,
    pub attempt_id: String,
    pub stage: ProviderProgressStage,
    pub percent: Option<u8>,
    pub detail_code: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderProgressStage {
    Install,
    Login,
    Catalog,
    Refresh,
}

#[derive(Clone)]
pub struct ProviderService {
    inner: Arc<ProviderServiceInner>,
}

struct ProviderServiceInner {
    dirs: crate::config::DataDirs,
    secrets: crate::provider_secrets::ProviderSecretStore,
    client: reqwest::Client,
    ollama_client: reqwest::Client,
    codex_lock: tokio::sync::Mutex<()>,
    claude_lock: tokio::sync::Mutex<()>,
    antigravity_lock: tokio::sync::Mutex<()>,
    opencode_lock: tokio::sync::Mutex<()>,
    ollama_lock: tokio::sync::Mutex<()>,
    settings_lock: tokio::sync::Mutex<()>,
    oauth_attempts: SyncMutex<HashMap<ProviderId, OAuthAttempt>>,
    busy: SyncMutex<HashMap<ProviderId, usize>>,
}

struct OAuthAttempt {
    id: String,
    cancellation: tokio::sync::watch::Sender<bool>,
}

pub struct ProviderBusyLease {
    service: ProviderService,
    provider: ProviderId,
}

impl Drop for ProviderBusyLease {
    fn drop(&mut self) {
        let mut busy = self.service.inner.busy.lock();
        if let Some(count) = busy.get_mut(&self.provider) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                busy.remove(&self.provider);
            }
        }
    }
}

impl ProviderService {
    pub fn new(dirs: crate::config::DataDirs) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(120))
            .user_agent(concat!("eud-agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| "provider_transport_closed".to_string())?;
        let ollama_client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(8))
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(concat!("eud-agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| "provider_transport_closed".to_string())?;
        let secrets = crate::provider_secrets::ProviderSecretStore::new(dirs.clone())?;
        Ok(Self {
            inner: Arc::new(ProviderServiceInner {
                dirs,
                secrets,
                client,
                ollama_client,
                codex_lock: tokio::sync::Mutex::new(()),
                claude_lock: tokio::sync::Mutex::new(()),
                antigravity_lock: tokio::sync::Mutex::new(()),
                opencode_lock: tokio::sync::Mutex::new(()),
                ollama_lock: tokio::sync::Mutex::new(()),
                settings_lock: tokio::sync::Mutex::new(()),
                oauth_attempts: SyncMutex::new(HashMap::new()),
                busy: SyncMutex::new(HashMap::new()),
            }),
        })
    }

    pub fn dirs(&self) -> &crate::config::DataDirs {
        &self.inner.dirs
    }

    pub fn enter_busy(&self, provider: ProviderId) -> ProviderBusyLease {
        *self.inner.busy.lock().entry(provider).or_default() += 1;
        ProviderBusyLease {
            service: self.clone(),
            provider,
        }
    }

    fn is_busy(&self, provider: ProviderId) -> bool {
        self.inner.busy.lock().get(&provider).copied().unwrap_or(0) > 0
    }

    pub async fn status_list(&self) -> Result<Vec<ProviderStatus>, String> {
        let config = self
            .inner
            .dirs
            .load_config()
            .map_err(|_| "provider_protocol_changed".to_string())?;
        let timeout = std::time::Duration::from_secs(8);
        let (codex, claude, antigravity, opencode, ollama) = tokio::join!(
            tokio::time::timeout(timeout, self.status_one(ProviderId::Codex)),
            tokio::time::timeout(timeout, self.status_one(ProviderId::ClaudeCode)),
            tokio::time::timeout(timeout, self.status_one(ProviderId::Antigravity)),
            tokio::time::timeout(timeout, self.status_one(ProviderId::OpencodeGo)),
            tokio::time::timeout(timeout, self.status_one(ProviderId::Ollama)),
        );
        let timed_out = || {
            (
                ProviderAvailability::Unavailable,
                Some(ProviderStatusCode::ProviderTransportClosed),
            )
        };
        let probes = [
            codex.unwrap_or_else(|_| timed_out()),
            claude.unwrap_or_else(|_| timed_out()),
            antigravity.unwrap_or_else(|_| timed_out()),
            opencode.unwrap_or_else(|_| timed_out()),
            ollama.unwrap_or_else(|_| timed_out()),
        ];
        Ok(ProviderId::ALL
            .into_iter()
            .zip(probes)
            .map(|(provider, (availability, detail_code))| ProviderStatus {
                provider,
                availability,
                selected_as_default: config.default_provider == Some(provider),
                can_install: matches!(provider, ProviderId::Codex | ProviderId::ClaudeCode),
                can_import: matches!(provider, ProviderId::Codex | ProviderId::ClaudeCode)
                    && self
                        .inner
                        .secrets
                        .ambient_source(provider)
                        .is_ok_and(|path| path.is_file()),
                experimental: provider == ProviderId::Antigravity,
                detail_code,
            })
            .collect())
    }

    async fn status_one(
        &self,
        provider: ProviderId,
    ) -> (ProviderAvailability, Option<ProviderStatusCode>) {
        match provider {
            ProviderId::Codex => {
                let dirs = self.inner.dirs.clone();
                let state =
                    tokio::task::spawn_blocking(move || crate::codex_auth::login_status(&dirs))
                        .await
                        .ok();
                match state {
                    Some(state) if state.authed => (ProviderAvailability::Ready, None),
                    Some(state) if state.resolved => (
                        ProviderAvailability::NeedsAuthentication,
                        Some(ProviderStatusCode::ProviderNotAuthenticated),
                    ),
                    _ => (
                        ProviderAvailability::NeedsInstall,
                        Some(ProviderStatusCode::ProviderNotInstalled),
                    ),
                }
            }
            ProviderId::ClaudeCode => {
                let dirs = self.inner.dirs.clone();
                let state =
                    tokio::task::spawn_blocking(move || crate::claude_auth::login_status(&dirs))
                        .await
                        .ok();
                match state {
                    Some(state) if !state.compatible => (
                        ProviderAvailability::Unavailable,
                        Some(ProviderStatusCode::ProviderProtocolChanged),
                    ),
                    Some(state) if state.authenticated => (ProviderAvailability::Ready, None),
                    Some(state) if state.resolved => (
                        ProviderAvailability::NeedsAuthentication,
                        Some(ProviderStatusCode::ProviderNotAuthenticated),
                    ),
                    _ => (
                        ProviderAvailability::NeedsInstall,
                        Some(ProviderStatusCode::ProviderNotInstalled),
                    ),
                }
            }
            ProviderId::Antigravity => {
                let status = crate::antigravity_auth::status(&self.inner.dirs).await;
                (status.availability, status.detail_code)
            }
            ProviderId::OpencodeGo => {
                let Some(key) = self
                    .inner
                    .secrets
                    .read_secret(provider, "api-key")
                    .ok()
                    .flatten()
                else {
                    return (
                        ProviderAvailability::NeedsCredential,
                        Some(ProviderStatusCode::ProviderCredentialMissing),
                    );
                };
                let mut key = key;
                let result =
                    crate::opencode_go::fetch_catalog(&self.inner.client, &key, None).await;
                key.zeroize();
                match result {
                    Ok(models) if !models.is_empty() => (ProviderAvailability::Ready, None),
                    Ok(_) => (
                        ProviderAvailability::Unavailable,
                        Some(ProviderStatusCode::ProviderCatalogUnavailable),
                    ),
                    Err(code) => (
                        if code == "provider_not_authenticated" {
                            ProviderAvailability::NeedsCredential
                        } else {
                            ProviderAvailability::Unavailable
                        },
                        Some(status_code(&code)),
                    ),
                }
            }
            ProviderId::Ollama => {
                let config = match self.inner.dirs.load_config() {
                    Ok(config) => config,
                    Err(_) => {
                        return (
                            ProviderAvailability::Unavailable,
                            Some(ProviderStatusCode::ProviderProtocolChanged),
                        )
                    }
                };
                let mut key = self
                    .inner
                    .secrets
                    .read_secret(provider, "api-key")
                    .ok()
                    .flatten();
                let result = crate::ollama::probe(
                    &self.inner.ollama_client,
                    &config.providers.ollama.base_url,
                    key.as_deref(),
                )
                .await;
                if let Some(key) = key.as_mut() {
                    key.zeroize();
                }
                match result {
                    Ok(()) => (ProviderAvailability::Ready, None),
                    Err(code) => (
                        if code == "provider_not_authenticated" {
                            ProviderAvailability::NeedsCredential
                        } else {
                            ProviderAvailability::Unavailable
                        },
                        Some(status_code(&code)),
                    ),
                }
            }
        }
    }

    pub async fn install<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        provider: ProviderId,
    ) -> Result<ProviderStatus, String> {
        if self.is_busy(provider) {
            return Err("provider_busy".to_string());
        }
        let attempt_id = uuid::Uuid::new_v4().to_string();
        emit_progress(
            app,
            provider,
            &attempt_id,
            ProviderProgressStage::Install,
            Some(0),
            None,
        );
        match provider {
            ProviderId::Codex => {
                let _guard = self.inner.codex_lock.lock().await;
                struct Emitter;
                impl crate::bootstrap::ProgressEmitter for Emitter {
                    fn emit(&self, _stage: &str, _pct: u8, _detail: &str) {}
                }
                crate::bootstrap::ensure_codex(&self.inner.dirs, &Emitter)
                    .await
                    .map_err(|_| "provider_transport_closed".to_string())?;
                crate::codex_client::ensure_codex_profile(&self.inner.dirs)?;
            }
            ProviderId::ClaudeCode => {
                let _guard = self.inner.claude_lock.lock().await;
                crate::claude_auth::install(&self.inner.dirs).await?;
            }
            _ => return Err("unsupported_operation".to_string()),
        }
        emit_progress(
            app,
            provider,
            &attempt_id,
            ProviderProgressStage::Install,
            Some(100),
            None,
        );
        self.status_for(provider).await
    }

    pub async fn login_start<R: tauri::Runtime>(
        &self,
        app: tauri::AppHandle<R>,
        provider: ProviderId,
    ) -> Result<String, String> {
        if self.is_busy(provider) {
            return Err("provider_busy".to_string());
        }
        if !matches!(
            provider,
            ProviderId::Codex | ProviderId::ClaudeCode | ProviderId::Antigravity
        ) {
            return Err("unsupported_operation".to_string());
        }
        let attempt_id = uuid::Uuid::new_v4().to_string();
        if let Some(previous) = self.inner.oauth_attempts.lock().remove(&provider) {
            previous.cancellation.send_replace(true);
        }
        let (cancellation, mut cancellation_rx) = tokio::sync::watch::channel(false);
        self.inner.oauth_attempts.lock().insert(
            provider,
            OAuthAttempt {
                id: attempt_id.clone(),
                cancellation,
            },
        );
        emit_progress(
            &app,
            provider,
            &attempt_id,
            ProviderProgressStage::Login,
            Some(0),
            None,
        );
        let service = self.clone();
        let emitted_attempt = attempt_id.clone();
        tauri::async_runtime::spawn(async move {
            let result = match provider {
                ProviderId::Codex => {
                    let _guard = service.inner.codex_lock.lock().await;
                    let dirs = service.inner.dirs.clone();
                    tokio::task::spawn_blocking(move || crate::codex_auth::login_oauth(&dirs))
                        .await
                        .map_err(|_| "provider_transport_closed".to_string())
                        .and_then(|result| result)
                }
                ProviderId::ClaudeCode => {
                    let _guard = service.inner.claude_lock.lock().await;
                    let dirs = service.inner.dirs.clone();
                    tokio::task::spawn_blocking(move || crate::claude_auth::login_start(&dirs))
                        .await
                        .map_err(|_| "provider_transport_closed".to_string())
                        .and_then(|result| result)
                }
                ProviderId::Antigravity => {
                    let _guard = service.inner.antigravity_lock.lock().await;
                    crate::antigravity_auth::login(&service.inner.dirs, &mut cancellation_rx).await
                }
                ProviderId::OpencodeGo | ProviderId::Ollama => {
                    Err("unsupported_operation".to_string())
                }
            };
            let current = service
                .inner
                .oauth_attempts
                .lock()
                .get(&provider)
                .map(|attempt| attempt.id == emitted_attempt)
                .unwrap_or(false);
            if current {
                service.inner.oauth_attempts.lock().remove(&provider);
                emit_progress(
                    &app,
                    provider,
                    &emitted_attempt,
                    ProviderProgressStage::Login,
                    Some(if result.is_ok() { 100 } else { 0 }),
                    result.err().as_deref(),
                );
            }
        });
        Ok(attempt_id)
    }

    pub fn cancel_login(&self, provider: ProviderId, attempt_id: &str) -> Result<(), String> {
        let mut attempts = self.inner.oauth_attempts.lock();
        let attempt = attempts
            .get(&provider)
            .filter(|attempt| attempt.id == attempt_id)
            .ok_or_else(|| "provider_cancelled".to_string())?;
        attempt.cancellation.send_replace(true);
        attempts.remove(&provider);
        Ok(())
    }

    pub async fn import_credential(&self, provider: ProviderId) -> Result<ProviderStatus, String> {
        if self.is_busy(provider) {
            return Err("provider_busy".to_string());
        }
        match provider {
            ProviderId::Codex => {
                let _guard = self.inner.codex_lock.lock().await;
                let dirs = self.inner.dirs.clone();
                self.inner.secrets.import_cli_credential(provider, || {
                    let state = crate::codex_auth::login_status(&dirs);
                    state
                        .authed
                        .then_some(())
                        .ok_or_else(|| "provider_not_authenticated".to_string())
                })?;
            }
            ProviderId::ClaudeCode => {
                let _guard = self.inner.claude_lock.lock().await;
                let dirs = self.inner.dirs.clone();
                self.inner.secrets.import_cli_credential(provider, || {
                    let state = crate::claude_auth::login_status(&dirs);
                    state
                        .authenticated
                        .then_some(())
                        .ok_or_else(|| "provider_not_authenticated".to_string())
                })?;
            }
            _ => return Err("unsupported_operation".to_string()),
        }
        self.status_for(provider).await
    }

    pub async fn save_api_key(
        &self,
        provider: ProviderId,
        mut key: String,
    ) -> Result<ProviderStatus, String> {
        if self.is_busy(provider) {
            key.zeroize();
            return Err("provider_busy".to_string());
        }
        if key.trim().is_empty() {
            key.zeroize();
            return Err("provider_credential_missing".to_string());
        }
        let result = match provider {
            ProviderId::Codex => {
                let _guard = self.inner.codex_lock.lock().await;
                let dirs = self.inner.dirs.clone();
                let owned = zeroize::Zeroizing::new(key.clone());
                tokio::task::spawn_blocking(move || crate::codex_auth::login_api_key(&dirs, &owned))
                    .await
                    .map_err(|_| "provider_transport_closed".to_string())?
                    .map(|_| ())
            }
            ProviderId::OpencodeGo => {
                let _guard = self.inner.opencode_lock.lock().await;
                let previous = self
                    .inner
                    .secrets
                    .read_secret(provider, "api-key")?
                    .map(zeroize::Zeroizing::new);
                self.inner
                    .secrets
                    .save_secret(provider, "api-key", key.trim())?;
                match crate::opencode_go::fetch_catalog(&self.inner.client, key.trim(), None).await
                {
                    Ok(models) if !models.is_empty() => Ok(()),
                    Ok(_) => Err("provider_catalog_unavailable".to_string()),
                    Err(error) => Err(error),
                }
                .inspect_err(|_| {
                    if let Some(previous) = previous.as_deref() {
                        let _ = self
                            .inner
                            .secrets
                            .save_secret(provider, "api-key", previous);
                    } else {
                        let _ = self.inner.secrets.delete_secret(provider, "api-key");
                    }
                })
            }
            ProviderId::Ollama => {
                let _guard = self.inner.ollama_lock.lock().await;
                let previous = self
                    .inner
                    .secrets
                    .read_secret(provider, "api-key")?
                    .map(zeroize::Zeroizing::new);
                self.inner
                    .secrets
                    .save_secret(provider, "api-key", key.trim())?;
                let config = self
                    .inner
                    .dirs
                    .load_config()
                    .map_err(|_| "provider_protocol_changed".to_string())?;
                crate::ollama::probe(
                    &self.inner.ollama_client,
                    &config.providers.ollama.base_url,
                    Some(key.trim()),
                )
                .await
                .inspect_err(|_| {
                    if let Some(previous) = previous.as_deref() {
                        let _ = self
                            .inner
                            .secrets
                            .save_secret(provider, "api-key", previous);
                    } else {
                        let _ = self.inner.secrets.delete_secret(provider, "api-key");
                    }
                })
            }
            _ => Err("unsupported_operation".to_string()),
        };
        key.zeroize();
        result?;
        self.status_for(provider).await
    }

    pub async fn logout(&self, provider: ProviderId) -> Result<ProviderStatus, String> {
        if self.is_busy(provider) {
            return Err("provider_busy".to_string());
        }
        match provider {
            ProviderId::Codex => {
                let _guard = self.inner.codex_lock.lock().await;
                let dirs = self.inner.dirs.clone();
                tokio::task::spawn_blocking(move || crate::codex_auth::logout(&dirs))
                    .await
                    .map_err(|_| "provider_transport_closed".to_string())??;
            }
            ProviderId::ClaudeCode => {
                let _guard = self.inner.claude_lock.lock().await;
                let dirs = self.inner.dirs.clone();
                tokio::task::spawn_blocking(move || crate::claude_auth::logout(&dirs))
                    .await
                    .map_err(|_| "provider_transport_closed".to_string())??;
            }
            ProviderId::Antigravity => {
                let _guard = self.inner.antigravity_lock.lock().await;
                crate::antigravity_auth::logout(&self.inner.dirs)?;
            }
            ProviderId::OpencodeGo => {
                let _guard = self.inner.opencode_lock.lock().await;
                self.inner.secrets.delete_secret(provider, "api-key")?;
            }
            ProviderId::Ollama => {
                let _guard = self.inner.ollama_lock.lock().await;
                self.inner.secrets.delete_secret(provider, "api-key")?;
            }
        }
        self.status_for(provider).await
    }

    pub async fn catalog(&self, provider: ProviderId) -> Result<Vec<ProviderModel>, String> {
        let config = self
            .inner
            .dirs
            .load_config()
            .map_err(|_| "provider_protocol_changed".to_string())?;
        let selected = config.providers.default_model(provider);
        match provider {
            ProviderId::Codex => {
                let _guard = self.inner.codex_lock.lock().await;
                let (mut client, _events) =
                    crate::codex_client::CodexAppServerClient::spawn_app_server(
                        self.inner.dirs.codex_workspace_dir(),
                        &self.inner.dirs,
                        None,
                        crate::codex_client::WorkspaceAccess::Read,
                        None,
                    )
                    .await
                    .map_err(|_| "provider_catalog_unavailable".to_string())?;
                let models = client
                    .list_models()
                    .await
                    .map_err(|_| "provider_catalog_unavailable".to_string())?;
                Ok(models
                    .into_iter()
                    .map(|model| ProviderModel {
                        provider,
                        model: model.model.clone(),
                        display_name: model.display_name,
                        description: model.description,
                        is_default: selected == Some(model.model.as_str()),
                        capabilities: ModelCapabilities {
                            vision: true,
                            tool_calls: true,
                            strict_structured_output: true,
                            reasoning_levels: model
                                .supported_reasoning_efforts
                                .iter()
                                .filter_map(|effort| reasoning_level(&effort.reasoning_effort))
                                .collect(),
                            native_compaction: true,
                            context_window: config
                                .providers
                                .codex
                                .large_context_models
                                .contains(&model.model)
                                .then_some(crate::codex_client::LARGE_CONTEXT_WINDOW_TOKENS as u64),
                            hosted_web_search: true,
                        },
                        privacy: None,
                    })
                    .collect())
            }
            ProviderId::ClaudeCode => Ok(crate::claude_client::provider_managed_models(selected)),
            ProviderId::Antigravity => {
                let credential =
                    crate::antigravity_auth::access_credential(&self.inner.dirs).await?;
                crate::antigravity_client::fetch_catalog(&self.inner.client, &credential, selected)
                    .await
            }
            ProviderId::OpencodeGo => {
                let mut key = self
                    .inner
                    .secrets
                    .read_secret(provider, "api-key")?
                    .ok_or_else(|| "provider_credential_missing".to_string())?;
                let result =
                    crate::opencode_go::fetch_catalog(&self.inner.client, &key, selected).await;
                key.zeroize();
                result
            }
            ProviderId::Ollama => selected
                .map(|model| crate::ollama::provider_model(model, selected))
                .transpose()
                .map(|model| model.into_iter().collect()),
        }
    }

    async fn version_channel(&self, provider: ProviderId) -> (Option<String>, Option<String>) {
        if provider != ProviderId::ClaudeCode {
            return (None, None);
        }
        let dirs = self.inner.dirs.clone();
        let state = tokio::task::spawn_blocking(move || crate::claude_auth::login_status(&dirs))
            .await
            .ok();
        let channel = Some(
            if self
                .inner
                .dirs
                .claude_bin_dir()
                .join("claude.exe")
                .is_file()
            {
                "app-managed · updates disabled"
            } else {
                "external executable"
            }
            .to_string(),
        );
        (state.and_then(|state| state.version), channel)
    }

    pub async fn settings_view(
        &self,
        provider: ProviderId,
    ) -> Result<ProviderSettingsView, String> {
        let config = self
            .inner
            .dirs
            .load_config()
            .map_err(|_| "provider_protocol_changed".to_string())?;
        let status = self.status_for(provider).await?;
        let models = if status.availability.is_ready() {
            self.catalog(provider).await?
        } else {
            Vec::new()
        };
        let (version, channel) = self.version_channel(provider).await;
        let base_url =
            (provider == ProviderId::Ollama).then(|| config.providers.ollama.base_url.clone());
        let has_api_key = matches!(provider, ProviderId::OpencodeGo | ProviderId::Ollama)
            && self
                .inner
                .secrets
                .read_secret(provider, "api-key")
                .is_ok_and(|key| key.is_some());
        Ok(ProviderSettingsView {
            provider,
            status,
            models,
            selected_model: config.providers.default_model(provider).map(str::to_string),
            selected_reasoning: config.providers.default_reasoning(provider),
            version,
            channel,
            base_url,
            has_api_key,
        })
    }

    pub async fn save_defaults(
        &self,
        provider: ProviderId,
        model: String,
        reasoning: Option<ReasoningSelection>,
        set_default_provider: bool,
    ) -> Result<ProviderSettingsView, String> {
        let _guard = self.inner.settings_lock.lock().await;
        let status = self.status_for(provider).await?;
        if !status.availability.is_ready() {
            return Err("provider_not_authenticated".to_string());
        }
        let model = if provider == ProviderId::Ollama {
            crate::ollama::validate_model(&model)?.to_string()
        } else {
            model
        };
        let models = if provider == ProviderId::Ollama {
            vec![crate::ollama::provider_model(&model, Some(model.as_str()))?]
        } else {
            self.catalog(provider).await?
        };
        let selected_model = models
            .iter()
            .find(|candidate| candidate.model == model)
            .ok_or_else(|| "provider_model_unavailable".to_string())?;
        if let Some(reasoning) = reasoning.as_ref() {
            let level = reasoning_level(&reasoning.level)
                .ok_or_else(|| "provider_capability_unsupported".to_string())?;
            if !selected_model
                .capabilities
                .reasoning_levels
                .contains(&level)
            {
                return Err("provider_capability_unsupported".to_string());
            }
        }
        let mut config = self
            .inner
            .dirs
            .load_config()
            .map_err(|_| "provider_protocol_changed".to_string())?;
        match provider {
            ProviderId::Codex => {
                config.providers.codex.default_model = Some(model.clone());
                config.providers.codex.default_reasoning = reasoning.clone();
            }
            ProviderId::ClaudeCode => {
                config.providers.claude_code.default_model = Some(model.clone());
                config.providers.claude_code.default_reasoning = reasoning.clone();
            }
            ProviderId::Antigravity => {
                config.providers.antigravity.default_model = Some(model.clone());
                config.providers.antigravity.default_reasoning = reasoning.clone();
            }
            ProviderId::OpencodeGo => {
                config.providers.opencode_go.default_model = Some(model.clone());
                config.providers.opencode_go.default_reasoning = reasoning.clone();
            }
            ProviderId::Ollama => {
                config.providers.ollama.default_model = Some(model.clone());
                config.providers.ollama.default_reasoning = reasoning.clone();
            }
        }
        if set_default_provider {
            config.default_provider = Some(provider);
        }
        self.inner
            .dirs
            .save_config(&config)
            .map_err(|_| "provider_protocol_changed".to_string())?;
        let (version, channel) = self.version_channel(provider).await;
        let base_url =
            (provider == ProviderId::Ollama).then(|| config.providers.ollama.base_url.clone());
        let has_api_key = matches!(provider, ProviderId::OpencodeGo | ProviderId::Ollama)
            && self
                .inner
                .secrets
                .read_secret(provider, "api-key")
                .is_ok_and(|key| key.is_some());
        Ok(ProviderSettingsView {
            provider,
            status: ProviderStatus {
                selected_as_default: config.default_provider == Some(provider),
                ..status
            },
            models,
            selected_model: Some(model),
            selected_reasoning: reasoning,
            version,
            channel,
            base_url,
            has_api_key,
        })
    }

    pub async fn save_base_url(
        &self,
        provider: ProviderId,
        base_url: String,
    ) -> Result<ProviderSettingsView, String> {
        if provider != ProviderId::Ollama {
            return Err("unsupported_operation".to_string());
        }
        if self.is_busy(provider) {
            return Err("provider_busy".to_string());
        }
        let base_url = crate::ollama::normalize_base_url(&base_url)?;
        {
            let _settings_guard = self.inner.settings_lock.lock().await;
            let _provider_guard = self.inner.ollama_lock.lock().await;
            let mut config = self
                .inner
                .dirs
                .load_config()
                .map_err(|_| "provider_protocol_changed".to_string())?;
            config.providers.ollama.base_url = base_url;
            self.inner
                .dirs
                .save_config(&config)
                .map_err(|_| "provider_protocol_changed".to_string())?;
        }
        self.settings_view(provider).await
    }

    async fn status_for(&self, provider: ProviderId) -> Result<ProviderStatus, String> {
        self.status_list()
            .await?
            .into_iter()
            .find(|status| status.provider == provider)
            .ok_or_else(|| "provider_protocol_changed".to_string())
    }
}

fn reasoning_level(value: &str) -> Option<ReasoningLevel> {
    match value {
        "none" => Some(ReasoningLevel::None),
        "minimal" => Some(ReasoningLevel::Minimal),
        "low" => Some(ReasoningLevel::Low),
        "medium" => Some(ReasoningLevel::Medium),
        "high" => Some(ReasoningLevel::High),
        "xhigh" => Some(ReasoningLevel::Xhigh),
        "max" => Some(ReasoningLevel::Max),
        _ => None,
    }
}

fn status_code(code: &str) -> ProviderStatusCode {
    match code {
        "provider_not_installed" => ProviderStatusCode::ProviderNotInstalled,
        "provider_not_authenticated" => ProviderStatusCode::ProviderNotAuthenticated,
        "provider_oauth_exchange_failed" => ProviderStatusCode::ProviderOauthExchangeFailed,
        "provider_oauth_client_unconfigured" => ProviderStatusCode::ProviderOauthClientUnconfigured,
        "provider_cloud_code_unauthorized" => ProviderStatusCode::ProviderCloudCodeUnauthorized,
        "provider_account_ineligible" => ProviderStatusCode::ProviderAccountIneligible,
        "provider_onboarding_required" => ProviderStatusCode::ProviderOnboardingRequired,
        "provider_auth_expired" => ProviderStatusCode::ProviderAuthExpired,
        "provider_credential_missing" => ProviderStatusCode::ProviderCredentialMissing,
        "provider_credential_store_unavailable" => {
            ProviderStatusCode::ProviderCredentialStoreUnavailable
        }
        "provider_import_unavailable" => ProviderStatusCode::ProviderImportUnavailable,
        "provider_catalog_unavailable" => ProviderStatusCode::ProviderCatalogUnavailable,
        "provider_model_unavailable" => ProviderStatusCode::ProviderModelUnavailable,
        "provider_capability_unsupported" => ProviderStatusCode::ProviderCapabilityUnsupported,
        "provider_endpoint_invalid" => ProviderStatusCode::ProviderEndpointInvalid,
        "provider_rate_limited" => ProviderStatusCode::ProviderRateLimited,
        "provider_quota_exhausted" => ProviderStatusCode::ProviderQuotaExhausted,
        "provider_transport_closed" => ProviderStatusCode::ProviderTransportClosed,
        "provider_structured_output_invalid" => ProviderStatusCode::ProviderStructuredOutputInvalid,
        "provider_busy" => ProviderStatusCode::ProviderBusy,
        "provider_cancelled" => ProviderStatusCode::ProviderCancelled,
        _ => ProviderStatusCode::ProviderProtocolChanged,
    }
}

fn emit_progress<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    provider: ProviderId,
    attempt_id: &str,
    stage: ProviderProgressStage,
    percent: Option<u8>,
    detail_code: Option<&str>,
) {
    let _ = app.emit(
        "provider_progress",
        ProviderProgressEvent {
            provider,
            attempt_id: attempt_id.to_string(),
            stage,
            percent,
            detail_code: detail_code.map(|code| {
                if code.starts_with("provider_") {
                    code.to_string()
                } else {
                    "provider_protocol_changed".to_string()
                }
            }),
        },
    );
}

#[tauri::command]
pub async fn provider_settings(
    service: tauri::State<'_, ProviderService>,
    provider: ProviderId,
) -> Result<ProviderSettingsView, String> {
    service.settings_view(provider).await
}

#[tauri::command]
pub async fn provider_status_list(
    service: tauri::State<'_, ProviderService>,
) -> Result<Vec<ProviderStatus>, String> {
    service.status_list().await
}

#[tauri::command]
pub async fn provider_install(
    app: tauri::AppHandle,
    service: tauri::State<'_, ProviderService>,
    provider: ProviderId,
) -> Result<ProviderStatus, String> {
    service.install(&app, provider).await
}

#[tauri::command]
pub async fn provider_login_start(
    app: tauri::AppHandle,
    service: tauri::State<'_, ProviderService>,
    provider: ProviderId,
) -> Result<String, String> {
    service.login_start(app, provider).await
}

#[tauri::command]
pub async fn provider_login_status(
    service: tauri::State<'_, ProviderService>,
    provider: ProviderId,
) -> Result<ProviderStatus, String> {
    service.status_for(provider).await
}

#[tauri::command]
pub async fn provider_login_cancel(
    service: tauri::State<'_, ProviderService>,
    provider: ProviderId,
    attempt_id: String,
) -> Result<(), String> {
    service.cancel_login(provider, &attempt_id)
}

#[tauri::command]
pub async fn provider_credential_import(
    service: tauri::State<'_, ProviderService>,
    provider: ProviderId,
) -> Result<ProviderStatus, String> {
    service.import_credential(provider).await
}

#[tauri::command]
pub async fn provider_api_key_save(
    service: tauri::State<'_, ProviderService>,
    provider: ProviderId,
    key: String,
) -> Result<ProviderStatus, String> {
    service.save_api_key(provider, key).await
}

#[tauri::command]
pub async fn provider_base_url_save(
    service: tauri::State<'_, ProviderService>,
    provider: ProviderId,
    base_url: String,
) -> Result<ProviderSettingsView, String> {
    service.save_base_url(provider, base_url).await
}

#[tauri::command]
pub async fn provider_logout(
    service: tauri::State<'_, ProviderService>,
    provider: ProviderId,
) -> Result<ProviderStatus, String> {
    service.logout(provider).await
}

#[tauri::command]
pub async fn provider_catalog(
    service: tauri::State<'_, ProviderService>,
    provider: ProviderId,
) -> Result<Vec<ProviderModel>, String> {
    service.catalog(provider).await
}

#[tauri::command]
pub async fn provider_defaults_save(
    service: tauri::State<'_, ProviderService>,
    provider: ProviderId,
    model: String,
    reasoning: Option<ReasoningSelection>,
    set_default_provider: bool,
) -> Result<ProviderSettingsView, String> {
    service
        .save_defaults(provider, model, reasoning, set_default_provider)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_taxonomy_never_returns_raw_provider_text() {
        assert_eq!(
            status_code("token=secret"),
            ProviderStatusCode::ProviderProtocolChanged
        );
        assert_eq!(
            status_code("provider_rate_limited"),
            ProviderStatusCode::ProviderRateLimited
        );
        assert_eq!(
            status_code("provider_credential_store_unavailable"),
            ProviderStatusCode::ProviderCredentialStoreUnavailable
        );
        assert_eq!(
            status_code("provider_cloud_code_unauthorized"),
            ProviderStatusCode::ProviderCloudCodeUnauthorized
        );
        assert_eq!(
            status_code("provider_oauth_client_unconfigured"),
            ProviderStatusCode::ProviderOauthClientUnconfigured
        );
        assert_eq!(
            status_code("provider_endpoint_invalid"),
            ProviderStatusCode::ProviderEndpointInvalid
        );
    }

    #[test]
    fn reasoning_levels_are_exact_not_guessed() {
        assert_eq!(reasoning_level("xhigh"), Some(ReasoningLevel::Xhigh));
        assert_eq!(reasoning_level("ultra"), None);
    }
    #[tokio::test]
    async fn logout_is_rejected_while_the_provider_owns_work() {
        let base = std::env::temp_dir().join(format!("eud-provider-busy-{}", uuid::Uuid::new_v4()));
        let dirs = crate::config::DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        let service = ProviderService::new(dirs).unwrap();
        let lease = service.enter_busy(ProviderId::OpencodeGo);
        assert_eq!(
            service.logout(ProviderId::OpencodeGo).await,
            Err("provider_busy".to_string())
        );
        drop(lease);
        std::fs::remove_dir_all(base).ok();
    }
}
