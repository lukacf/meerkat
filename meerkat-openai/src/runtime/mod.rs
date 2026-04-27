//! OpenAI provider runtime.

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
pub mod oauth;

use std::sync::Arc;

use async_trait::async_trait;

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_core::AuthError;
use meerkat_core::{AuthLease, AuthMetadata, AuthProfile, BackendProfile, BindingPolicy, Provider};

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_auth_core::resolver::refresh_allowed;
use meerkat_auth_core::resolver::{
    finalize_auth_metadata, interactive_login_error, resolve_external_authorizer,
    resolve_simple_secret,
};
use meerkat_llm_core::provider_runtime::binding::{
    NormalizedAuthMethod, NormalizedBackendKind, ResolvedConnection, StaticLease, ValidatedBinding,
};
use meerkat_llm_core::provider_runtime::errors::{
    ProviderAuthError, ProviderBindingError, ProviderClientError,
};
use meerkat_llm_core::provider_runtime::registry::ResolverEnvironment;
use meerkat_llm_core::provider_runtime::runtime::ProviderRuntime;
use meerkat_llm_core::{ImageGenerationExecutor, LlmClient};

pub use meerkat_core::provider_matrix::openai::{OpenAiAuthMethod, OpenAiBackendKind};

/// Allowed (backend, auth) combinations for OpenAI. Phase 2 parses all
/// variants but only resolves ApiKey / StaticBearer / ExternalAuthorizer;
/// the ChatGpt combinations return `InteractiveLoginRequired` on resolve
/// and `MissingFeature("openai-chatgpt-auth")` on build.
pub const ALLOWED_BINDINGS: &[(OpenAiBackendKind, OpenAiAuthMethod)] = &[
    (OpenAiBackendKind::OpenAiApi, OpenAiAuthMethod::ApiKey),
    (OpenAiBackendKind::OpenAiApi, OpenAiAuthMethod::StaticBearer),
    (
        OpenAiBackendKind::OpenAiApi,
        OpenAiAuthMethod::ExternalAuthorizer,
    ),
    (
        OpenAiBackendKind::ChatGptBackend,
        OpenAiAuthMethod::ManagedChatGptOauth,
    ),
    (
        OpenAiBackendKind::ChatGptBackend,
        OpenAiAuthMethod::ExternalChatGptTokens,
    ),
];

pub struct OpenAiProviderRuntime;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ProviderRuntime for OpenAiProviderRuntime {
    fn provider_id(&self) -> Provider {
        Provider::OpenAI
    }

    fn validate_binding(
        &self,
        connection_ref: &meerkat_core::ConnectionRef,
        backend: &BackendProfile,
        auth: &AuthProfile,
        policy: &BindingPolicy,
    ) -> Result<ValidatedBinding, ProviderBindingError> {
        if backend.provider != Provider::OpenAI || auth.provider != Provider::OpenAI {
            return Err(ProviderBindingError::ProviderMismatch);
        }
        let backend_kind = OpenAiBackendKind::parse(&backend.backend_kind).ok_or_else(|| {
            ProviderBindingError::UnknownBackendKind(backend.backend_kind.clone())
        })?;
        let auth_method = OpenAiAuthMethod::parse(&auth.auth_method)
            .ok_or_else(|| ProviderBindingError::UnknownAuthMethod(auth.auth_method.clone()))?;
        if !ALLOWED_BINDINGS.contains(&(backend_kind, auth_method)) {
            return Err(ProviderBindingError::UnsupportedCombination {
                backend: backend.backend_kind.clone(),
                auth: auth.auth_method.clone(),
            });
        }
        Ok(ValidatedBinding {
            connection_ref: connection_ref.clone(),
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(backend_kind),
            auth: NormalizedAuthMethod::OpenAi(auth_method),
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
            NormalizedAuthMethod::OpenAi(m) => m,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };
        let backend_kind = match binding.backend {
            NormalizedBackendKind::OpenAi(k) => k,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };

        let source_label = format!("openai:{}", binding.auth_profile.id);
        let lease: Arc<dyn AuthLease> = match auth_method {
            OpenAiAuthMethod::ApiKey | OpenAiAuthMethod::StaticBearer => {
                let secret =
                    resolve_simple_secret(&binding.auth_profile.source, env, binding).await?;
                let metadata = finalize_auth_metadata(binding, AuthMetadata::default())?;
                Arc::new(StaticLease::inline_secret(
                    secret,
                    metadata,
                    None,
                    source_label.clone(),
                ))
            }
            OpenAiAuthMethod::ExternalAuthorizer => {
                resolve_external_authorizer(&binding.auth_profile.source, env, binding).await?
            }
            OpenAiAuthMethod::ManagedChatGptOauth | OpenAiAuthMethod::ExternalChatGptTokens => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
                {
                    let store = env
                        .token_store
                        .as_ref()
                        .ok_or_else(|| interactive_login_error(binding))?;
                    let key =
                        meerkat_core::auth::TokenKey::from_connection_ref(&binding.connection_ref);
                    let persisted = store
                        .load(&key)
                        .await
                        .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?
                        .ok_or_else(|| interactive_login_error(binding))?;

                    let mut chatgpt_account_id = persisted.account_id.clone();
                    let mut chatgpt_is_fedramp: Option<bool> = None;
                    let mut chatgpt_plan_type: Option<String> = None;
                    if let Some(id_token) = persisted.id_token.as_deref()
                        && let Ok(claims) =
                            meerkat_auth_core::auth_oauth::jwt::decode_payload(id_token)
                    {
                        let lifted = oauth::ChatGptIdClaims::lift_from_claims(&claims.raw);
                        if chatgpt_account_id.is_none() {
                            chatgpt_account_id = lifted.account_id;
                        }
                        chatgpt_is_fedramp = lifted.is_fedramp;
                        chatgpt_plan_type = lifted.plan_type;
                    }

                    let access = match auth_method {
                        OpenAiAuthMethod::ExternalChatGptTokens => persisted
                            .primary_secret
                            .clone()
                            .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?,
                        OpenAiAuthMethod::ManagedChatGptOauth => {
                            use chrono::{Duration, Utc};
                            let fresh = persisted
                                .expires_at
                                .is_none_or(|exp| exp - Utc::now() > Duration::seconds(60));
                            if let (true, Some(access)) = (fresh, persisted.primary_secret.clone())
                            {
                                access
                            } else {
                                if !refresh_allowed(binding) {
                                    return Err(ProviderAuthError::Auth(AuthError::Expired));
                                }
                                let coord = env.refresh_coord.clone().unwrap_or_else(|| {
                                    Arc::new(meerkat_auth_core::InMemoryCoordinator::new())
                                });
                                let endpoints =
                                    oauth::chatgpt_endpoints("http://127.0.0.1:0/callback");
                                let runtime = oauth::OpenAiOAuthRuntime::new(
                                    store.clone(),
                                    coord,
                                    endpoints,
                                    key,
                                );
                                runtime.get_or_refresh_access_token().await.map_err(
                                    |e| match e {
                                        oauth::OpenAiOAuthError::InteractiveLoginRequired => {
                                            interactive_login_error(binding)
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
                    let mut metadata = AuthMetadata::default();
                    if chatgpt_account_id.is_some()
                        || chatgpt_is_fedramp.is_some()
                        || chatgpt_plan_type.is_some()
                    {
                        metadata.account_id = chatgpt_account_id.clone();
                        metadata.plan = chatgpt_plan_type.clone();
                        metadata.provider_metadata =
                            Some(meerkat_core::ProviderAuthMetadata::OpenAi(
                                meerkat_core::OpenAiAuthMetadata {
                                    plan_type: chatgpt_plan_type,
                                    user_id: None,
                                    account_id: chatgpt_account_id,
                                    is_fedramp: chatgpt_is_fedramp,
                                    email: None,
                                },
                            ));
                    }
                    let metadata = finalize_auth_metadata(binding, metadata)?;
                    Arc::new(StaticLease::inline_secret(
                        access,
                        metadata,
                        persisted.expires_at,
                        source_label.clone(),
                    ))
                }
                #[cfg(not(all(not(target_arch = "wasm32"), feature = "oauth")))]
                {
                    return Err(interactive_login_error(binding));
                }
            }
        };

        Ok(ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(backend_kind),
            backend_profile: binding.backend_profile.clone(),
            auth_lease: lease,
        })
    }

    fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        // ProviderRuntimeRegistry dispatches on Provider enum; non-OpenAI
        // arms are unreachable at runtime.
        let backend_kind = match connection.backend {
            NormalizedBackendKind::OpenAi(k) => k,
            other => unreachable!(
                "OpenAiProviderRuntime received non-OpenAi backend: {other:?} \
                 — registry dispatch invariant violated"
            ),
        };
        // Authorizer-backed path (ExternalAuthorizer→DynamicAuthorizer
        // envelope). Wire through OpenAiClient.with_authorizer so the
        // request uses HttpAuthorizer::authorize for headers rather
        // than Authorization: Bearer <api_key>. Plan §6.11: read the
        // authorizer from the auth lease directly instead of the
        // (deleted side channel).
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(authorizer) = connection.resolved_authorizer() {
            let base_url = connection
                .backend_profile
                .base_url
                .clone()
                .unwrap_or_else(|| backend_kind.default_base_url().into());
            let client =
                crate::OpenAiClient::new_with_optional_api_key_and_base_url(None, base_url)
                    .with_authorizer(authorizer);
            return Ok(Arc::new(client));
        }
        // No inline secret and no DynamicAuthorizer kind: the
        // lease was constructed empty. Surface as missing credential
        // material (or MissingFeature on wasm32 where authorizers
        // don't compile) so surfaces get a typed error.
        #[cfg(target_arch = "wasm32")]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::MissingFeature(
                "openai-authorizer-backed auth not available on wasm32",
            ))?;
        #[cfg(not(target_arch = "wasm32"))]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        // Pull account identity + fedramp from the resolved lease's
        // AuthMetadata so the ChatGPT backend can emit the wire
        // headers Codex's bearer_auth_provider.rs:23-38 requires.
        let (account_id, is_fedramp) = match connection.auth_lease.metadata().provider_metadata {
            Some(meerkat_core::ProviderAuthMetadata::OpenAi(ref m)) => {
                (m.account_id.clone(), m.is_fedramp)
            }
            _ => (connection.auth_lease.metadata().account_id.clone(), None),
        };

        match backend_kind {
            OpenAiBackendKind::OpenAiApi => {
                // S1-verified: OpenAiClient::new returns Self (infallible).
                let client = match &connection.backend_profile.base_url {
                    Some(url) => crate::OpenAiClient::new_with_base_url(secret, url.clone()),
                    None => crate::OpenAiClient::new(secret),
                };
                Ok(Arc::new(client))
            }
            OpenAiBackendKind::ChatGptBackend => {
                // ChatGPT backend: emit account_id + fedramp wire
                // headers per Codex bearer_auth_provider.rs:23-38.
                let base_url = connection
                    .backend_profile
                    .base_url
                    .clone()
                    .unwrap_or_else(|| OpenAiBackendKind::ChatGptBackend.default_base_url().into());
                let mut extra_headers: Vec<(String, String)> = Vec::new();
                if let Some(acct) = account_id {
                    extra_headers.push((
                        meerkat_core::provider_matrix::openai_auth::CHATGPT_ACCOUNT_HEADER
                            .to_string(),
                        acct,
                    ));
                }
                if matches!(is_fedramp, Some(true)) {
                    extra_headers.push((
                        meerkat_core::provider_matrix::openai_auth::FEDRAMP_HEADER.to_string(),
                        "true".to_string(),
                    ));
                }
                let client = crate::OpenAiClient::new_with_base_url(secret, base_url)
                    .with_extra_headers(extra_headers);
                Ok(Arc::new(client))
            }
        }
    }

    fn build_image_generation_executor(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Option<Arc<dyn ImageGenerationExecutor>>, ProviderClientError> {
        let backend_kind = match connection.backend {
            NormalizedBackendKind::OpenAi(k) => k,
            other => unreachable!(
                "OpenAiProviderRuntime received non-OpenAi backend: {other:?} \
                 — registry dispatch invariant violated"
            ),
        };
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(authorizer) = connection.resolved_authorizer() {
            let base_url = connection
                .backend_profile
                .base_url
                .clone()
                .unwrap_or_else(|| backend_kind.default_base_url().into());
            let client =
                crate::OpenAiClient::new_with_optional_api_key_and_base_url(None, base_url)
                    .with_authorizer(authorizer);
            return Ok(Some(Arc::new(client)));
        }
        #[cfg(target_arch = "wasm32")]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::MissingFeature(
                "openai-authorizer-backed auth not available on wasm32",
            ))?;
        #[cfg(not(target_arch = "wasm32"))]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        let client = match backend_kind {
            OpenAiBackendKind::OpenAiApi => match &connection.backend_profile.base_url {
                Some(url) => crate::OpenAiClient::new_with_base_url(secret, url.clone()),
                None => crate::OpenAiClient::new(secret),
            },
            OpenAiBackendKind::ChatGptBackend => {
                let base_url = connection
                    .backend_profile
                    .base_url
                    .clone()
                    .unwrap_or_else(|| OpenAiBackendKind::ChatGptBackend.default_base_url().into());
                crate::OpenAiClient::new_with_base_url(secret, base_url)
            }
        };
        Ok(Some(Arc::new(client)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn allowed_bindings_contain_expected_combinations() {
        assert!(
            ALLOWED_BINDINGS.contains(&(OpenAiBackendKind::OpenAiApi, OpenAiAuthMethod::ApiKey))
        );
        assert!(ALLOWED_BINDINGS.contains(&(
            OpenAiBackendKind::ChatGptBackend,
            OpenAiAuthMethod::ManagedChatGptOauth,
        )));
    }

    #[test]
    fn provider_id_is_openai() {
        assert_eq!(OpenAiProviderRuntime.provider_id(), Provider::OpenAI);
    }

    fn backend(kind: &str) -> BackendProfile {
        BackendProfile {
            id: "b".into(),
            provider: Provider::OpenAI,
            backend_kind: kind.into(),
            base_url: None,
            options: serde_json::Value::Null,
        }
    }

    fn auth(method: &str) -> AuthProfile {
        AuthProfile {
            id: "a".into(),
            provider: Provider::OpenAI,
            auth_method: method.into(),
            source: meerkat_core::CredentialSourceSpec::InlineSecret {
                secret: "sk-x".into(),
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        }
    }

    #[test]
    fn validate_accepts_allowed_combination() {
        let rt = OpenAiProviderRuntime;
        let vb = rt
            .validate_binding(
                &meerkat_core::ConnectionRef {
                    realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
                    binding: meerkat_core::connection::BindingId::parse("default").unwrap(),
                    profile: None,
                },
                &backend("openai_api"),
                &auth("api_key"),
                &BindingPolicy::default(),
            )
            .expect("allowed combination");
        assert_eq!(vb.provider, Provider::OpenAI);
    }

    #[test]
    fn validate_rejects_unknown_backend_kind() {
        let rt = OpenAiProviderRuntime;
        let err = rt
            .validate_binding(
                &meerkat_core::ConnectionRef {
                    realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
                    binding: meerkat_core::connection::BindingId::parse("default").unwrap(),
                    profile: None,
                },
                &backend("bogus_backend"),
                &auth("api_key"),
                &BindingPolicy::default(),
            )
            .unwrap_err();
        assert!(matches!(err, ProviderBindingError::UnknownBackendKind(_)));
    }

    #[test]
    fn validate_rejects_unsupported_combo() {
        // openai_api + managed_chatgpt_oauth is NOT in ALLOWED_BINDINGS.
        let rt = OpenAiProviderRuntime;
        let err = rt
            .validate_binding(
                &meerkat_core::ConnectionRef {
                    realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
                    binding: meerkat_core::connection::BindingId::parse("default").unwrap(),
                    profile: None,
                },
                &backend("openai_api"),
                &auth("managed_chatgpt_oauth"),
                &BindingPolicy::default(),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            ProviderBindingError::UnsupportedCombination { .. }
        ));
    }

    #[test]
    fn validate_rejects_provider_mismatch() {
        let rt = OpenAiProviderRuntime;
        let mut wrong = backend("openai_api");
        wrong.provider = Provider::Anthropic;
        let err = rt
            .validate_binding(
                &meerkat_core::ConnectionRef {
                    realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
                    binding: meerkat_core::connection::BindingId::parse("default").unwrap(),
                    profile: None,
                },
                &wrong,
                &auth("api_key"),
                &BindingPolicy::default(),
            )
            .unwrap_err();
        assert!(matches!(err, ProviderBindingError::ProviderMismatch));
    }

    #[test]
    fn validate_propagates_binding_policy() {
        // Dogma §16: policy declared on the binding must flow through
        // validate_binding, not default-injected at the provider seam.
        let rt = OpenAiProviderRuntime;
        let policy = BindingPolicy {
            allow_auth_override: true,
            require_metadata_account: true,
            require_metadata_workspace: false,
        };
        let vb = rt
            .validate_binding(
                &meerkat_core::ConnectionRef {
                    realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
                    binding: meerkat_core::connection::BindingId::parse("default").unwrap(),
                    profile: None,
                },
                &backend("openai_api"),
                &auth("api_key"),
                &policy,
            )
            .expect("allowed combination");
        assert_eq!(vb.policy, policy);
    }
}
