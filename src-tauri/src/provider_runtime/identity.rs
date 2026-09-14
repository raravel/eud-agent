use super::ProviderRuntimeError;
use crate::{
    provider::{
        ModelCapabilities, ProviderBinding, ProviderConversationState, ProviderId,
        ReasoningSelection,
    },
    session::SessionKind,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(u64);

impl RunId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunIdentity {
    pub session_id: String,
    pub run_id: RunId,
    pub request_id: String,
    pub session_kind: SessionKind,
    pub cancellation_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingSnapshot {
    pub provider: ProviderId,
    pub model: String,
    pub reasoning: Option<ReasoningSelection>,
    pub base_url: Option<String>,
    pub capabilities: Option<ModelCapabilities>,
    pub conversation: ProviderConversationState,
}

impl BindingSnapshot {
    pub fn from_binding(
        binding: &ProviderBinding,
        capabilities: Option<ModelCapabilities>,
    ) -> Result<Self, ProviderRuntimeError> {
        binding
            .validate()
            .map_err(ProviderRuntimeError::InvalidBinding)?;
        Ok(Self {
            provider: binding.provider,
            model: binding.model.clone(),
            reasoning: binding.reasoning.clone(),
            base_url: binding.base_url.clone(),
            capabilities,
            conversation: binding.conversation.clone(),
        })
    }

    pub fn to_binding(&self) -> ProviderBinding {
        ProviderBinding {
            provider: self.provider,
            model: self.model.clone(),
            reasoning: self.reasoning.clone(),
            base_url: self.base_url.clone(),
            conversation: self.conversation.clone(),
        }
    }
}
