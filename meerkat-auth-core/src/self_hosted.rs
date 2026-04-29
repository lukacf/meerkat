//! Self-hosted provider credential runtime.
//!
//! Self-hosted OpenAI-compatible clients are constructed by higher layers
//! because model aliases provide transport details, but credential resolution
//! still belongs to the canonical provider runtime seam.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::{
    AuthLease, AuthMetadata, AuthProfile, BackendProfile, BindingPolicy, ConnectionRef, Provider,
};
use meerkat_llm_core::LlmClient;
use meerkat_llm_core::provider_runtime::{
    NormalizedAuthMethod, NormalizedBackendKind, ProviderBindingError, ProviderClientError,
    ProviderRuntime, ResolvedConnection, ResolverEnvironment, StaticLease, ValidatedBinding,
};

pub struct SelfHostedProviderRuntime;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ProviderRuntime for SelfHostedProviderRuntime {
    fn provider_id(&self) -> Provider {
        Provider::SelfHosted
    }

    fn validate_binding(
        &self,
        connection_ref: &ConnectionRef,
        backend: &BackendProfile,
        auth: &AuthProfile,
        policy: &BindingPolicy,
    ) -> Result<ValidatedBinding, ProviderBindingError> {
        if backend.provider != Provider::SelfHosted || auth.provider != Provider::SelfHosted {
            return Err(ProviderBindingError::ProviderMismatch);
        }
        if !matches!(
            backend.backend_kind.as_str(),
            "self_hosted" | "openai_compatible"
        ) {
            return Err(ProviderBindingError::UnknownBackendKind(
                backend.backend_kind.clone(),
            ));
        }
        let auth_method = match auth.auth_method.as_str() {
            "api_key" => meerkat_core::provider_matrix::openai::OpenAiAuthMethod::ApiKey,
            "none" | "static_bearer" => {
                meerkat_core::provider_matrix::openai::OpenAiAuthMethod::StaticBearer
            }
            other => return Err(ProviderBindingError::UnknownAuthMethod(other.to_string())),
        };

        Ok(ValidatedBinding {
            connection_ref: connection_ref.clone(),
            provider: Provider::SelfHosted,
            backend: NormalizedBackendKind::OpenAi(
                meerkat_core::provider_matrix::openai::OpenAiBackendKind::OpenAiApi,
            ),
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
    ) -> Result<ResolvedConnection, meerkat_llm_core::provider_runtime::ProviderAuthError> {
        let source_label = format!("self_hosted:{}", binding.auth_profile.id);
        let lease: Arc<dyn AuthLease> = match binding.auth_profile.auth_method.as_str() {
            "none" => Arc::new(StaticLease::empty_lease(
                AuthMetadata::default(),
                source_label,
            )),
            _ => {
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
