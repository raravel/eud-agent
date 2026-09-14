use super::{ProviderAdapter, ProviderRuntimeError};
use crate::provider::{ProviderBinding, ProviderId};
use std::path::PathBuf;

pub(crate) fn production_adapter(
    binding: &ProviderBinding,
    dirs: &crate::config::DataDirs,
    fallback_cwd: PathBuf,
) -> Result<Box<dyn ProviderAdapter>, ProviderRuntimeError> {
    binding
        .validate()
        .map_err(ProviderRuntimeError::InvalidBinding)?;
    match binding.provider {
        ProviderId::Codex => {
            let launch = crate::codex_client::resolve_codex_launch_config(dirs)
                .map_err(|error| ProviderRuntimeError::Transport(error.to_string()))?;
            Ok(Box::new(crate::codex_adapter::CodexAdapter::new(
                fallback_cwd,
                launch,
            )))
        }
        ProviderId::ClaudeCode => {
            let config = dirs
                .load_config()
                .map_err(|_| ProviderRuntimeError::Protocol("provider_protocol_changed".into()))?;
            let executable = crate::claude_auth::resolve_claude_cmd(dirs, &config)
                .map_err(|error| ProviderRuntimeError::Transport(error.to_string()))?;
            let adapter = crate::claude_client::ProductionClaudeCodeAdapter::new(
                binding.model.clone(),
                executable,
                dirs.claude_config_dir(),
            )?;
            Ok(Box::new(adapter))
        }
        ProviderId::Antigravity => Ok(Box::new(
            crate::antigravity_client::AntigravityAdapter::new(
                binding.model.clone(),
                crate::antigravity_auth::AntigravityAuthHandle::new(dirs.clone()),
            )?,
        )),
        ProviderId::OpencodeGo => {
            let secrets = crate::provider_secrets::ProviderSecretStore::new(dirs.clone())
                .map_err(ProviderRuntimeError::Transport)?;
            Ok(Box::new(crate::opencode_go::OpenCodeGoAdapter::new(
                secrets,
            )?))
        }
        ProviderId::Ollama => {
            let secrets = crate::provider_secrets::ProviderSecretStore::new(dirs.clone())
                .map_err(ProviderRuntimeError::Transport)?;
            let api_key = secrets
                .read_secret(ProviderId::Ollama, "api-key")
                .map_err(ProviderRuntimeError::Transport)?;
            Ok(Box::new(crate::ollama::ProductionOllamaAdapter::new(
                api_key,
            )?))
        }
    }
}
