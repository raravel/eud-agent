mod adapter;

pub use adapter::ProductionClaudeCodeAdapter;

use crate::provider::{ModelCapabilities, ProviderId, ProviderModel};

const CLAUDE_PROVIDER_DEFAULT: &str = "provider-default";

pub fn provider_managed_models(selected: Option<&str>) -> Vec<ProviderModel> {
    vec![ProviderModel {
        provider: ProviderId::ClaudeCode,
        model: CLAUDE_PROVIDER_DEFAULT.to_string(),
        display_name: "Claude Code 기본 모델".to_string(),
        description: "Claude Code가 현재 계정과 배포 기준으로 모델을 선택합니다.".to_string(),
        is_default: selected == Some(CLAUDE_PROVIDER_DEFAULT),
        capabilities: ModelCapabilities {
            vision: true,
            tool_calls: true,
            strict_structured_output: true,
            reasoning_levels: Vec::new(),
            native_compaction: true,
            context_window: None,
            hosted_web_search: false,
        },
        privacy: None,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_exposes_cli_managed_default() {
        let models = provider_managed_models(Some(CLAUDE_PROVIDER_DEFAULT));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, CLAUDE_PROVIDER_DEFAULT);
        assert!(models[0].is_default);
        assert!(models[0].capabilities.reasoning_levels.is_empty());
    }
}
