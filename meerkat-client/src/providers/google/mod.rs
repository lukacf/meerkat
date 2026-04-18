//! Google provider runtime (Gemini API, Vertex AI, Code Assist).

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

pub use auth::GoogleAuthMethod;
pub use backend::GoogleBackendKind;

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
        backend: &BackendProfile,
        auth: &AuthProfile,
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
            provider: Provider::Gemini,
            backend: NormalizedBackendKind::Google(backend_kind),
            auth: NormalizedAuthMethod::Google(auth_method),
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

        let shim_credential = match auth_method {
            GoogleAuthMethod::ApiKey | GoogleAuthMethod::BearerApiKey => {
                resolve_simple_secret(&binding.auth_profile.source, env, binding).await?
            }
            GoogleAuthMethod::ExternalAuthorizer => {
                resolve_external_authorizer(&binding.auth_profile.source, env, binding).await?
            }
            // api_key_express uses the simple-secret path (it's just an
            // API key passed through Vertex).
            GoogleAuthMethod::ApiKeyExpress => {
                resolve_simple_secret(&binding.auth_profile.source, env, binding).await?
            }
            // ADC / compute ADC drive through GoogleAuthAuthorizer at
            // request time; we return the authorizer marker via the
            // ResolvedConnection's shim_credential.
            GoogleAuthMethod::Adc | GoogleAuthMethod::ComputeAdc => {
                // The real authorizer is constructed by build_client /
                // Phase 6 owned-HTTP; here we mark the credential as
                // dynamic.
                ShimCredential::Authorizer
            }
            GoogleAuthMethod::GoogleOauth => {
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
                    let key =
                        crate::auth_store::TokenKey::new(realm_id, binding.auth_profile.id.clone());
                    let persisted = store
                        .load(&key)
                        .await
                        .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?
                        .ok_or(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired))?;
                    use chrono::{Duration, Utc};
                    let fresh = persisted
                        .expires_at
                        .is_none_or(|exp| exp - Utc::now() > Duration::seconds(60));
                    if let (true, Some(access)) = (fresh, persisted.primary_secret.clone()) {
                        ShimCredential::Secret(access)
                    } else {
                        let coord = env.refresh_coord.clone().unwrap_or_else(|| {
                            Arc::new(crate::auth_store::InMemoryCoordinator::new())
                        });
                        let endpoints = oauth::code_assist_endpoints("http://127.0.0.1:0/callback");
                        let runtime = oauth::GoogleCodeAssistOAuthRuntime::new(
                            store.clone(),
                            coord,
                            endpoints,
                            key,
                        );
                        let access =
                            runtime
                                .get_or_refresh_access_token()
                                .await
                                .map_err(|e| match e {
                                    oauth::GoogleCodeAssistOAuthError::InteractiveLoginRequired => {
                                        ProviderAuthError::Auth(AuthError::InteractiveLoginRequired)
                                    }
                                    other => {
                                        ProviderAuthError::SourceResolutionFailed(other.to_string())
                                    }
                                })?;
                        ShimCredential::Secret(access)
                    }
                }
                #[cfg(not(all(not(target_arch = "wasm32"), feature = "oauth")))]
                {
                    return Err(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired));
                }
            }
        };

        // Plan §6.11 migration: populate the lease with real credential
        // material (StaticLease with __secret__ header OR empty for the
        // authorizer path — build_client constructs the concrete
        // HttpAuthorizer on-demand for ADC / Compute / OAuth flows).
        let source_label = format!("google:{}", binding.auth_profile.id);
        let lease: Arc<dyn meerkat_core::AuthLease> = match &shim_credential {
            ShimCredential::Secret(s) => Arc::new(StaticLease::new(
                vec![("__secret__".to_string(), s.clone())],
                AuthMetadata::default(),
                None,
                source_label,
            )),
            ShimCredential::Authorizer | ShimCredential::None => {
                Arc::new(StaticLease::empty(AuthMetadata::default(), source_label))
            }
        };

        Ok(ResolvedConnection {
            provider: Provider::Gemini,
            backend: NormalizedBackendKind::Google(backend_kind),
            backend_profile: binding.backend_profile.clone(),
            auth_lease: lease,
            shim_credential,
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
            let client = crate::gemini::GeminiClient::new_with_base_url(String::new(), base_url)
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
                    Some(url) => {
                        crate::gemini::GeminiClient::new_with_base_url(secret, url.clone())
                    }
                    None => crate::gemini::GeminiClient::new(secret),
                };
                Ok(Arc::new(client))
            }
            GoogleBackendKind::VertexAi => {
                // VertexAi `api_key_express` + `bearer_api_key` use the
                // Vertex-region URL (per BackendProfile.base_url) with
                // the same generative-language wire as GoogleGenAi. ADC
                // + ExternalAuthorizer paths arrive via the Authorizer
                // ShimCredential arm (not here). For a raw secret, we
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
                let client = crate::gemini::GeminiClient::new_with_base_url(secret, base_url);
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
                        std::sync::Arc::new(crate::authorizers::StaticBearerAuthorizer::new(
                            secret,
                            "code-assist-bearer",
                        ));
                    let client =
                        crate::gemini::GeminiClient::new_with_base_url(String::new(), base_url)
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
