//! OpenAI provider runtime.

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

pub use auth::OpenAiAuthMethod;
pub use backend::OpenAiBackendKind;

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
        backend: &BackendProfile,
        auth: &AuthProfile,
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
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(backend_kind),
            auth: NormalizedAuthMethod::OpenAi(auth_method),
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

        let shim_credential = match auth_method {
            OpenAiAuthMethod::ApiKey | OpenAiAuthMethod::StaticBearer => {
                resolve_simple_secret(&binding.auth_profile.source, env, binding).await?
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
                        OpenAiAuthMethod::ExternalChatGptTokens => {
                            // External tokens are not refreshed by
                            // meerkat — the host manages the lifecycle.
                            // We just read the persisted access_token.
                            let secret = persisted
                                .primary_secret
                                .clone()
                                .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
                            ShimCredential::Secret(secret)
                        }
                        OpenAiAuthMethod::ManagedChatGptOauth => {
                            use chrono::{Duration, Utc};
                            let fresh = persisted
                                .expires_at
                                .is_none_or(|exp| exp - Utc::now() > Duration::seconds(60));
                            if let (true, Some(access)) = (fresh, persisted.primary_secret.clone())
                            {
                                ShimCredential::Secret(access)
                            } else {
                                let coord = env.refresh_coord.clone().unwrap_or_else(|| {
                                    Arc::new(crate::auth_store::InMemoryCoordinator::new())
                                });
                                let endpoints =
                                    oauth::chatgpt_endpoints("http://127.0.0.1:0/callback");
                                let runtime = oauth::OpenAiOAuthRuntime::new(
                                    store.clone(),
                                    coord,
                                    endpoints,
                                    key,
                                );
                                let access = runtime.get_or_refresh_access_token().await.map_err(
                                    |e| match e {
                                        oauth::OpenAiOAuthError::InteractiveLoginRequired => {
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
            format!("openai:{}", binding.auth_profile.id),
        ));

        Ok(ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(backend_kind),
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
            NormalizedBackendKind::OpenAi(k) => k,
            _ => {
                return Err(ProviderClientError::MissingFeature(
                    "openai-provider-mismatch",
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
            OpenAiBackendKind::OpenAiApi => {
                // S1-verified: OpenAiClient::new returns Self (infallible).
                let client = match &connection.backend_profile.base_url {
                    Some(url) => {
                        crate::openai::OpenAiClient::new_with_base_url(secret, url.clone())
                    }
                    None => crate::openai::OpenAiClient::new(secret),
                };
                Ok(Arc::new(client))
            }
            OpenAiBackendKind::ChatGptBackend => {
                Err(ProviderClientError::MissingFeature("openai-chatgpt-auth"))
            }
        }
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
            storage: Default::default(),
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        }
    }

    #[test]
    fn validate_accepts_allowed_combination() {
        let rt = OpenAiProviderRuntime;
        let vb = rt
            .validate_binding(&backend("openai_api"), &auth("api_key"))
            .expect("allowed combination");
        assert_eq!(vb.provider, Provider::OpenAI);
    }

    #[test]
    fn validate_rejects_unknown_backend_kind() {
        let rt = OpenAiProviderRuntime;
        let err = rt
            .validate_binding(&backend("bogus_backend"), &auth("api_key"))
            .unwrap_err();
        assert!(matches!(err, ProviderBindingError::UnknownBackendKind(_)));
    }

    #[test]
    fn validate_rejects_unsupported_combo() {
        // openai_api + managed_chatgpt_oauth is NOT in ALLOWED_BINDINGS.
        let rt = OpenAiProviderRuntime;
        let err = rt
            .validate_binding(&backend("openai_api"), &auth("managed_chatgpt_oauth"))
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
        let err = rt.validate_binding(&wrong, &auth("api_key")).unwrap_err();
        assert!(matches!(err, ProviderBindingError::ProviderMismatch));
    }
}
