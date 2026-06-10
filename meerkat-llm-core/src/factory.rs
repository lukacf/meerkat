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
    /// The requested provider/transport combination is not supported. The
    /// payload is a human-facing reason (e.g. "self_hosted requires the openai
    /// feature"), not a parsed identity — display-only.
    #[error("Unsupported provider: {0}")]
    UnsupportedProvider(String),

    /// Provider auth/binding resolution failed, carrying the typed
    /// [`ProviderAuthError`] cause so callers branch on the auth/binding/
    /// resolution kind rather than re-parsing a flattened string.
    #[error("provider auth resolution failed: {0}")]
    ProviderAuth(#[from] crate::provider_runtime::errors::ProviderAuthError),

    /// Building the concrete LLM client from a resolved connection failed,
    /// carrying the typed [`ProviderClientError`] cause rather than a flattened
    /// string.
    #[error("provider client build failed: {0}")]
    ClientBuild(#[from] crate::provider_runtime::errors::ProviderClientError),

    /// Realm/binding connection-target selection failed, carrying the typed
    /// [`meerkat_core::ConnectionTargetError`] cause so callers branch on the
    /// realm/binding selection fault instead of re-parsing a flattened string.
    #[error("connection target resolution failed: {0}")]
    ConnectionTarget(#[from] meerkat_core::ConnectionTargetError),

    /// The persisted TokenStore backing OAuth-backed bindings failed to open.
    ///
    /// A store that cannot be opened is a fault, not an absence of
    /// credentials: collapsing it to "no store attached" would launder the
    /// open failure into `AuthError::InteractiveLoginRequired` at the first
    /// OAuth resolution. The typed open fault is carried (shared via `Arc`
    /// because the factory holds the original) and propagates at the
    /// resolution seam.
    #[cfg(not(target_arch = "wasm32"))]
    #[error("token store unavailable: {0}")]
    TokenStore(std::sync::Arc<meerkat_core::auth::TokenStoreError>),

    /// Client creation failed for a reason with no richer typed cause (config
    /// load, unknown model, lower-level client-build error). The payload is an
    /// opaque, display-only message — no production code branches on its text.
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

        let err = FactoryError::ClientCreationFailed("timeout".into());
        assert_eq!(err.to_string(), "Failed to create client: timeout");
    }

    #[test]
    fn provider_auth_cause_is_carried_typed_not_flattened() {
        // A provider auth/binding resolution failure is carried as the typed
        // `ProviderAuthError` (via `#[from]`), not flattened into a
        // `ClientCreationFailed` string — callers can match the typed cause.
        let cause = crate::provider_runtime::errors::ProviderAuthError::NoRuntimeRegistered(
            meerkat_core::Provider::OpenAI,
        );
        let err: FactoryError = cause.into();
        assert!(matches!(err, FactoryError::ProviderAuth(_)));
    }
}
