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
use meerkat_auth_core::resolver::refresh_allowed;
use meerkat_auth_core::resolver::{
    finalize_auth_metadata, interactive_login_error, resolve_external_authorizer,
    resolve_simple_secret,
};
use meerkat_llm_core::LlmClient;
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
            GoogleAuthMethod::ExternalAuthorizer => {
                resolve_external_authorizer(&binding.auth_profile.source, env, binding).await?
            }
            GoogleAuthMethod::Adc => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "adc"))]
                {
                    let authorizer: Arc<dyn HttpAuthorizer> = Arc::new(
                        meerkat_auth_core::authorizers::GoogleAuthAuthorizer::with_env_lookup(
                            meerkat_auth_core::authorizers::GoogleAuthChain::Default,
                            env.env_lookup.clone(),
                        ),
                    );
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
                    Arc::new(DynamicLease::new(
                        authorizer,
                        metadata,
                        None,
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
                    let authorizer: Arc<dyn HttpAuthorizer> = Arc::new(
                        meerkat_auth_core::authorizers::GoogleAuthAuthorizer::with_env_lookup(
                            meerkat_auth_core::authorizers::GoogleAuthChain::ComputeOnly,
                            env.env_lookup.clone(),
                        ),
                    );
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
                    Arc::new(DynamicLease::new(
                        authorizer,
                        metadata,
                        None,
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
                    let persisted = store
                        .load(&key)
                        .await
                        .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?
                        .ok_or_else(|| interactive_login_error(binding))?;
                    let mut google_email: Option<String> = None;
                    let mut google_user_id: Option<String> = None;
                    // Plan §4b.12: lift OIDC claims into AuthMetadata.
                    if let Some(id_token) = persisted.id_token.as_deref()
                        && let Ok(claims) =
                            meerkat_auth_core::auth_oauth::jwt::decode_payload(id_token)
                    {
                        let lifted = oauth::GoogleIdClaims::lift_from_claims(&claims.raw);
                        google_email = lifted.email;
                        google_user_id = lifted.user_id;
                    }
                    use chrono::{Duration, Utc};
                    let fresh = persisted
                        .expires_at
                        .is_none_or(|exp| exp - Utc::now() > Duration::seconds(60));
                    let access = if let (true, Some(access)) =
                        (fresh, persisted.primary_secret.clone())
                    {
                        access
                    } else {
                        if !refresh_allowed(binding) {
                            return Err(ProviderAuthError::Auth(AuthError::Expired));
                        }
                        let coord = env.refresh_coord.clone().unwrap_or_else(|| {
                            Arc::new(meerkat_auth_core::InMemoryCoordinator::new())
                        });
                        let endpoints = oauth::code_assist_endpoints("http://127.0.0.1:0/callback");
                        let runtime = oauth::GoogleCodeAssistOAuthRuntime::new(
                            store.clone(),
                            coord,
                            endpoints,
                            key,
                        );
                        runtime
                            .get_or_refresh_access_token()
                            .await
                            .map_err(|e| match e {
                                oauth::GoogleCodeAssistOAuthError::InteractiveLoginRequired => {
                                    interactive_login_error(binding)
                                }
                                other => {
                                    ProviderAuthError::SourceResolutionFailed(other.to_string())
                                }
                            })?
                    };
                    let mut metadata = AuthMetadata::default();
                    if google_email.is_some() || google_user_id.is_some() {
                        metadata.account_id =
                            google_user_id.clone().or_else(|| google_email.clone());
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
