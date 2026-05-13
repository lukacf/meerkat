//! LLM client factory errors.
//!
//! Plan §6.7 + §6.8 deleted the legacy factory trait / default impl /
//! provider enum that previously lived in this module. Client
//! construction now flows exclusively through the provider runtime
//! registry (`ProviderRuntimeRegistry::resolve` →
//! `ProviderRuntime::build_client`), and the canonical provider
//! identity lives on `meerkat_core::Provider`. This module retains
//! only the error type, still used by the facade's
//! `build_llm_client_for_identity` path.

/// Error types surfaced by LLM-client construction.
#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    /// The requested provider is not supported.
    #[error("Unsupported provider: {0}")]
    UnsupportedProvider(String),

    /// Client creation failed (including missing credentials).
    ///
    /// Phase 6.3 collapsed the legacy `MissingApiKey(provider)` variant
    /// into this broader failure variant; producers emit a message of
    /// the form "Missing API key for provider: <p>" when credentials
    /// are absent, preserving downstream text-based error parsing.
    #[error("Failed to create client: {0}")]
    ClientCreationFailed(String),

    /// Provider-runtime connection/auth resolution failed with a typed cause.
    #[error("Provider connection resolution failed: {0}")]
    ProviderConnection(#[from] crate::provider_runtime::ProviderAuthError),

    /// Provider-runtime client construction failed with a typed cause.
    #[error("Provider client creation failed: {0}")]
    ProviderClient(#[from] crate::provider_runtime::ProviderClientError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_error_display_shapes() {
        let err = FactoryError::UnsupportedProvider("test".into());
        assert_eq!(err.to_string(), "Unsupported provider: test");

        let err =
            FactoryError::ClientCreationFailed("Missing API key for provider: anthropic".into());
        assert_eq!(
            err.to_string(),
            "Failed to create client: Missing API key for provider: anthropic"
        );

        let err = FactoryError::ClientCreationFailed("timeout".into());
        assert_eq!(err.to_string(), "Failed to create client: timeout");

        let err =
            FactoryError::ProviderConnection(crate::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::MissingSecret,
            ));
        assert!(matches!(err, FactoryError::ProviderConnection(_)));

        let err = FactoryError::ProviderClient(
            crate::provider_runtime::ProviderClientError::NoCredentialMaterial,
        );
        assert!(matches!(err, FactoryError::ProviderClient(_)));
    }
}
