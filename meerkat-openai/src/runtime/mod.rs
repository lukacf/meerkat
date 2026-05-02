//! OpenAI provider runtime.

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
pub mod oauth;

use std::sync::Arc;

use async_trait::async_trait;

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_core::AuthError;
use meerkat_core::{AuthLease, AuthMetadata, AuthProfile, BackendProfile, BindingPolicy, Provider};

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_auth_core::resolver::{
    ManagedOauthAccess, RefreshableStoredTokenLease, StoredTokenRefreshFn,
    StoredTokenRefreshOutcome, begin_managed_oauth_refresh, fail_managed_oauth_refresh,
    oauth_refresh_error_text_is_permanent, resolve_lease_bound_stored_tokens,
    resolve_managed_oauth_access, save_and_complete_managed_oauth_refresh,
};
use meerkat_auth_core::resolver::{
    finalize_auth_metadata, interactive_login_error, resolve_external_authorizer,
    resolve_simple_secret_with_auth_context,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_auth_core::{
    auth_store::PersistedAuthMode, oauth_flow::validate_oauth_target_for_auth_mode,
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

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn openai_oauth_refresh_failure_is_permanent(error: &oauth::OpenAiOAuthError) -> bool {
    match error {
        oauth::OpenAiOAuthError::InteractiveLoginRequired
        | oauth::OpenAiOAuthError::MissingRefreshToken => true,
        oauth::OpenAiOAuthError::Refresh(meerkat_auth_core::RefreshError::Refresh(message)) => {
            managed_store_oauth_refresh_failure_is_permanent(message)
        }
        oauth::OpenAiOAuthError::OAuth(error) => {
            managed_store_oauth_refresh_failure_is_permanent(&error.to_string())
        }
        _ => false,
    }
}

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

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn openai_oauth_refresh_error_is_permanent(error: &oauth::OpenAiOAuthError) -> bool {
    match error {
        oauth::OpenAiOAuthError::InteractiveLoginRequired
        | oauth::OpenAiOAuthError::MissingRefreshToken => true,
        other => oauth_refresh_error_text_is_permanent(&other.to_string()),
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn openai_oauth_refresh_error_to_provider(
    error: oauth::OpenAiOAuthError,
    binding: &ValidatedBinding,
) -> ProviderAuthError {
    let error_text = error.to_string();
    if matches!(error, oauth::OpenAiOAuthError::InteractiveLoginRequired) {
        interactive_login_error(binding)
    } else {
        ProviderAuthError::SourceResolutionFailed(error_text)
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn provider_auth_error_to_auth_error(error: ProviderAuthError) -> AuthError {
    match error {
        ProviderAuthError::Auth(error) => error,
        other => AuthError::Other(other.to_string()),
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn openai_managed_oauth_refresh_fn(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    store: Arc<dyn meerkat_core::auth::TokenStore>,
    key: meerkat_core::auth::TokenKey,
    expected_mode: meerkat_core::auth::PersistedAuthMode,
) -> Result<StoredTokenRefreshFn, ProviderAuthError> {
    let auth_lease_handle = env.auth_lease_handle.clone().ok_or_else(|| {
        ProviderAuthError::SourceResolutionFailed(
            "managed OAuth refresh requires an AuthMachine lease handle".into(),
        )
    })?;
    let refresh_coord = env
        .refresh_coord
        .clone()
        .unwrap_or_else(|| Arc::new(meerkat_auth_core::InMemoryCoordinator::new()));
    let refresh_env = ResolverEnvironment {
        env_lookup: env.env_lookup.clone(),
        external_resolvers: env.external_resolvers.clone(),
        now: env.now.clone(),
        auth_lease_handle: Some(auth_lease_handle),
        token_store: Some(store.clone()),
        refresh_coord: Some(refresh_coord.clone()),
    };
    let binding = binding.clone();
    Ok(Arc::new(move |_reason| {
        let refresh_env = ResolverEnvironment {
            env_lookup: refresh_env.env_lookup.clone(),
            external_resolvers: refresh_env.external_resolvers.clone(),
            now: refresh_env.now.clone(),
            auth_lease_handle: refresh_env.auth_lease_handle.clone(),
            token_store: refresh_env.token_store.clone(),
            refresh_coord: refresh_env.refresh_coord.clone(),
        };
        let binding = binding.clone();
        let store = store.clone();
        let refresh_coord = refresh_coord.clone();
        let key = key.clone();
        Box::pin(async move {
            let (lifecycle, tokens) =
                begin_managed_oauth_refresh(&refresh_env, &binding, expected_mode)
                    .await
                    .map_err(provider_auth_error_to_auth_error)?;
            let endpoints = oauth::chatgpt_endpoints("http://127.0.0.1:0/callback");
            let runtime =
                oauth::OpenAiOAuthRuntime::new(store, refresh_coord, endpoints, key.clone());
            let refreshed = runtime
                .refresh_access_token_from_persisted_without_save(&tokens)
                .await
                .map_err(|error| {
                    let permanent = openai_oauth_refresh_error_is_permanent(&error);
                    let _ = fail_managed_oauth_refresh(
                        &refresh_env,
                        &binding,
                        lifecycle.clone(),
                        permanent,
                    );
                    provider_auth_error_to_auth_error(openai_oauth_refresh_error_to_provider(
                        error, &binding,
                    ))
                })?;
            save_and_complete_managed_oauth_refresh(
                &refresh_env,
                &binding,
                &key,
                lifecycle,
                &tokens,
                &refreshed,
            )
            .await
            .and_then(|completion| {
                let secret = refreshed.primary_secret.clone().ok_or_else(|| {
                    ProviderAuthError::SourceResolutionFailed(
                        "managed OAuth refresh returned no primary_secret".into(),
                    )
                })?;
                Ok(StoredTokenRefreshOutcome {
                    secret,
                    expires_at: completion.expires_at,
                    lease_snapshot: Some(completion.lease_snapshot),
                })
            })
            .map_err(provider_auth_error_to_auth_error)
        })
    }))
}

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
                let resolved = resolve_simple_secret_with_auth_context(
                    &binding.auth_profile.source,
                    env,
                    binding,
                )
                .await?;
                let metadata = finalize_auth_metadata(binding, AuthMetadata::default())?;
                Arc::new(
                    StaticLease::inline_secret(
                        resolved.secret,
                        metadata,
                        None,
                        source_label.clone(),
                    )
                    .with_auth_lease_snapshot(resolved.auth_lease_snapshot),
                )
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

                    let (resolved_tokens, lease_expires_at, auth_lease_snapshot) = match auth_method
                    {
                        OpenAiAuthMethod::ExternalChatGptTokens => {
                            let resolved = resolve_lease_bound_stored_tokens(
                                env,
                                binding,
                                meerkat_core::auth::PersistedAuthMode::ExternalTokens,
                            )
                            .await?;
                            (
                                resolved.tokens,
                                resolved.expires_at,
                                Some(resolved.lease_snapshot),
                            )
                        }
                        OpenAiAuthMethod::ManagedChatGptOauth => {
                            match resolve_managed_oauth_access(
                                env,
                                binding,
                                meerkat_core::auth::PersistedAuthMode::ChatgptOauth,
                            )
                            .await?
                            {
                                ManagedOauthAccess::Cached {
                                    tokens,
                                    expires_at,
                                    lease_snapshot,
                                } => (tokens, expires_at, Some(lease_snapshot)),
                                ManagedOauthAccess::Refresh { lifecycle, tokens } => {
                                    let coord = env.refresh_coord.clone().unwrap_or_else(|| {
                                        Arc::new(meerkat_auth_core::InMemoryCoordinator::new())
                                    });
                                    let endpoints =
                                        oauth::chatgpt_endpoints("http://127.0.0.1:0/callback");
                                    let runtime = oauth::OpenAiOAuthRuntime::new(
                                        store.clone(),
                                        coord,
                                        endpoints,
                                        key.clone(),
                                    );
                                    let refreshed = runtime
                                        .refresh_access_token_from_persisted_without_save(&tokens)
                                        .await
                                        .map_err(|e| {
                                            let permanent =
                                                openai_oauth_refresh_error_is_permanent(&e);
                                            let _ = fail_managed_oauth_refresh(
                                                env,
                                                binding,
                                                lifecycle.clone(),
                                                permanent,
                                            );
                                            openai_oauth_refresh_error_to_provider(e, binding)
                                        })?;
                                    let completion = save_and_complete_managed_oauth_refresh(
                                        env, binding, &key, lifecycle, &tokens, &refreshed,
                                    )
                                    .await?;
                                    (
                                        refreshed,
                                        completion.expires_at,
                                        Some(completion.lease_snapshot),
                                    )
                                }
                            }
                        }
                        _ => unreachable!("arm guarded by outer match"),
                    };
                    let access = resolved_tokens
                        .primary_secret
                        .clone()
                        .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
                    let mut chatgpt_account_id = resolved_tokens.account_id.clone();
                    let mut chatgpt_is_fedramp: Option<bool> = None;
                    let mut chatgpt_plan_type: Option<String> = None;
                    if let Some(id_token) = resolved_tokens.id_token.as_deref()
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
                    match auth_method {
                        OpenAiAuthMethod::ManagedChatGptOauth => {
                            let refresh = openai_managed_oauth_refresh_fn(
                                env,
                                binding,
                                store.clone(),
                                key.clone(),
                                meerkat_core::auth::PersistedAuthMode::ChatgptOauth,
                            )?;
                            Arc::new(RefreshableStoredTokenLease::inline_secret(
                                access,
                                metadata,
                                lease_expires_at,
                                auth_lease_snapshot,
                                source_label.clone(),
                                refresh,
                            ))
                        }
                        OpenAiAuthMethod::ExternalChatGptTokens => Arc::new(
                            StaticLease::inline_secret(
                                access,
                                metadata,
                                lease_expires_at,
                                source_label.clone(),
                            )
                            .with_auth_lease_snapshot(auth_lease_snapshot),
                        ),
                        _ => unreachable!("arm guarded by outer match"),
                    }
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

    fn image_generation_profile(
        &self,
    ) -> Option<Arc<dyn meerkat_core::ImageGenerationProviderProfile>> {
        Some(Arc::new(crate::OpenAiImageGenerationProfile))
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
