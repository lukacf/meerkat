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
    NormalizedAuthMethod, ProviderAuthError, ProviderBindingError, ProviderClientError,
    ProviderRuntime, ResolvedConnection, ResolverEnvironment, StaticLease, ValidatedBinding,
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
