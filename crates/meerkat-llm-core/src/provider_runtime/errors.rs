//! Error types for the provider-runtime layer.

use thiserror::Error;

/// Binding-validation errors raised by a provider runtime.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
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
#[derive(Debug, Clone, Error)]
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
    /// The registry resolved a connection whose provider identity does not
    /// match the provider the caller asked to resolve for. This is a typed
    /// invariant breach of the binding → connection seam, not a config
    /// validation failure.
    #[error(
        "auth binding resolved a connection for provider {resolved:?}, expected provider {expected:?}"
    )]
    ResolvedProviderMismatch {
        expected: meerkat_core::Provider,
        resolved: meerkat_core::Provider,
    },
    /// The host refused to run a program of its own outside its OS process
    /// confinement ([`meerkat_core::HostLaunchExemption`]): a confined
    /// process could change the program, the interpreter it names or its
    /// working directory. The program did not run, and no other credential
    /// stands in for it.
    #[error(
        "{exemption} refused: a confined process could change its program, its interpreter or its working directory ({refusal})"
    )]
    HostLaunchRefused {
        exemption: meerkat_core::HostLaunchExemption,
        refusal: meerkat_core::confinement::ConfinementRefusal,
    },
}

impl PartialEq for ProviderAuthError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Auth(a), Self::Auth(b)) => a == b,
            (Self::Binding(a), Self::Binding(b)) => a == b,
            (Self::SourceResolutionFailed(a), Self::SourceResolutionFailed(b)) => a == b,
            (Self::ExternalResolverMissing(a), Self::ExternalResolverMissing(b)) => a == b,
            (Self::NoRuntimeRegistered(a), Self::NoRuntimeRegistered(b)) => a == b,
            (
                Self::ResolvedProviderMismatch {
                    expected: ae,
                    resolved: ar,
                },
                Self::ResolvedProviderMismatch {
                    expected: be,
                    resolved: br,
                },
            ) => ae == be && ar == br,
            (
                Self::HostLaunchRefused {
                    exemption: ae,
                    refusal: ar,
                },
                Self::HostLaunchRefused {
                    exemption: be,
                    refusal: br,
                },
            ) => ae == be && ar == br,
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
    ClientInit(String),
    #[error("no credential material (auth_lease is empty)")]
    NoCredentialMaterial,
    #[error("dynamic authorizer requires Phase 3 owned HTTP path")]
    DynamicAuthorizerNotYetSupportedInShimMode,
    #[error("invalid base_url: {0}")]
    InvalidBaseUrl(String),
}

impl From<crate::LlmError> for ProviderClientError {
    fn from(e: crate::LlmError) -> Self {
        ProviderClientError::ClientInit(e.to_string())
    }
}

/// Why the account behind a resolved route cannot serve a model that only
/// its own model list could define: the model registry defines no such
/// model. The runtime of a backend whose accounts state the models they serve
/// gives it ([`crate::provider_runtime::ProviderRuntime::account_model`]).
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum AccountModelRefusal {
    /// The account's model list is not loaded, so it cannot define the
    /// model.
    #[error(
        "the model list of credential '{credential}' is not available, so it cannot define model '{model}'; retry once the list loads"
    )]
    ModelsUnavailable {
        credential: meerkat_core::AuthCredentialIdentity,
        model: String,
    },
    /// The account does not offer the model: its model list does not name
    /// it, or the account's policy disables it. `available` lists the ids
    /// the account offers on a protocol Meerkat speaks.
    #[error(
        "credential '{credential}' does not offer model '{model}'{}",
        offered_models_suffix(.available)
    )]
    NotOffered {
        credential: meerkat_core::AuthCredentialIdentity,
        model: String,
        available: Vec<String>,
    },
    /// The account offers the model on no endpoint Meerkat speaks.
    #[error("credential '{credential}' offers model '{model}' on no endpoint Meerkat speaks")]
    UnsupportedEndpoints {
        credential: meerkat_core::AuthCredentialIdentity,
        model: String,
    },
}

/// The offered ids a [`AccountModelRefusal::NotOffered`] names, as a
/// message suffix; empty when the account offers none.
fn offered_models_suffix(available: &[String]) -> String {
    if available.is_empty() {
        String::new()
    } else {
        format!("; it offers: {}", available.join(", "))
    }
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
        let e: ProviderClientError = ProviderClientError::ClientInit("invalid key".into());
        assert!(matches!(e, ProviderClientError::ClientInit(_)));
    }

    #[test]
    fn auth_error_from_core() {
        let e: ProviderAuthError = meerkat_core::AuthError::MissingSecret.into();
        assert!(matches!(e, ProviderAuthError::Auth(_)));
    }
}
