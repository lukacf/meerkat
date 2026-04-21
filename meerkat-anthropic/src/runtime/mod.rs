//! Anthropic provider runtime.

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
pub mod oauth;

use std::sync::Arc;

use async_trait::async_trait;

use meerkat_core::{AuthError, AuthMetadata, AuthProfile, BackendProfile, BindingPolicy, Provider};

use meerkat_auth_core::resolver::{resolve_external_authorizer, resolve_simple_secret};
use meerkat_llm_core::LlmClient;
use meerkat_llm_core::provider_runtime::binding::{
    NormalizedAuthMethod, NormalizedBackendKind, ResolvedConnection, StaticLease, ValidatedBinding,
};
use meerkat_llm_core::provider_runtime::errors::{
    ProviderAuthError, ProviderBindingError, ProviderClientError,
};
use meerkat_llm_core::provider_runtime::registry::ResolverEnvironment;
use meerkat_llm_core::provider_runtime::runtime::ProviderRuntime;

pub use meerkat_core::provider_matrix::anthropic::{AnthropicAuthMethod, AnthropicBackendKind};

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
        policy: &BindingPolicy,
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
            policy: policy.clone(),
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

        // Plan §4b.12: metadata carried on the lease. Populated below
        // when the ClaudeAiOauth / OauthToApiKey path finds an id_token
        // with user:profile claims.
        #[cfg_attr(
            not(all(not(target_arch = "wasm32"), feature = "oauth")),
            allow(unused_mut)
        )]
        let mut anthropic_email: Option<String> = None;
        #[cfg_attr(
            not(all(not(target_arch = "wasm32"), feature = "oauth")),
            allow(unused_mut)
        )]
        let mut anthropic_user_id: Option<String> = None;
        #[cfg_attr(
            not(all(not(target_arch = "wasm32"), feature = "oauth")),
            allow(unused_mut)
        )]
        let mut anthropic_subscription_tier: Option<String> = None;

        // Plan §6.11: Option<String> for secret material; None for
        // authorizer-backed paths (build_client constructs the concrete
        // HttpAuthorizer for AWS SigV4 / Google / Azure AD at build time).
        let secret_opt: Option<String> = match auth_method {
            AnthropicAuthMethod::ApiKey
            | AnthropicAuthMethod::StaticBearer
            | AnthropicAuthMethod::BedrockBearer
            | AnthropicAuthMethod::FoundryApiKey => {
                Some(resolve_simple_secret(&binding.auth_profile.source, env, binding).await?)
            }
            AnthropicAuthMethod::ExternalAuthorizer => {
                resolve_external_authorizer(&binding.auth_profile.source, env, binding).await?;
                None
            }
            AnthropicAuthMethod::BedrockAwsSigv4
            | AnthropicAuthMethod::VertexGoogleAuth
            | AnthropicAuthMethod::FoundryAzureAd => None,
            AnthropicAuthMethod::ClaudeAiOauth | AnthropicAuthMethod::OauthToApiKey => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
                {
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
                    let key = meerkat_core::auth::TokenKey::new(
                        realm_id,
                        binding.auth_profile.id.clone(),
                    );
                    let persisted = store
                        .load(&key)
                        .await
                        .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?
                        .ok_or(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired))?;
                    // Plan §4b.12: lift id_token claims.
                    if let Some(id_token) = persisted.id_token.as_deref()
                        && let Ok(claims) =
                            meerkat_auth_core::auth_oauth::jwt::decode_payload(id_token)
                    {
                        let lifted = oauth::AnthropicIdClaims::lift_from_claims(&claims.raw);
                        anthropic_email = lifted.email;
                        anthropic_user_id = lifted.user_id;
                        anthropic_subscription_tier = lifted.subscription_tier;
                    }

                    let secret = match auth_method {
                        AnthropicAuthMethod::OauthToApiKey => persisted
                            .primary_secret
                            .clone()
                            .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?,
                        AnthropicAuthMethod::ClaudeAiOauth => {
                            use chrono::{Duration, Utc};
                            let fresh = persisted
                                .expires_at
                                .is_none_or(|exp| exp - Utc::now() > Duration::seconds(60));
                            if let (true, Some(access)) = (fresh, persisted.primary_secret.clone())
                            {
                                access
                            } else {
                                let coord = env.refresh_coord.clone().unwrap_or_else(|| {
                                    Arc::new(meerkat_auth_core::InMemoryCoordinator::new())
                                });
                                let endpoints =
                                    oauth::claude_ai_endpoints(oauth::MANUAL_REDIRECT_URL);
                                let runtime = oauth::AnthropicOAuthRuntime::new(
                                    store.clone(),
                                    coord,
                                    endpoints,
                                    key,
                                );
                                runtime.get_or_refresh_access_token().await.map_err(
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
                                )?
                            }
                        }
                        _ => unreachable!("arm guarded by outer match"),
                    };
                    Some(secret)
                }
                #[cfg(not(all(not(target_arch = "wasm32"), feature = "oauth")))]
                {
                    return Err(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired));
                }
            }
        };

        // Plan §6.11 + §4b.12: populate the lease with resolved secret
        // as a typed InlineSecret variant, or an empty lease for
        // authorizer paths. Attach id_token-lifted claims as
        // `ProviderAuthMetadata::Anthropic`.
        let mut metadata = AuthMetadata::default();
        if anthropic_email.is_some()
            || anthropic_user_id.is_some()
            || anthropic_subscription_tier.is_some()
        {
            metadata.account_id = anthropic_user_id
                .clone()
                .or_else(|| anthropic_email.clone());
            metadata.plan = anthropic_subscription_tier.clone();
            metadata.provider_metadata = Some(meerkat_core::ProviderAuthMetadata::Anthropic(
                meerkat_core::AnthropicAuthMetadata {
                    subscription_tier: anthropic_subscription_tier,
                    aws_region: None,
                    vertex_project_id: None,
                    vertex_region: None,
                    foundry_deployment: None,
                },
            ));
        }
        let source_label = format!("anthropic:{}", binding.auth_profile.id);
        let lease: Arc<dyn meerkat_core::AuthLease> = match secret_opt {
            Some(secret) => Arc::new(StaticLease::inline_secret(
                secret,
                metadata,
                None,
                source_label,
            )),
            None => Arc::new(StaticLease::empty_lease(
                AuthMetadata::default(),
                source_label,
            )),
        };

        Ok(ResolvedConnection {
            provider: Provider::Anthropic,
            backend: NormalizedBackendKind::Anthropic(backend_kind),
            backend_profile: binding.backend_profile.clone(),
            auth_lease: lease,
        })
    }

    fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        // ProviderRuntimeRegistry dispatches on Provider enum, so this
        // runtime only receives Anthropic-backend connections. The match
        // is defensive; the non-Anthropic arms are unreachable at runtime.
        let backend_kind = match connection.backend {
            NormalizedBackendKind::Anthropic(k) => k,
            other => unreachable!(
                "AnthropicProviderRuntime received non-Anthropic backend: {other:?} \
                 — registry dispatch invariant violated"
            ),
        };
        // Plan §6.11: derive credential material from the auth lease
        // directly, directly from the lease. resolved_authorizer()
        // returns Some when the lease is a DynamicAuthorizer (Bedrock
        // SigV4 / Vertex GoogleAuth / Foundry AzureAd /
        // ExternalAuthorizer-DynamicAuthorizer); resolved_secret()
        // returns Some when the lease is a StaticHeaders with the
        // the resolved inline secret (api_key / static_bearer /
        // oauth_to_api_key / bedrock_bearer / pre-resolved Bearer).
        let authorizer_opt = connection.resolved_authorizer();
        let secret_opt = connection.resolved_secret();

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(authorizer) = authorizer_opt {
            // All authorizer-backed backends wire the same way:
            // AnthropicClient with .authorizer(...) + .base_url(...).
            // Bedrock / Vertex require a non-empty base URL; AnthropicApi
            // falls back to its default.
            let base_url = match backend_kind {
                AnthropicBackendKind::Bedrock => connection
                    .backend_profile
                    .base_url
                    .clone()
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| {
                        ProviderClientError::InvalidBaseUrl(
                            "bedrock backend requires BackendProfile.base_url".to_string(),
                        )
                    })?,
                AnthropicBackendKind::Vertex | AnthropicBackendKind::Foundry => connection
                    .backend_profile
                    .base_url
                    .clone()
                    .unwrap_or_default(),
                AnthropicBackendKind::AnthropicApi => connection
                    .backend_profile
                    .base_url
                    .clone()
                    .unwrap_or_else(|| {
                        AnthropicBackendKind::AnthropicApi.default_base_url().into()
                    }),
            };
            let client = crate::AnthropicClient::builder(String::new())
                .authorizer(authorizer)
                .base_url(base_url)
                .build()
                .map_err(ProviderClientError::from)?;
            return Ok(Arc::new(client));
        }

        #[cfg(target_arch = "wasm32")]
        if authorizer_opt.is_some() {
            return Err(ProviderClientError::MissingFeature(
                "authorizer-backed auth not available on wasm32",
            ));
        }

        let secret = secret_opt.ok_or(ProviderClientError::NoCredentialMaterial)?;

        match backend_kind {
            // Native Anthropic API — plain x-api-key auth.
            AnthropicBackendKind::AnthropicApi | AnthropicBackendKind::Foundry => {
                let mut client =
                    crate::AnthropicClient::new(secret).map_err(ProviderClientError::from)?;
                if let Some(url) = &connection.backend_profile.base_url {
                    client = client.with_base_url(url.clone());
                }
                Ok(Arc::new(client))
            }
            // Bedrock static bearer (AWS_BEARER_TOKEN_BEDROCK).
            #[cfg(not(target_arch = "wasm32"))]
            AnthropicBackendKind::Bedrock => {
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
                    std::sync::Arc::new(
                        meerkat_auth_core::authorizers::StaticBearerAuthorizer::new(
                            secret,
                            "bedrock-bearer",
                        ),
                    );
                let client = crate::AnthropicClient::builder(String::new())
                    .authorizer(authorizer)
                    .base_url(base_url)
                    .build()
                    .map_err(ProviderClientError::from)?;
                Ok(Arc::new(client))
            }
            #[cfg(target_arch = "wasm32")]
            AnthropicBackendKind::Bedrock => Err(ProviderClientError::MissingFeature(
                "bedrock-backend not available on wasm32",
            )),
            // Vertex with a pre-resolved bearer secret (ExternalAuthorizer
            // producing an InlineSecret envelope).
            #[cfg(not(target_arch = "wasm32"))]
            AnthropicBackendKind::Vertex => {
                let base_url = connection
                    .backend_profile
                    .base_url
                    .clone()
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| {
                        ProviderClientError::InvalidBaseUrl(
                            "vertex backend requires BackendProfile.base_url \
                             (e.g. https://<region>-aiplatform.googleapis.com)"
                                .to_string(),
                        )
                    })?;
                let authorizer: std::sync::Arc<dyn meerkat_core::HttpAuthorizer> =
                    std::sync::Arc::new(
                        meerkat_auth_core::authorizers::StaticBearerAuthorizer::new(
                            secret,
                            "vertex-bearer",
                        ),
                    );
                let client = crate::AnthropicClient::builder(String::new())
                    .authorizer(authorizer)
                    .base_url(base_url)
                    .build()
                    .map_err(ProviderClientError::from)?;
                Ok(Arc::new(client))
            }
            #[cfg(target_arch = "wasm32")]
            AnthropicBackendKind::Vertex => Err(ProviderClientError::MissingFeature(
                "vertex-backend with authorizer-backed auth not available on wasm32",
            )),
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
