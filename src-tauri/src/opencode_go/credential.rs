use super::*;

pub(super) enum OpenCodeGoCredential {
    Stored(ProviderSecretStore),
    #[cfg(test)]
    Fixture(Zeroizing<String>),
}

impl OpenCodeGoCredential {
    pub(super) fn read(&self) -> Result<Zeroizing<String>, ProviderRuntimeError> {
        match self {
            Self::Stored(store) => store
                .read_secret(ProviderId::OpencodeGo, "api-key")
                .map_err(ProviderRuntimeError::Transport)?
                .map(Zeroizing::new)
                .ok_or_else(|| {
                    ProviderRuntimeError::Transport("provider_credential_missing".to_string())
                }),
            #[cfg(test)]
            Self::Fixture(secret) => Ok(secret.clone()),
        }
    }
}
