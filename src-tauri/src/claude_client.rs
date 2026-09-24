mod adapter;
mod catalog;
mod credentials;

pub use adapter::ProductionClaudeCodeAdapter;
pub(crate) use catalog::fetch_catalog;
#[cfg(test)]
use catalog::CLAUDE_PROVIDER_DEFAULT;
pub use catalog::{bound_model, provider_default_model};
pub(crate) use credentials::{access_token, stored_credential_present};

use crate::provider::ProviderModel;

/// Catalog without network access: the CLI-selected default only.
pub fn provider_managed_models(selected: Option<&str>) -> Vec<ProviderModel> {
    vec![provider_default_model(selected)]
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
