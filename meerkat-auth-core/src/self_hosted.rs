//! Self-hosted provider credential runtime.
//!
//! Self-hosted OpenAI-compatible clients are constructed by higher layers
//! because model aliases provide transport details, but credential resolution
//! still belongs to the canonical provider runtime seam.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::provider_matrix::SelfHostedAuthMethod;
use meerkat_core::{AuthLease, AuthMetadata, Provider};
use meerkat_llm_core::LlmClient;
use meerkat_llm_core::provider_runtime::{
    NormalizedAuthMethod, NormalizedBackendKind, ProviderAuthError, ProviderBindingError,
    ProviderClientError, ProviderRuntime, ResolvedConnection, ResolverEnvironment, StaticLease,
    ValidatedBinding,
};

pub struct SelfHostedProviderRuntime;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ProviderRuntime for SelfHostedProviderRuntime {
    fn provider_id(&self) -> Provider {
        Provider::SelfHosted
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        if binding.provider != Provider::SelfHosted {
            return Err(ProviderAuthError::Binding(
                ProviderBindingError::ProviderMismatch,
            ));
        }
        match binding.backend {
            NormalizedBackendKind::SelfHosted(_) => {}
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        }
        let auth_method = match binding.auth {
            NormalizedAuthMethod::SelfHosted(method) => method,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };
        let source_label = format!("self_hosted:{}", binding.auth_profile.id);
        let lease: Arc<dyn AuthLease> = match auth_method {
            SelfHostedAuthMethod::None => Arc::new(StaticLease::empty_lease(
                AuthMetadata::default(),
                source_label,
            )),
            SelfHostedAuthMethod::ApiKey | SelfHostedAuthMethod::StaticBearer => {
                let secret = crate::resolver::resolve_simple_secret(
                    &binding.auth_profile.source,
                    env,
                    binding,
                )
                .await?;
                Arc::new(StaticLease::inline_secret(
                    secret,
                    AuthMetadata::default(),
                    None,
                    source_label,
                ))
            }
        };

        Ok(ResolvedConnection {
            provider: Provider::SelfHosted,
            backend: binding.backend,
            backend_profile: Arc::clone(&binding.backend_profile),
            auth_lease: lease,
        })
    }

    fn build_client(
        &self,
        _connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        Err(ProviderClientError::MissingFeature(
            "self-hosted client construction is surface-owned because model aliases carry remote_model/api_style",
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::provider_matrix::{
        OpenAiAuthMethod, OpenAiBackendKind, SelfHostedAuthMethod, SelfHostedBackendKind,
    };
    use meerkat_core::{
        AuthProfile, BackendProfile, BindingId, BindingPolicy, ConnectionRef, CredentialSourceSpec,
        RealmId,
    };

    fn connection_ref() -> ConnectionRef {
        ConnectionRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default").unwrap(),
            profile: None,
        }
    }

    fn backend_profile(provider: Provider, backend_kind: &str) -> BackendProfile {
        BackendProfile {
            id: "backend".into(),
            provider,
            backend_kind: backend_kind.into(),
            base_url: None,
            options: serde_json::Value::Null,
        }
    }

    fn auth_profile(provider: Provider, auth_method: &str) -> AuthProfile {
        AuthProfile {
            id: "auth".into(),
            provider,
            auth_method: auth_method.into(),
            source: CredentialSourceSpec::InlineSecret {
                secret: "secret".into(),
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        }
    }

    fn self_hosted_binding() -> ValidatedBinding {
        ValidatedBinding {
            connection_ref: connection_ref(),
            provider: Provider::SelfHosted,
            backend: NormalizedBackendKind::SelfHosted(SelfHostedBackendKind::SelfHosted),
            auth: NormalizedAuthMethod::SelfHosted(SelfHostedAuthMethod::None),
            backend_profile: Arc::new(backend_profile(
                Provider::SelfHosted,
                SelfHostedBackendKind::SelfHosted.as_str(),
            )),
            auth_profile: Arc::new(auth_profile(
                Provider::SelfHosted,
                SelfHostedAuthMethod::None.as_str(),
            )),
            policy: BindingPolicy::default(),
        }
    }

    #[tokio::test]
    async fn direct_resolve_rejects_mismatched_provider_tag() {
        let mut binding = self_hosted_binding();
        binding.provider = Provider::OpenAI;

        let err = SelfHostedProviderRuntime
            .resolve_binding(&binding, &ResolverEnvironment::testing())
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Binding(ProviderBindingError::ProviderMismatch)
        ));
    }

    #[tokio::test]
    async fn direct_resolve_rejects_non_self_hosted_backend_tag() {
        let mut binding = self_hosted_binding();
        binding.backend = NormalizedBackendKind::OpenAi(OpenAiBackendKind::OpenAiApi);
        binding.backend_profile = Arc::new(backend_profile(
            Provider::OpenAI,
            OpenAiBackendKind::OpenAiApi.as_str(),
        ));

        let err = SelfHostedProviderRuntime
            .resolve_binding(&binding, &ResolverEnvironment::testing())
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Binding(ProviderBindingError::ProviderMismatch)
        ));
    }

    #[tokio::test]
    async fn direct_resolve_rejects_non_self_hosted_auth_tag() {
        let mut binding = self_hosted_binding();
        binding.auth = NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::ApiKey);
        binding.auth_profile = Arc::new(auth_profile(
            Provider::OpenAI,
            OpenAiAuthMethod::ApiKey.as_str(),
        ));

        let err = SelfHostedProviderRuntime
            .resolve_binding(&binding, &ResolverEnvironment::testing())
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Binding(ProviderBindingError::ProviderMismatch)
        ));
    }
}
