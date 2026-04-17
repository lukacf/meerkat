//! Anthropic provider runtime.

pub mod auth;
pub mod backend;

use std::sync::Arc;

use async_trait::async_trait;

use meerkat_core::{AuthError, AuthMetadata, AuthProfile, BackendProfile, Provider};

use crate::runtime::binding::{
    NormalizedAuthMethod, NormalizedBackendKind, ResolvedConnection, ShimCredential, StaticLease,
    ValidatedBinding,
};
use crate::runtime::errors::{ProviderAuthError, ProviderBindingError, ProviderClientError};
use crate::runtime::provider_runtime::ProviderRuntime;
use crate::runtime::registry::ResolverEnvironment;
use crate::runtime::resolver::{resolve_external_authorizer, resolve_simple_secret};
use crate::types::LlmClient;

pub use auth::AnthropicAuthMethod;
pub use backend::AnthropicBackendKind;

/// Allowed (backend, auth) combinations for Anthropic.
pub const ALLOWED_BINDINGS: &[(AnthropicBackendKind, AnthropicAuthMethod)] = &[
    (
        AnthropicBackendKind::AnthropicApi,
        AnthropicAuthMethod::ApiKey,
    ),
    (
        AnthropicBackendKind::AnthropicApi,
        AnthropicAuthMethod::StaticBearer,
    ),
    (
        AnthropicBackendKind::AnthropicApi,
        AnthropicAuthMethod::ClaudeAiOauth,
    ),
    (
        AnthropicBackendKind::AnthropicApi,
        AnthropicAuthMethod::OauthToApiKey,
    ),
    (
        AnthropicBackendKind::AnthropicApi,
        AnthropicAuthMethod::ExternalAuthorizer,
    ),
];

pub struct AnthropicProviderRuntime;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ProviderRuntime for AnthropicProviderRuntime {
    fn provider_id(&self) -> Provider {
        Provider::Anthropic
    }

    fn validate_binding(
        &self,
        backend: &BackendProfile,
        auth: &AuthProfile,
    ) -> Result<ValidatedBinding, ProviderBindingError> {
        if backend.provider != Provider::Anthropic || auth.provider != Provider::Anthropic {
            return Err(ProviderBindingError::ProviderMismatch);
        }
        let backend_kind = AnthropicBackendKind::parse(&backend.backend_kind).ok_or_else(|| {
            ProviderBindingError::UnknownBackendKind(backend.backend_kind.clone())
        })?;
        let auth_method = AnthropicAuthMethod::parse(&auth.auth_method)
            .ok_or_else(|| ProviderBindingError::UnknownAuthMethod(auth.auth_method.clone()))?;
        if !ALLOWED_BINDINGS.contains(&(backend_kind, auth_method)) {
            return Err(ProviderBindingError::UnsupportedCombination {
                backend: backend.backend_kind.clone(),
                auth: auth.auth_method.clone(),
            });
        }
        Ok(ValidatedBinding {
            provider: Provider::Anthropic,
            backend: NormalizedBackendKind::Anthropic(backend_kind),
            auth: NormalizedAuthMethod::Anthropic(auth_method),
            backend_profile: Arc::new(backend.clone()),
            auth_profile: Arc::new(auth.clone()),
            policy: Default::default(),
        })
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        let auth_method = match binding.auth {
            NormalizedAuthMethod::Anthropic(m) => m,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };
        let backend_kind = match binding.backend {
            NormalizedBackendKind::Anthropic(k) => k,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };

        let shim_credential = match auth_method {
            AnthropicAuthMethod::ApiKey | AnthropicAuthMethod::StaticBearer => {
                resolve_simple_secret(&binding.auth_profile.source, env, binding).await?
            }
            AnthropicAuthMethod::ExternalAuthorizer => {
                resolve_external_authorizer(&binding.auth_profile.source, env, binding).await?
            }
            AnthropicAuthMethod::ClaudeAiOauth | AnthropicAuthMethod::OauthToApiKey => {
                return Err(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired));
            }
        };

        let lease: Arc<dyn meerkat_core::AuthLease> = Arc::new(StaticLease::empty(
            AuthMetadata::default(),
            format!("anthropic:{}", binding.auth_profile.id),
        ));

        Ok(ResolvedConnection {
            provider: Provider::Anthropic,
            backend: NormalizedBackendKind::Anthropic(backend_kind),
            backend_profile: binding.backend_profile.clone(),
            auth_lease: lease,
            shim_credential,
        })
    }

    fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        let backend_kind = match connection.backend {
            NormalizedBackendKind::Anthropic(k) => k,
            _ => {
                return Err(ProviderClientError::MissingFeature(
                    "anthropic-provider-mismatch",
                ));
            }
        };
        let secret = match connection.shim_credential {
            ShimCredential::Secret(s) => s,
            ShimCredential::Authorizer => {
                return Err(ProviderClientError::DynamicAuthorizerNotYetSupportedInShimMode);
            }
            ShimCredential::None => return Err(ProviderClientError::NoCredentialMaterial),
        };
        match backend_kind {
            AnthropicBackendKind::AnthropicApi => {
                // S1a-verified: AnthropicClient::new returns Result<Self, LlmError>.
                let mut client = crate::anthropic::AnthropicClient::new(secret)
                    .map_err(ProviderClientError::ClientInit)?;
                if let Some(url) = &connection.backend_profile.base_url {
                    client = client.with_base_url(url.clone());
                    // S1d: with_base_url swallows rebuild errors. Phase 3
                    // owns the HTTP path and fixes this.
                }
                Ok(Arc::new(client))
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn allowed_bindings_cover_api_key_and_oauth_variants() {
        assert!(ALLOWED_BINDINGS.contains(&(
            AnthropicBackendKind::AnthropicApi,
            AnthropicAuthMethod::ApiKey,
        )));
        assert!(ALLOWED_BINDINGS.contains(&(
            AnthropicBackendKind::AnthropicApi,
            AnthropicAuthMethod::ClaudeAiOauth,
        )));
    }

    #[test]
    fn provider_id_is_anthropic() {
        assert_eq!(AnthropicProviderRuntime.provider_id(), Provider::Anthropic);
    }
}
