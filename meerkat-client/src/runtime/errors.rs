//! Error types for the provider-runtime layer.

use thiserror::Error;

use crate::error::LlmError;

/// Binding-validation errors raised by a provider runtime.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderBindingError {
    #[error("unsupported combination: backend={backend} auth={auth}")]
    UnsupportedCombination { backend: String, auth: String },
    #[error("unknown backend_kind: {0}")]
    UnknownBackendKind(String),
    #[error("unknown auth_method: {0}")]
    UnknownAuthMethod(String),
    #[error("provider mismatch between backend and auth")]
    ProviderMismatch,
    #[error("binding missing required default: {0}")]
    MissingRequiredDefault(&'static str),
}

/// Resolution-phase errors (fetching credentials, invoking external resolvers,
/// propagating generic `AuthError`).
#[derive(Debug, Error)]
pub enum ProviderAuthError {
    #[error("auth error: {0}")]
    Auth(#[from] meerkat_core::AuthError),
    #[error("binding validation failed: {0}")]
    Binding(ProviderBindingError),
    #[error("source resolution failed: {0}")]
    SourceResolutionFailed(String),
    #[error("external resolver not registered: {0}")]
    ExternalResolverMissing(String),
    #[error("no runtime registered for provider: {0:?}")]
    NoRuntimeRegistered(meerkat_core::Provider),
}

impl PartialEq for ProviderAuthError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Auth(a), Self::Auth(b)) => a == b,
            (Self::Binding(a), Self::Binding(b)) => a == b,
            (Self::SourceResolutionFailed(a), Self::SourceResolutionFailed(b)) => a == b,
            (Self::ExternalResolverMissing(a), Self::ExternalResolverMissing(b)) => a == b,
            (Self::NoRuntimeRegistered(a), Self::NoRuntimeRegistered(b)) => a == b,
            _ => false,
        }
    }
}

/// Client-construction errors (feature-gated combinations, upstream LlmError
/// from provider constructors, dynamic-authorizer-in-shim-mode rejection).
#[derive(Debug, Error)]
pub enum ProviderClientError {
    #[error("missing feature: {0}")]
    MissingFeature(&'static str),
    #[error("provider client init failed: {0}")]
    ClientInit(#[from] LlmError),
    #[error("no credential material (auth_lease is empty)")]
    NoCredentialMaterial,
    #[error("dynamic authorizer requires Phase 3 owned HTTP path")]
    DynamicAuthorizerNotYetSupportedInShimMode,
    #[error("invalid base_url: {0}")]
    InvalidBaseUrl(String),
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn binding_error_variants_display() {
        for err in [
            ProviderBindingError::UnsupportedCombination {
                backend: "b".into(),
                auth: "a".into(),
            },
            ProviderBindingError::UnknownBackendKind("x".into()),
            ProviderBindingError::UnknownAuthMethod("y".into()),
            ProviderBindingError::ProviderMismatch,
            ProviderBindingError::MissingRequiredDefault("base_url"),
        ] {
            assert!(!err.to_string().is_empty(), "{err:?}");
        }
    }

    #[test]
    fn client_init_from_llm_error() {
        // S1a-derived construction test: proves ProviderClientError::ClientInit
        // carries From<LlmError>. Does NOT attempt to drive AnthropicClient
        // into its real Err arm (see S1d — with_base_url swallows errors).
        let e: ProviderClientError = LlmError::InvalidApiKey.into();
        assert!(matches!(e, ProviderClientError::ClientInit(_)));
    }

    #[test]
    fn auth_error_from_core() {
        let e: ProviderAuthError = meerkat_core::AuthError::MissingSecret.into();
        assert!(matches!(e, ProviderAuthError::Auth(_)));
    }
}
