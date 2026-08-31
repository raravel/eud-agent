//! Closed five-provider domain model shared by config, sessions, IPC, and engines.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderId {
    Codex,
    ClaudeCode,
    Antigravity,
    OpencodeGo,
    Ollama,
}

impl ProviderId {
    pub const ALL: [Self; 5] = [
        Self::Codex,
        Self::ClaudeCode,
        Self::Antigravity,
        Self::OpencodeGo,
        Self::Ollama,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
            Self::Antigravity => "Antigravity",
            Self::OpencodeGo => "OpenCode Go",
            Self::Ollama => "Ollama",
        }
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let id = match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::Antigravity => "antigravity",
            Self::OpencodeGo => "opencode-go",
            Self::Ollama => "ollama",
        };
        formatter.write_str(id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReasoningLevel {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReasoningSelection {
    pub level: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelCapabilities {
    pub vision: bool,
    pub tool_calls: bool,
    pub strict_structured_output: bool,
    pub reasoning_levels: Vec<ReasoningLevel>,
    pub native_compaction: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    pub hosted_web_search: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DataTrainingPolicy {
    NotUsed,
    Used,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelPrivacy {
    pub training: DataTrainingPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u32>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderModel {
    pub provider: ProviderId,
    pub model: String,
    pub display_name: String,
    pub description: String,
    pub is_default: bool,
    pub capabilities: ModelCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privacy: Option<ModelPrivacy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderAvailability {
    Ready,
    NeedsInstall,
    NeedsAuthentication,
    NeedsCredential,
    Degraded,
    Unavailable,
}

impl ProviderAvailability {
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatusCode {
    ProviderNotInstalled,
    ProviderNotAuthenticated,
    ProviderOauthExchangeFailed,
    ProviderOauthClientUnconfigured,
    ProviderCloudCodeUnauthorized,
    ProviderAccountIneligible,
    ProviderAuthExpired,
    ProviderCredentialMissing,
    ProviderCredentialStoreUnavailable,
    ProviderImportUnavailable,
    ProviderCatalogUnavailable,
    ProviderModelUnavailable,
    ProviderCapabilityUnsupported,
    ProviderRateLimited,
    ProviderQuotaExhausted,
    ProviderEndpointInvalid,
    ProviderProtocolChanged,
    ProviderTransportClosed,
    ProviderStructuredOutputInvalid,
    ProviderBusy,
    ProviderCancelled,
    ProviderOnboardingRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderStatus {
    pub provider: ProviderId,
    pub availability: ProviderAvailability,
    pub selected_as_default: bool,
    pub can_install: bool,
    pub can_import: bool,
    pub experimental: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_code: Option<ProviderStatusCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProviderConversationState {
    Codex { thread_id: Option<String> },
    ClaudeCode { session_id: Option<String> },
    Antigravity { transcript_revision: u64 },
    OpencodeGo { transcript_revision: u64 },
    Ollama { transcript_revision: u64 },
}

impl ProviderConversationState {
    pub const fn empty(provider: ProviderId) -> Self {
        match provider {
            ProviderId::Codex => Self::Codex { thread_id: None },
            ProviderId::ClaudeCode => Self::ClaudeCode { session_id: None },
            ProviderId::Antigravity => Self::Antigravity {
                transcript_revision: 0,
            },
            ProviderId::OpencodeGo => Self::OpencodeGo {
                transcript_revision: 0,
            },
            ProviderId::Ollama => Self::Ollama {
                transcript_revision: 0,
            },
        }
    }

    pub const fn provider(&self) -> ProviderId {
        match self {
            Self::Codex { .. } => ProviderId::Codex,
            Self::ClaudeCode { .. } => ProviderId::ClaudeCode,
            Self::Antigravity { .. } => ProviderId::Antigravity,
            Self::OpencodeGo { .. } => ProviderId::OpencodeGo,
            Self::Ollama { .. } => ProviderId::Ollama,
        }
    }

    pub fn conversation_key(&self) -> Option<String> {
        match self {
            Self::Codex { thread_id } => thread_id.clone(),
            Self::ClaudeCode { session_id } => session_id.clone(),
            Self::Antigravity {
                transcript_revision,
            }
            | Self::OpencodeGo {
                transcript_revision,
            }
            | Self::Ollama {
                transcript_revision,
            } => (*transcript_revision > 0).then(|| transcript_revision.to_string()),
        }
    }

    pub fn is_started(&self) -> bool {
        self.conversation_key().is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderBinding {
    pub provider: ProviderId,
    pub model: String,
    pub reasoning: Option<ReasoningSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub conversation: ProviderConversationState,
}

impl ProviderBinding {
    pub fn new(
        provider: ProviderId,
        model: String,
        reasoning: Option<ReasoningSelection>,
    ) -> Result<Self, String> {
        Self::new_with_base_url(provider, model, reasoning, None)
    }

    pub fn new_with_base_url(
        provider: ProviderId,
        model: String,
        reasoning: Option<ReasoningSelection>,
        base_url: Option<String>,
    ) -> Result<Self, String> {
        let binding = Self {
            provider,
            model,
            reasoning,
            base_url,
            conversation: ProviderConversationState::empty(provider),
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.provider != self.conversation.provider() {
            return Err("provider binding conversation variant mismatch".to_string());
        }
        if self.model.trim().is_empty() {
            return Err("provider binding model is empty".to_string());
        }
        match (self.provider, self.base_url.as_deref()) {
            (ProviderId::Ollama, Some(base_url))
                if !base_url.trim().is_empty()
                    && base_url.len() <= 2_048
                    && !base_url.chars().any(char::is_control) => {}
            (ProviderId::Ollama, _) => {
                return Err("ollama provider binding base URL is invalid".to_string())
            }
            (_, None) => {}
            (_, Some(_)) => {
                return Err("provider binding base URL is only valid for Ollama".to_string())
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexProviderSettings {
    pub executable_override: Option<String>,
    pub default_model: Option<String>,
    pub default_reasoning: Option<ReasoningSelection>,
    pub large_context_models: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeCodeProviderSettings {
    pub executable_override: Option<String>,
    pub default_model: Option<String>,
    pub default_reasoning: Option<ReasoningSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AntigravityProviderSettings {
    pub default_model: Option<String>,
    pub default_reasoning: Option<ReasoningSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenCodeGoProviderSettings {
    pub default_model: Option<String>,
    pub default_reasoning: Option<ReasoningSelection>,
}

pub const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";

fn default_ollama_base_url() -> String {
    DEFAULT_OLLAMA_BASE_URL.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OllamaProviderSettings {
    #[serde(default = "default_ollama_base_url")]
    pub base_url: String,
    pub default_model: Option<String>,
    pub default_reasoning: Option<ReasoningSelection>,
}

impl Default for OllamaProviderSettings {
    fn default() -> Self {
        Self {
            base_url: default_ollama_base_url(),
            default_model: None,
            default_reasoning: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSettings {
    pub codex: CodexProviderSettings,
    pub claude_code: ClaudeCodeProviderSettings,
    pub antigravity: AntigravityProviderSettings,
    pub opencode_go: OpenCodeGoProviderSettings,
    #[serde(default)]
    pub ollama: OllamaProviderSettings,
}

impl ProviderSettings {
    pub fn default_model(&self, provider: ProviderId) -> Option<&str> {
        match provider {
            ProviderId::Codex => self.codex.default_model.as_deref(),
            ProviderId::ClaudeCode => self.claude_code.default_model.as_deref(),
            ProviderId::Antigravity => self.antigravity.default_model.as_deref(),
            ProviderId::OpencodeGo => self.opencode_go.default_model.as_deref(),
            ProviderId::Ollama => self.ollama.default_model.as_deref(),
        }
    }

    pub fn default_reasoning(&self, provider: ProviderId) -> Option<ReasoningSelection> {
        match provider {
            ProviderId::Codex => self.codex.default_reasoning.clone(),
            ProviderId::ClaudeCode => self.claude_code.default_reasoning.clone(),
            ProviderId::Antigravity => self.antigravity.default_reasoning.clone(),
            ProviderId::OpencodeGo => self.opencode_go.default_reasoning.clone(),
            ProviderId::Ollama => self.ollama.default_reasoning.clone(),
        }
    }
}
pub fn default_binding(config: &crate::config::Config) -> Result<ProviderBinding, String> {
    let provider = config
        .default_provider
        .ok_or_else(|| "provider_default_required".to_string())?;
    let model = config
        .providers
        .default_model(provider)
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| "provider_model_unavailable".to_string())?
        .to_string();
    ProviderBinding::new_with_base_url(
        provider,
        model,
        config.providers.default_reasoning(provider),
        (provider == ProviderId::Ollama).then(|| config.providers.ollama.base_url.clone()),
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSettingsView {
    pub provider: ProviderId,
    pub status: ProviderStatus,
    pub models: Vec<ProviderModel>,
    pub selected_model: Option<String>,
    pub selected_reasoning: Option<ReasoningSelection>,
    pub version: Option<String>,
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub has_api_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionModelSettings {
    pub provider: ProviderId,
    pub models: Vec<ProviderModel>,
    pub selected_model: String,
    pub selected_reasoning: Option<ReasoningSelection>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ids_are_exact_and_unknown_values_fail() {
        let values = ProviderId::ALL
            .into_iter()
            .map(|provider| serde_json::to_string(&provider).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            [
                "\"codex\"",
                "\"claude-code\"",
                "\"antigravity\"",
                "\"opencode-go\"",
                "\"ollama\"",
            ]
        );
        assert!(serde_json::from_str::<ProviderId>("\"other\"").is_err());
    }

    #[test]
    fn binding_rejects_mismatched_conversation_variant() {
        let binding = ProviderBinding {
            provider: ProviderId::Codex,
            model: "gpt-test".to_string(),
            reasoning: None,
            base_url: None,
            conversation: ProviderConversationState::ClaudeCode { session_id: None },
        };
        assert_eq!(
            binding.validate(),
            Err("provider binding conversation variant mismatch".to_string())
        );
    }

    #[test]
    fn ollama_binding_requires_a_pinned_base_url() {
        assert_eq!(
            ProviderBinding::new(ProviderId::Ollama, "qwen3:8b".to_string(), None),
            Err("ollama provider binding base URL is invalid".to_string())
        );
        let binding = ProviderBinding::new_with_base_url(
            ProviderId::Ollama,
            "qwen3:8b".to_string(),
            None,
            Some(DEFAULT_OLLAMA_BASE_URL.to_string()),
        )
        .unwrap();
        assert_eq!(binding.base_url.as_deref(), Some(DEFAULT_OLLAMA_BASE_URL));
    }
}
