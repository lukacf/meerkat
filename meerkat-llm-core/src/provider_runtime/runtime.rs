//! The `ProviderRuntime` trait — per-provider implementation of credential
//! resolution and client construction.

use std::sync::Arc;

use async_trait::async_trait;

use meerkat_core::{ImageGenerationProviderProfile, Provider};

use crate::provider_runtime::binding::{ResolvedConnection, ValidatedBinding};
use crate::provider_runtime::errors::{ProviderAuthError, ProviderClientError};
use crate::provider_runtime::registry::ResolverEnvironment;
use crate::realtime_session::RealtimeSessionFactory;
use crate::{ImageGenerationExecutor, LlmClient};

/// Per-provider runtime contract: resolve credentials and construct clients.
///
/// Backend/auth compatibility is owned by
/// [`ProviderRuntimeCatalog`](crate::provider_runtime::catalog::ProviderRuntimeCatalog)
/// and applied by
/// [`ProviderRuntimeRegistry`](crate::provider_runtime::registry::ProviderRuntimeRegistry)
/// before dispatching to a runtime.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait ProviderRuntime: Send + Sync {
    /// Return the provider identity this runtime owns.
    fn provider_id(&self) -> Provider;

    /// Resolve credential material per `CredentialSourceSpec` and wrap in a
    /// lease. Populates `ResolvedConnection.shim_credential` for Phase 2.
    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError>;

    /// Construct a concrete LlmClient from the resolved connection.
    /// Phase 2 shim: extracts shim_credential and hands it to the legacy
    /// provider constructors.
    fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError>;

    /// Construct a realtime-capable text-turn client from a resolved
    /// provider connection. Provider runtimes own the backend/auth mechanics
    /// for this specialized transport; shared factory code only decides
    /// whether the selected model requires realtime routing.
    fn build_realtime_text_client(
        &self,
        _connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        Err(ProviderClientError::MissingFeature("realtime-text"))
    }

    /// Construct a provider-neutral realtime session factory from a resolved
    /// provider connection. Provider runtimes own the backend/auth gating for
    /// the realtime socket — the same fail-closed matrix as
    /// [`Self::build_realtime_text_client`] — and mint the concrete adapter
    /// in-crate, so credential material never leaves the provider seam.
    fn build_realtime_session_factory(
        &self,
        _connection: ResolvedConnection,
    ) -> Result<Arc<dyn RealtimeSessionFactory>, ProviderClientError> {
        Err(ProviderClientError::MissingFeature("realtime-session"))
    }

    /// Construct an optional image-generation executor from the same resolved
    /// provider connection used for the text client.
    fn build_image_generation_executor(
        &self,
        _connection: ResolvedConnection,
    ) -> Result<Option<Arc<dyn ImageGenerationExecutor>>, ProviderClientError> {
        Ok(None)
    }

    /// Return provider-owned image model planning rules, if this runtime
    /// supports the `generate_image` substrate.
    fn image_generation_profile(&self) -> Option<Arc<dyn ImageGenerationProviderProfile>> {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::provider_runtime::binding::{NormalizedAuthMethod, NormalizedBackendKind};
    use meerkat_core::{AuthMetadata, BindingPolicy, ResolvedAuthKind};

    // Minimal MockRuntime to exercise object safety and trait wiring.
    struct MockRuntime;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl ProviderRuntime for MockRuntime {
        fn provider_id(&self) -> Provider {
            Provider::Other
        }
        async fn resolve_binding(
            &self,
            _binding: &ValidatedBinding,
            _env: &ResolverEnvironment,
        ) -> Result<ResolvedConnection, ProviderAuthError> {
            Err(ProviderAuthError::Auth(
                meerkat_core::AuthError::InteractiveLoginRequired,
            ))
        }
        fn build_client(
            &self,
            _connection: ResolvedConnection,
        ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
            Err(ProviderClientError::MissingFeature("mock"))
        }
    }

    #[test]
    fn realtime_session_factory_defaults_to_missing_feature() {
        use crate::provider_runtime::binding::StaticLease;

        let runtime: Arc<dyn ProviderRuntime> = Arc::new(MockRuntime);
        let connection = ResolvedConnection {
            provider: Provider::Other,
            backend: NormalizedBackendKind::OpenAi(
                meerkat_core::provider_matrix::OpenAiBackendKind::OpenAiApi,
            ),
            backend_profile: Arc::new(meerkat_core::BackendProfile {
                id: "b".into(),
                provider: Provider::Other,
                backend_kind: "other_api".into(),
                base_url: None,
                options: serde_json::Value::Null,
            }),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "secret".into(),
                AuthMetadata::default(),
                None,
                "test",
            )),
        };
        let err = match runtime.build_realtime_session_factory(connection) {
            Ok(_) => panic!("default realtime session factory must fail closed"),
            Err(err) => err,
        };
        assert!(matches!(
            err,
            ProviderClientError::MissingFeature("realtime-session")
        ));
    }

    #[test]
    fn mock_is_object_safe() {
        let runtime: Arc<dyn ProviderRuntime> = Arc::new(MockRuntime);
        assert_eq!(runtime.provider_id(), Provider::Other);

        // Exercise the other method signatures to prove trait object wiring.
        let _env = ResolverEnvironment::testing();
        let _ = ResolvedAuthKind::None;
        let _ = AuthMetadata::default();
        let _ = BindingPolicy::default();
        // Variants that require provider features are exercised in their
        // own module tests.
        let _ = std::marker::PhantomData::<NormalizedBackendKind>;
        let _ = std::marker::PhantomData::<NormalizedAuthMethod>;
    }
}
