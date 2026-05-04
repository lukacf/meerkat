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
        if binding.provider() != Provider::SelfHosted {
            return Err(ProviderAuthError::Binding(
                ProviderBindingError::ProviderMismatch,
            ));
        }
        match binding.backend() {
            NormalizedBackendKind::SelfHosted(_) => {}
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        }
        let auth_method = match binding.auth() {
            NormalizedAuthMethod::SelfHosted(method) => method,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };
        let source_label = format!("self_hosted:{}", binding.auth_profile().id);
        let lease: Arc<dyn AuthLease> = match auth_method {
            SelfHostedAuthMethod::None => Arc::new(StaticLease::empty_lease(
                AuthMetadata::default(),
                source_label,
            )),
            SelfHostedAuthMethod::ApiKey | SelfHostedAuthMethod::StaticBearer => {
                let secret = crate::resolver::resolve_simple_secret(
                    &binding.auth_profile().source,
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
            backend: binding.backend(),
            backend_profile: Arc::clone(binding.backend_profile()),
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
    use meerkat_core::provider_matrix::{SelfHostedAuthMethod, SelfHostedBackendKind};
    use meerkat_core::{
        AuthBindingRef, AuthProfile, BackendProfile, BindingId, BindingPolicy,
        CredentialSourceSpec, RealmId,
    };
    use meerkat_llm_core::provider_runtime::ProviderRuntimeCatalog;

    fn auth_binding() -> AuthBindingRef {
        AuthBindingRef {
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
        let backend = backend_profile(
            Provider::SelfHosted,
            SelfHostedBackendKind::SelfHosted.as_str(),
        );
        let auth = auth_profile(Provider::SelfHosted, SelfHostedAuthMethod::None.as_str());
        ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &backend,
            &auth,
            &BindingPolicy::default(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn direct_resolve_accepts_catalog_validated_self_hosted_none() {
        let resolved = SelfHostedProviderRuntime
            .resolve_binding(&self_hosted_binding(), &ResolverEnvironment::testing())
            .await
            .unwrap();

        assert_eq!(resolved.provider, Provider::SelfHosted);
        assert!(matches!(
            resolved.backend,
            NormalizedBackendKind::SelfHosted(SelfHostedBackendKind::SelfHosted)
        ));
    }

    #[test]
    fn catalog_rejects_mismatched_self_hosted_provider_tags() {
        let backend = backend_profile(
            Provider::SelfHosted,
            SelfHostedBackendKind::SelfHosted.as_str(),
        );
        let auth = auth_profile(Provider::OpenAI, "api_key");
        let err = ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &backend,
            &auth,
            &BindingPolicy::default(),
        )
        .unwrap_err();

        assert!(matches!(err, ProviderBindingError::ProviderMismatch));
    }

    #[test]
    fn catalog_rejects_non_self_hosted_backend_for_self_hosted_provider() {
        let backend = backend_profile(Provider::OpenAI, "openai_api");
        let auth = auth_profile(Provider::SelfHosted, SelfHostedAuthMethod::None.as_str());
        let err = ProviderRuntimeCatalog::validate_binding_for_provider(
            Provider::SelfHosted,
            &auth_binding(),
            &backend,
            &auth,
            &BindingPolicy::default(),
        )
        .unwrap_err();

        assert!(matches!(err, ProviderBindingError::ProviderMismatch));
    }
}
