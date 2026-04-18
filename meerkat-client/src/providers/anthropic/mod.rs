//! Anthropic provider runtime.

pub mod auth;
pub mod backend;
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
pub mod oauth;

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
    // Native Anthropic API.
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
    // Bedrock.
    (
        AnthropicBackendKind::Bedrock,
        AnthropicAuthMethod::BedrockBearer,
    ),
    (
        AnthropicBackendKind::Bedrock,
        AnthropicAuthMethod::BedrockAwsSigv4,
    ),
    (
        AnthropicBackendKind::Bedrock,
        AnthropicAuthMethod::ExternalAuthorizer,
    ),
    // Vertex.
    (
        AnthropicBackendKind::Vertex,
        AnthropicAuthMethod::VertexGoogleAuth,
    ),
    (
        AnthropicBackendKind::Vertex,
        AnthropicAuthMethod::ExternalAuthorizer,
    ),
    // Foundry.
    (
        AnthropicBackendKind::Foundry,
        AnthropicAuthMethod::FoundryApiKey,
    ),
    (
        AnthropicBackendKind::Foundry,
        AnthropicAuthMethod::FoundryAzureAd,
    ),
    (
        AnthropicBackendKind::Foundry,
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
            AnthropicAuthMethod::ApiKey
            | AnthropicAuthMethod::StaticBearer
            | AnthropicAuthMethod::BedrockBearer
            | AnthropicAuthMethod::FoundryApiKey => {
                // All of these resolve to a simple static secret.
                resolve_simple_secret(&binding.auth_profile.source, env, binding).await?
            }
            AnthropicAuthMethod::ExternalAuthorizer => {
                resolve_external_authorizer(&binding.auth_profile.source, env, binding).await?
            }
            AnthropicAuthMethod::BedrockAwsSigv4
            | AnthropicAuthMethod::VertexGoogleAuth
            | AnthropicAuthMethod::FoundryAzureAd => {
                // Dynamic authorizer cases — build_client constructs the
                // concrete authorizer (AwsSts / GoogleAuth / AzureAd) and
                // attaches it to an AnthropicClient or BedrockClient with
                // the binding's base_url.
                ShimCredential::Authorizer
            }
            AnthropicAuthMethod::ClaudeAiOauth | AnthropicAuthMethod::OauthToApiKey => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
                {
                    // Read persisted tokens from the token store (if one is
                    // wired into the env). The interactive login flow runs
                    // in the CLI/surface layer (Phase 4d); this path only
                    // retrieves + refreshes the persisted credential.
                    let store = env
                        .token_store
                        .as_ref()
                        .ok_or(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired))?;
                    let realm_id = binding
                        .backend_profile
                        .options
                        .get("realm_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("dev")
                        .to_string();
                    let key =
                        crate::auth_store::TokenKey::new(realm_id, binding.auth_profile.id.clone());
                    let persisted = store
                        .load(&key)
                        .await
                        .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?
                        .ok_or(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired))?;

                    match auth_method {
                        AnthropicAuthMethod::OauthToApiKey => {
                            // The OAuth→API-key provisioning flow converts
                            // the OAuth token into a long-lived API key at
                            // login time. Here we just read the stored key.
                            let secret = persisted
                                .primary_secret
                                .clone()
                                .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
                            ShimCredential::Secret(secret)
                        }
                        AnthropicAuthMethod::ClaudeAiOauth => {
                            // For claude_ai_oauth, the persisted bundle
                            // carries an access_token + refresh_token. If
                            // the access_token is still fresh, use it
                            // directly; otherwise refresh.
                            use chrono::{Duration, Utc};
                            let fresh = persisted
                                .expires_at
                                .is_none_or(|exp| exp - Utc::now() > Duration::seconds(60));
                            if let (true, Some(access)) = (fresh, persisted.primary_secret.clone())
                            {
                                ShimCredential::Secret(access)
                            } else {
                                // Need refresh — drive through OAuth runtime.
                                let coord = env.refresh_coord.clone().unwrap_or_else(|| {
                                    Arc::new(crate::auth_store::InMemoryCoordinator::new())
                                });
                                let endpoints =
                                    oauth::claude_ai_endpoints(oauth::MANUAL_REDIRECT_URL);
                                let runtime = oauth::AnthropicOAuthRuntime::new(
                                    store.clone(),
                                    coord,
                                    endpoints,
                                    key,
                                );
                                let access = runtime.get_or_refresh_access_token().await.map_err(
                                    |e| match e {
                                        oauth::AnthropicOAuthError::InteractiveLoginRequired => {
                                            ProviderAuthError::Auth(
                                                AuthError::InteractiveLoginRequired,
                                            )
                                        }
                                        other => ProviderAuthError::SourceResolutionFailed(
                                            other.to_string(),
                                        ),
                                    },
                                )?;
                                ShimCredential::Secret(access)
                            }
                        }
                        _ => unreachable!("arm guarded by outer match"),
                    }
                }
                #[cfg(not(all(not(target_arch = "wasm32"), feature = "oauth")))]
                {
                    return Err(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired));
                }
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
        match (backend_kind, &connection.shim_credential) {
            // Native Anthropic API — plain x-api-key auth.
            (AnthropicBackendKind::AnthropicApi, ShimCredential::Secret(secret)) => {
                let mut client = crate::anthropic::AnthropicClient::new(secret.clone())
                    .map_err(ProviderClientError::ClientInit)?;
                if let Some(url) = &connection.backend_profile.base_url {
                    client = client.with_base_url(url.clone());
                }
                Ok(Arc::new(client))
            }
            // Foundry API-key — same shape as native, different base URL.
            (AnthropicBackendKind::Foundry, ShimCredential::Secret(secret)) => {
                let mut client = crate::anthropic::AnthropicClient::new(secret.clone())
                    .map_err(ProviderClientError::ClientInit)?;
                if let Some(url) = &connection.backend_profile.base_url {
                    client = client.with_base_url(url.clone());
                }
                Ok(Arc::new(client))
            }
            // Bedrock static bearer (AWS_BEARER_TOKEN_BEDROCK) — per
            // plan §Phase 4b.6 method `bedrock_bearer`, this path
            // targets the Messages API via base_url override supplied
            // by the BackendProfile and injects
            // `Authorization: Bearer <token>`. AnthropicClient's
            // authorizer seam accepts a StaticBearerAuthorizer for
            // this pattern — no Bedrock-specific LlmClient needed for
            // the bearer subpath (the SigV4 subpath uses
            // AwsStsAuthorizer, which is a separate variant below).
            (AnthropicBackendKind::Bedrock, ShimCredential::Secret(secret)) => {
                let base_url = connection
                    .backend_profile
                    .base_url
                    .clone()
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| {
                        ProviderClientError::InvalidBaseUrl(
                            "bedrock backend requires BackendProfile.base_url \
                             (e.g. https://bedrock-runtime.us-east-1.amazonaws.com)"
                                .to_string(),
                        )
                    })?;
                let authorizer: std::sync::Arc<dyn meerkat_core::HttpAuthorizer> =
                    std::sync::Arc::new(crate::authorizers::StaticBearerAuthorizer::new(
                        secret.clone(),
                        "bedrock-bearer",
                    ));
                // Use a placeholder api_key — AnthropicClient with an
                // authorizer suppresses x-api-key in favor of the
                // authorizer-injected Authorization header.
                let client = crate::anthropic::AnthropicClient::builder(String::new())
                    .authorizer(authorizer)
                    .base_url(base_url)
                    .build()
                    .map_err(ProviderClientError::ClientInit)?;
                Ok(Arc::new(client))
            }
            // Dynamic authorizer flows: Vertex (Google Bearer), Foundry
            // Azure AD (Bearer), Bedrock SigV4. AnthropicClient supports
            // Bearer-style authorizers (Vertex + Foundry AzureAd) via
            // .authorizer(...). Bedrock SigV4 needs a new client for the
            // event-stream wire protocol — tracked separately.
            (
                AnthropicBackendKind::Vertex | AnthropicBackendKind::Foundry,
                ShimCredential::Authorizer,
            ) => {
                // Authorizer-backed Bearer flow. We need the concrete
                // HttpAuthorizer — in the current Phase 4b shim, we
                // construct it here from the binding metadata.
                // `ResolvedConnection.auth_lease` carries the authorizer
                // for DynamicAuthorizer leases; we extract it.
                let authorizer = match connection.auth_lease.kind() {
                    meerkat_core::ResolvedAuthKind::DynamicAuthorizer(auth) => auth.clone(),
                    _ => {
                        return Err(ProviderClientError::MissingFeature(
                            "anthropic-dynamic-authorizer-lease-mismatch",
                        ));
                    }
                };
                let base_url = connection
                    .backend_profile
                    .base_url
                    .clone()
                    .unwrap_or_default();
                let client = crate::anthropic::AnthropicClient::builder(String::new())
                    .base_url(base_url)
                    .authorizer(authorizer)
                    .build()
                    .map_err(ProviderClientError::ClientInit)?;
                Ok(Arc::new(client))
            }
            (AnthropicBackendKind::Bedrock, ShimCredential::Authorizer) => {
                // Bedrock SigV4 with dynamic AwsStsAuthorizer — requires
                // event-stream parsing. Tracked.
                Err(ProviderClientError::MissingFeature(
                    "anthropic-bedrock-sigv4 (event-stream parser — tracked)",
                ))
            }
            (AnthropicBackendKind::Vertex, ShimCredential::Secret(_)) => {
                // Vertex with a raw secret doesn't match any allowed
                // binding combination, but if it gets here, fall back
                // to the Messages API via static bearer. Typically
                // Vertex uses VertexGoogleAuth (authorizer path).
                Err(ProviderClientError::MissingFeature(
                    "anthropic-vertex-static-secret (use vertex_google_auth)",
                ))
            }
            (_, ShimCredential::Authorizer) => {
                Err(ProviderClientError::DynamicAuthorizerNotYetSupportedInShimMode)
            }
            (_, ShimCredential::None) => Err(ProviderClientError::NoCredentialMaterial),
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
