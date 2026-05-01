//! Google provider runtime (Gemini API, Vertex AI, Code Assist).

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
pub mod oauth;

use std::sync::Arc;

use async_trait::async_trait;

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_core::AuthError;
#[cfg(all(not(target_arch = "wasm32"), feature = "adc"))]
use meerkat_core::HttpAuthorizer;
use meerkat_core::{AuthLease, AuthMetadata, AuthProfile, BackendProfile, BindingPolicy, Provider};

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_auth_core::resolver::{
    ManagedOauthAccess, RefreshableStoredTokenLease, StoredTokenRefreshFn,
    StoredTokenRefreshOutcome, begin_managed_oauth_refresh, fail_managed_oauth_refresh,
    oauth_refresh_error_text_is_permanent, resolve_managed_oauth_access,
    save_and_complete_managed_oauth_refresh,
};
use meerkat_auth_core::resolver::{
    finalize_auth_metadata, interactive_login_error, resolve_external_authorizer,
    resolve_simple_secret_with_auth_context,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "adc"))]
use meerkat_llm_core::provider_runtime::binding::DynamicLease;
use meerkat_llm_core::provider_runtime::binding::{
    NormalizedAuthMethod, NormalizedBackendKind, ResolvedConnection, StaticLease, ValidatedBinding,
};
use meerkat_llm_core::provider_runtime::errors::{
    ProviderAuthError, ProviderBindingError, ProviderClientError,
};
use meerkat_llm_core::provider_runtime::registry::ResolverEnvironment;
use meerkat_llm_core::provider_runtime::runtime::ProviderRuntime;
use meerkat_llm_core::{ImageGenerationExecutor, LlmClient};

pub use meerkat_core::provider_matrix::google::{GoogleAuthMethod, GoogleBackendKind};

/// Allowed (backend, auth) combinations for Google.
pub const ALLOWED_BINDINGS: &[(GoogleBackendKind, GoogleAuthMethod)] = &[
    (GoogleBackendKind::GoogleGenAi, GoogleAuthMethod::ApiKey),
    (
        GoogleBackendKind::GoogleGenAi,
        GoogleAuthMethod::BearerApiKey,
    ),
    (
        GoogleBackendKind::GoogleGenAi,
        GoogleAuthMethod::ExternalAuthorizer,
    ),
    (GoogleBackendKind::VertexAi, GoogleAuthMethod::Adc),
    (GoogleBackendKind::VertexAi, GoogleAuthMethod::ApiKeyExpress),
    (
        GoogleBackendKind::VertexAi,
        GoogleAuthMethod::ExternalAuthorizer,
    ),
    (
        GoogleBackendKind::GoogleCodeAssist,
        GoogleAuthMethod::GoogleOauth,
    ),
    (
        GoogleBackendKind::GoogleCodeAssist,
        GoogleAuthMethod::ComputeAdc,
    ),
    (
        GoogleBackendKind::GoogleCodeAssist,
        GoogleAuthMethod::ExternalAuthorizer,
    ),
];

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn google_oauth_refresh_error_is_permanent(error: &oauth::GoogleCodeAssistOAuthError) -> bool {
    match error {
        oauth::GoogleCodeAssistOAuthError::InteractiveLoginRequired
        | oauth::GoogleCodeAssistOAuthError::MissingRefreshToken => true,
        other => oauth_refresh_error_text_is_permanent(&other.to_string()),
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn google_oauth_refresh_error_to_provider(
    error: oauth::GoogleCodeAssistOAuthError,
    binding: &ValidatedBinding,
) -> ProviderAuthError {
    let error_text = error.to_string();
    if matches!(
        error,
        oauth::GoogleCodeAssistOAuthError::InteractiveLoginRequired
    ) {
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
fn google_managed_oauth_refresh_fn(
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
            let endpoints = oauth::code_assist_endpoints("http://127.0.0.1:0/callback");
            let runtime = oauth::GoogleCodeAssistOAuthRuntime::new(
                store,
                refresh_coord,
                endpoints,
                key.clone(),
            );
            let refreshed = runtime
                .refresh_access_token_from_persisted_without_save(&tokens)
                .await
                .map_err(|error| {
                    let permanent = google_oauth_refresh_error_is_permanent(&error);
                    let _ = fail_managed_oauth_refresh(
                        &refresh_env,
                        &binding,
                        lifecycle.clone(),
                        permanent,
                    );
                    provider_auth_error_to_auth_error(google_oauth_refresh_error_to_provider(
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

pub struct GoogleProviderRuntime;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ProviderRuntime for GoogleProviderRuntime {
    fn provider_id(&self) -> Provider {
        Provider::Gemini
    }

    fn validate_binding(
        &self,
        connection_ref: &meerkat_core::ConnectionRef,
        backend: &BackendProfile,
        auth: &AuthProfile,
        policy: &BindingPolicy,
    ) -> Result<ValidatedBinding, ProviderBindingError> {
        if backend.provider != Provider::Gemini || auth.provider != Provider::Gemini {
            return Err(ProviderBindingError::ProviderMismatch);
        }
        let backend_kind = GoogleBackendKind::parse(&backend.backend_kind).ok_or_else(|| {
            ProviderBindingError::UnknownBackendKind(backend.backend_kind.clone())
        })?;
        let auth_method = GoogleAuthMethod::parse(&auth.auth_method)
            .ok_or_else(|| ProviderBindingError::UnknownAuthMethod(auth.auth_method.clone()))?;
        if !ALLOWED_BINDINGS.contains(&(backend_kind, auth_method)) {
            return Err(ProviderBindingError::UnsupportedCombination {
                backend: backend.backend_kind.clone(),
                auth: auth.auth_method.clone(),
            });
        }
        Ok(ValidatedBinding {
            connection_ref: connection_ref.clone(),
            provider: Provider::Gemini,
            backend: NormalizedBackendKind::Google(backend_kind),
            auth: NormalizedAuthMethod::Google(auth_method),
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
            NormalizedAuthMethod::Google(m) => m,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };
        let backend_kind = match binding.backend {
            NormalizedBackendKind::Google(k) => k,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };

        let source_label = format!("google:{}", binding.auth_profile.id);
        let lease: Arc<dyn AuthLease> = match auth_method {
            GoogleAuthMethod::ApiKey
            | GoogleAuthMethod::BearerApiKey
            | GoogleAuthMethod::ApiKeyExpress => {
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
            GoogleAuthMethod::ExternalAuthorizer => {
                resolve_external_authorizer(&binding.auth_profile.source, env, binding).await?
            }
            GoogleAuthMethod::Adc => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "adc"))]
                {
                    let handle = env.auth_lease_handle.clone().ok_or_else(|| {
                        ProviderAuthError::SourceResolutionFailed(
                            "google adc requires an AuthMachine lease handle for token freshness"
                                .into(),
                        )
                    })?;
                    let mut authorizer =
                        meerkat_auth_core::authorizers::GoogleAuthAuthorizer::with_env_lookup(
                            meerkat_auth_core::authorizers::GoogleAuthChain::Default,
                            env.env_lookup.clone(),
                        );
                    authorizer = authorizer.with_auth_lease_observer(
                        handle,
                        meerkat_core::handles::LeaseKey::from_connection_ref(
                            &binding.connection_ref,
                        ),
                    );
                    let authorizer: Arc<dyn HttpAuthorizer> = Arc::new(authorizer);
                    let metadata = finalize_auth_metadata(
                        binding,
                        AuthMetadata {
                            provider_metadata: Some(meerkat_core::ProviderAuthMetadata::Google(
                                meerkat_core::GoogleAuthMetadata {
                                    project_id: backend_option_string(binding, "project_id"),
                                    region: backend_option_string(binding, "region"),
                                    ..Default::default()
                                },
                            )),
                            ..Default::default()
                        },
                    )?;
                    Arc::new(DynamicLease::from_authorizer(
                        authorizer,
                        metadata,
                        source_label.clone(),
                    ))
                }
                #[cfg(any(target_arch = "wasm32", not(feature = "adc")))]
                {
                    return Err(ProviderAuthError::SourceResolutionFailed(
                        "adc requires the gemini `adc` feature on non-wasm32".into(),
                    ));
                }
            }
            GoogleAuthMethod::ComputeAdc => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "adc"))]
                {
                    let handle = env.auth_lease_handle.clone().ok_or_else(|| {
                        ProviderAuthError::SourceResolutionFailed(
                            "google compute_adc requires an AuthMachine lease handle for token freshness"
                                .into(),
                        )
                    })?;
                    let mut authorizer =
                        meerkat_auth_core::authorizers::GoogleAuthAuthorizer::with_env_lookup(
                            meerkat_auth_core::authorizers::GoogleAuthChain::ComputeOnly,
                            env.env_lookup.clone(),
                        );
                    authorizer = authorizer.with_auth_lease_observer(
                        handle,
                        meerkat_core::handles::LeaseKey::from_connection_ref(
                            &binding.connection_ref,
                        ),
                    );
                    let authorizer: Arc<dyn HttpAuthorizer> = Arc::new(authorizer);
                    let metadata = finalize_auth_metadata(
                        binding,
                        AuthMetadata {
                            provider_metadata: Some(meerkat_core::ProviderAuthMetadata::Google(
                                meerkat_core::GoogleAuthMetadata {
                                    project_id: backend_option_string(binding, "project_id"),
                                    region: backend_option_string(binding, "region"),
                                    ..Default::default()
                                },
                            )),
                            ..Default::default()
                        },
                    )?;
                    Arc::new(DynamicLease::from_authorizer(
                        authorizer,
                        metadata,
                        source_label.clone(),
                    ))
                }
                #[cfg(any(target_arch = "wasm32", not(feature = "adc")))]
                {
                    return Err(ProviderAuthError::SourceResolutionFailed(
                        "compute_adc requires the gemini `adc` feature on non-wasm32".into(),
                    ));
                }
            }
            GoogleAuthMethod::GoogleOauth => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
                {
                    let store = env
                        .token_store
                        .as_ref()
                        .ok_or_else(|| interactive_login_error(binding))?;
                    let key =
                        meerkat_core::auth::TokenKey::from_connection_ref(&binding.connection_ref);
                    let (resolved_tokens, lease_expires_at, auth_lease_snapshot) =
                        match resolve_managed_oauth_access(
                            env,
                            binding,
                            meerkat_core::auth::PersistedAuthMode::GoogleOauth,
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
                                    oauth::code_assist_endpoints("http://127.0.0.1:0/callback");
                                let runtime = oauth::GoogleCodeAssistOAuthRuntime::new(
                                    store.clone(),
                                    coord,
                                    endpoints,
                                    key.clone(),
                                );
                                let refreshed = runtime
                                    .refresh_access_token_from_persisted_without_save(&tokens)
                                    .await
                                    .map_err(|e| {
                                        let permanent = google_oauth_refresh_error_is_permanent(&e);
                                        let _ = fail_managed_oauth_refresh(
                                            env,
                                            binding,
                                            lifecycle.clone(),
                                            permanent,
                                        );
                                        google_oauth_refresh_error_to_provider(e, binding)
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
                        };
                    let access = resolved_tokens
                        .primary_secret
                        .clone()
                        .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
                    let mut google_email: Option<String> = None;
                    let mut google_user_id: Option<String> = None;
                    // Plan §4b.12: lift OIDC claims into AuthMetadata.
                    if let Some(id_token) = resolved_tokens.id_token.as_deref()
                        && let Ok(claims) =
                            meerkat_auth_core::auth_oauth::jwt::decode_payload(id_token)
                    {
                        let lifted = oauth::GoogleIdClaims::lift_from_claims(&claims.raw);
                        google_email = lifted.email;
                        google_user_id = lifted.user_id;
                    }
                    let mut metadata = AuthMetadata::default();
                    if google_email.is_some() || google_user_id.is_some() {
                        metadata.account_id = google_user_id.or_else(|| google_email.clone());
                        metadata.provider_metadata =
                            Some(meerkat_core::ProviderAuthMetadata::Google(
                                meerkat_core::GoogleAuthMetadata {
                                    account_email: google_email,
                                    project_id: backend_option_string(binding, "project_id"),
                                    region: backend_option_string(binding, "region"),
                                    code_assist_tier: backend_option_string(binding, "tier"),
                                },
                            ));
                    }
                    let metadata = finalize_auth_metadata(binding, metadata)?;
                    let refresh = google_managed_oauth_refresh_fn(
                        env,
                        binding,
                        store.clone(),
                        key.clone(),
                        meerkat_core::auth::PersistedAuthMode::GoogleOauth,
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
                #[cfg(not(all(not(target_arch = "wasm32"), feature = "oauth")))]
                {
                    return Err(interactive_login_error(binding));
                }
            }
        };

        Ok(ResolvedConnection {
            provider: Provider::Gemini,
            backend: NormalizedBackendKind::Google(backend_kind),
            backend_profile: binding.backend_profile.clone(),
            auth_lease: lease,
        })
    }

    fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        // ProviderRuntimeRegistry dispatches on Provider enum; non-Google
        // arms are unreachable at runtime.
        let backend_kind = match connection.backend {
            NormalizedBackendKind::Google(k) => k,
            other => unreachable!(
                "GoogleProviderRuntime received non-Google backend: {other:?} \
                 — registry dispatch invariant violated"
            ),
        };
        // Authorizer-backed path (Vertex ADC, Code Assist GoogleOauth/
        // ComputeAdc, ExternalAuthorizer-dynamic). Must run before the
        // simpler secret-extraction branch because the authorizer
        // needs backend-specific wiring (Vertex vs Code Assist base
        // URLs). Plan §6.11: read the authorizer from the auth lease
        // directly.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(authorizer) = connection.resolved_authorizer() {
            let base_url = connection
                .backend_profile
                .base_url
                .clone()
                .filter(|u| !u.is_empty())
                .ok_or_else(|| {
                    ProviderClientError::InvalidBaseUrl(
                        "Google authorizer-backed backends require \
                         BackendProfile.base_url"
                            .to_string(),
                    )
                })?;
            let client = crate::GeminiClient::new_with_base_url(String::new(), base_url)
                .with_authorizer(authorizer);
            return Ok(Arc::new(client));
        }
        #[cfg(target_arch = "wasm32")]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::MissingFeature(
                "google-authorizer-backed auth not available on wasm32",
            ))?;
        #[cfg(not(target_arch = "wasm32"))]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        match backend_kind {
            GoogleBackendKind::GoogleGenAi => {
                // S1-verified: GeminiClient::new returns Self (infallible).
                let client = match &connection.backend_profile.base_url {
                    Some(url) => crate::GeminiClient::new_with_base_url(secret, url.clone()),
                    None => crate::GeminiClient::new(secret),
                };
                Ok(Arc::new(client))
            }
            GoogleBackendKind::VertexAi => {
                // VertexAi `api_key_express` + `bearer_api_key` use the
                // Vertex-region URL (per BackendProfile.base_url) with
                // the same generative-language wire as GoogleGenAi. ADC
                // + ExternalAuthorizer paths arrive via the Authorizer
                // (deleted shim path). For a raw secret, we
                // treat it as the api_key_express path.
                let base_url = connection
                    .backend_profile
                    .base_url
                    .clone()
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| {
                        ProviderClientError::InvalidBaseUrl(
                            "vertex_ai backend requires BackendProfile.base_url \
                             (e.g. https://<region>-aiplatform.googleapis.com)"
                                .to_string(),
                        )
                    })?;
                let client = crate::GeminiClient::new_with_base_url(secret, base_url);
                Ok(Arc::new(client))
            }
            GoogleBackendKind::GoogleCodeAssist => {
                // Code Assist with a pre-resolved bearer secret (e.g.
                // ExternalAuthorizer→Secret subpath, where the host
                // resolved an OAuth access token via its own flow).
                // Wire as GeminiClient with StaticBearerAuthorizer
                // pointed at the Code Assist base URL. Requires
                // BackendProfile.base_url (Code Assist endpoint varies
                // by tier — production is
                // https://cloudcode-pa.googleapis.com).
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let base_url = connection
                        .backend_profile
                        .base_url
                        .clone()
                        .filter(|u| !u.is_empty())
                        .ok_or_else(|| {
                            ProviderClientError::InvalidBaseUrl(
                                "google_code_assist backend requires \
                                 BackendProfile.base_url (e.g. \
                                 https://cloudcode-pa.googleapis.com)"
                                    .to_string(),
                            )
                        })?;
                    let authorizer: std::sync::Arc<dyn meerkat_core::HttpAuthorizer> =
                        std::sync::Arc::new(
                            meerkat_auth_core::authorizers::StaticBearerAuthorizer::new(
                                secret,
                                "code-assist-bearer",
                            ),
                        );
                    let client = crate::GeminiClient::new_with_base_url(String::new(), base_url)
                        .with_authorizer(authorizer);
                    Ok(Arc::new(client))
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = secret;
                    Err(ProviderClientError::MissingFeature(
                        "google_code_assist backend not available on wasm32",
                    ))
                }
            }
        }
    }

    fn build_image_generation_executor(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Option<Arc<dyn ImageGenerationExecutor>>, ProviderClientError> {
        let backend_kind = match connection.backend {
            NormalizedBackendKind::Google(k) => k,
            other => unreachable!(
                "GoogleProviderRuntime received non-Google backend: {other:?} \
                 — registry dispatch invariant violated"
            ),
        };
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(authorizer) = connection.resolved_authorizer() {
            let base_url = connection
                .backend_profile
                .base_url
                .clone()
                .filter(|u| !u.is_empty())
                .ok_or_else(|| {
                    ProviderClientError::InvalidBaseUrl(
                        "Google authorizer-backed backends require BackendProfile.base_url"
                            .to_string(),
                    )
                })?;
            let client = crate::GeminiClient::new_with_base_url(String::new(), base_url)
                .with_authorizer(authorizer);
            return Ok(Some(Arc::new(client)));
        }
        #[cfg(target_arch = "wasm32")]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::MissingFeature(
                "google-authorizer-backed auth not available on wasm32",
            ))?;
        #[cfg(not(target_arch = "wasm32"))]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        let client = match backend_kind {
            GoogleBackendKind::GoogleGenAi => match &connection.backend_profile.base_url {
                Some(url) => crate::GeminiClient::new_with_base_url(secret, url.clone()),
                None => crate::GeminiClient::new(secret),
            },
            GoogleBackendKind::VertexAi | GoogleBackendKind::GoogleCodeAssist => {
                let base_url = connection
                    .backend_profile
                    .base_url
                    .clone()
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| {
                        ProviderClientError::InvalidBaseUrl(
                            "google image executor backend requires BackendProfile.base_url"
                                .to_string(),
                        )
                    })?;
                crate::GeminiClient::new_with_base_url(secret, base_url)
            }
        };
        Ok(Some(Arc::new(client)))
    }

    fn image_generation_profile(
        &self,
    ) -> Option<Arc<dyn meerkat_core::ImageGenerationProviderProfile>> {
        Some(Arc::new(crate::GeminiImageGenerationProfile))
    }
}

#[cfg(any(
    all(not(target_arch = "wasm32"), feature = "adc"),
    all(not(target_arch = "wasm32"), feature = "oauth")
))]
fn backend_option_string(binding: &ValidatedBinding, key: &str) -> Option<String> {
    binding
        .backend_profile
        .options
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn allowed_bindings_cover_three_backends() {
        assert!(
            ALLOWED_BINDINGS.contains(&(GoogleBackendKind::GoogleGenAi, GoogleAuthMethod::ApiKey,))
        );
        assert!(ALLOWED_BINDINGS.contains(&(GoogleBackendKind::VertexAi, GoogleAuthMethod::Adc,)));
        assert!(ALLOWED_BINDINGS.contains(&(
            GoogleBackendKind::GoogleCodeAssist,
            GoogleAuthMethod::GoogleOauth,
        )));
    }

    #[test]
    fn provider_id_is_gemini() {
        assert_eq!(GoogleProviderRuntime.provider_id(), Provider::Gemini);
    }
}
