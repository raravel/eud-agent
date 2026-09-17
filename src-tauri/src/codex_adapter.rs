use std::path::PathBuf;

use tokio::process::{ChildStdin, ChildStdout};

mod compaction;
#[cfg(all(test, windows))]
mod process_fixture;
#[cfg(all(test, windows))]
mod process_tests;
mod step;
mod turn;

use turn::transport_error;

use crate::{
    codex_client::{AppServerEvent, CodexAppServerClient, CodexLaunchConfig, CodexModelSelection},
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{
        AdapterEvent, AdapterFuture, AdapterLoopKind, AdapterStepOutcome, AdapterStepRequest,
        ProviderAdapter, ProviderRuntimeError, RunIdentity, WorkspaceAccess,
    },
};

type NativeClient = CodexAppServerClient<ChildStdout, ChildStdin>;

struct NativeOperationGuard<'a> {
    client: &'a mut Option<NativeClient>,
    events: &'a mut Option<tokio::sync::mpsc::Receiver<AppServerEvent>>,
    completed: bool,
}

impl Drop for NativeOperationGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            *self.client = None;
            *self.events = None;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClientKey {
    cwd: PathBuf,
    access: WorkspaceAccess,
    mcp_endpoint: Option<String>,
    native_tools_enabled: bool,
    /// Session-private `TEMP`/`TMP` target baked into the spawned process.
    temp_dir: Option<PathBuf>,
}

pub struct CodexAdapter {
    fallback_cwd: PathBuf,
    launch: CodexLaunchConfig,
    client_key: Option<ClientKey>,
    client: Option<NativeClient>,
    events: Option<tokio::sync::mpsc::Receiver<AppServerEvent>>,
    committed_thread_id: Option<String>,
    native_state_trusted: bool,
}

impl CodexAdapter {
    pub fn new(fallback_cwd: PathBuf, launch: CodexLaunchConfig) -> Self {
        Self {
            fallback_cwd,
            launch,
            client_key: None,
            client: None,
            events: None,
            committed_thread_id: None,
            native_state_trusted: true,
        }
    }

    async fn ensure_foreground_client(
        &mut self,
        key: ClientKey,
        model: CodexModelSelection,
        large_context: bool,
    ) -> Result<(), ProviderRuntimeError> {
        if !self.native_state_trusted {
            self.invalidate_native_state();
        }
        let reusable = self.client_key.as_ref() == Some(&key)
            && self
                .client
                .as_ref()
                .is_some_and(|client| !client.is_transport_closed());
        if !reusable {
            self.client = None;
            self.events = None;
            let (client, events) = CodexAppServerClient::spawn_app_server(
                &key.cwd,
                &self.launch,
                key.mcp_endpoint.as_deref(),
                key.access,
                key.native_tools_enabled,
                key.temp_dir.as_deref(),
            )
            .await
            .map_err(transport_error)?;
            if let Some(thread_id) = self.committed_thread_id.clone() {
                client.set_thread_id(thread_id).await;
            }
            self.client = Some(client);
            self.events = Some(events);
            self.client_key = Some(key);
        }
        let client = self.client.as_mut().ok_or_else(|| {
            ProviderRuntimeError::Transport("Codex app-server client is unavailable".to_string())
        })?;
        client.set_model_selection(Some(model));
        client.set_large_context_enabled(large_context);
        Ok(())
    }

    fn invalidate_native_state(&mut self) {
        self.client = None;
        self.events = None;
        self.client_key = None;
        self.committed_thread_id = None;
        self.native_state_trusted = true;
    }
}

impl ProviderAdapter for CodexAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn loop_kind(&self) -> AdapterLoopKind {
        AdapterLoopKind::NativeSession
    }

    fn run_step<'a>(
        &'a mut self,
        request: AdapterStepRequest,
        event_tx: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<AdapterStepOutcome, ProviderRuntimeError>> {
        self.execute_step(request, event_tx)
    }

    fn interrupt(
        &mut self,
        _identity: &RunIdentity,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            self.invalidate_native_state();
            Ok(())
        })
    }

    fn reset(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            self.invalidate_native_state();
            Ok(())
        })
    }

    fn compact<'a>(
        &'a mut self,
        identity: RunIdentity,
        cancellation: tokio::sync::watch::Receiver<u64>,
        event_tx: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<ProviderConversationState, ProviderRuntimeError>> {
        self.compact_native(identity, cancellation, event_tx)
    }

    fn seed(
        &mut self,
        state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async move {
            let ProviderConversationState::Codex { thread_id } = state else {
                return Err(ProviderRuntimeError::InvalidBinding(
                    "Codex adapter received incompatible conversation state".to_string(),
                ));
            };
            self.invalidate_native_state();
            self.committed_thread_id = thread_id;
            Ok(())
        })
    }

    fn prepare_compaction<'a>(
        &'a mut self,
        request: &'a crate::provider_runtime::CompactionRequest,
    ) -> AdapterFuture<'a, Result<(), ProviderRuntimeError>> {
        self.prepare_native_compaction(request)
    }
}
