mod adapter;
mod catalog;
mod codec;
mod schema;

use crate::provider::{ModelCapabilities, ProviderId, ProviderModel};

pub(crate) use adapter::AntigravityAdapter;
pub use catalog::fetch_catalog;
use schema::normalize_cca_parameters;

const STRUCTURED_TOOL: &str = "submit_structured_result";

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveAntigravityModel {
    id: String,
    display_name: String,
    supports_images: bool,
    supports_thinking: bool,
    thinking_budget: Option<u64>,
    context_window: Option<u64>,
    max_output_tokens: Option<u64>,
    api_provider: Option<String>,
    model_provider: Option<String>,
}

impl LiveAntigravityModel {
    fn provider_model(&self, selected: Option<&str>) -> ProviderModel {
        let description = match (&self.model_provider, &self.api_provider) {
            (Some(model_provider), Some(api_provider)) => {
                format!("{model_provider} · {api_provider}")
            }
            (Some(provider), None) | (None, Some(provider)) => provider.clone(),
            (None, None) => "Antigravity live catalog".to_string(),
        };
        ProviderModel {
            provider: ProviderId::Antigravity,
            model: self.id.clone(),
            display_name: self.display_name.clone(),
            description,
            is_default: selected == Some(self.id.as_str()),
            capabilities: ModelCapabilities {
                vision: self.supports_images,
                tool_calls: true,
                strict_structured_output: true,
                reasoning_levels: Vec::new(),
                native_compaction: false,
                context_window: self.context_window,
                hosted_web_search: false,
            },
            privacy: None,
        }
    }
}

#[cfg(test)]
mod tests;
