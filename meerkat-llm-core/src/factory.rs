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
    }
}
