//! The `ProviderRuntime` trait — per-provider implementation of validation,
//! resolution, and client construction.

use std::sync::Arc;

use async_trait::async_trait;

use meerkat_core::{
    AuthProfile, BackendProfile, BindingPolicy, ConnectionRef, ImageGenerationProviderProfile,
    Provider,
};

use crate::provider_runtime::binding::{ResolvedConnection, ValidatedBinding};
use crate::provider_runtime::errors::{
    ProviderAuthError, ProviderBindingError, ProviderClientError,
};
use crate::provider_runtime::registry::ResolverEnvironment;
use crate::{ImageGenerationExecutor, LlmClient};

/// Per-provider runtime contract: validate a binding, resolve credentials,
/// construct an LlmClient.
///
/// Implementations live in `crate::providers::{openai,anthropic,google}`.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait ProviderRuntime: Send + Sync {
    /// Return the provider identity this runtime owns.
    fn provider_id(&self) -> Provider;

    /// Normalize backend + auth strings into typed enums and verify the
    /// combination is listed in the runtime's `ALLOWED_BINDINGS` table.
    ///
    /// `policy` carries the binding's declared auth policy
    /// (`allow_auth_override`, metadata requirements). Implementations
    /// MUST populate `ValidatedBinding.policy` from this argument — the
    /// policy is a contract fact about the binding, not an injectable
    /// default. Dogma §16 (binding policy flows through resolution):
    /// dropping it at validation silently substitutes the wrong policy
    /// and bleeds defaults across bindings that declared otherwise.
    fn validate_binding(
        &self,
        connection_ref: &ConnectionRef,
        backend: &BackendProfile,
        auth: &AuthProfile,
        policy: &BindingPolicy,
    ) -> Result<ValidatedBinding, ProviderBindingError>;

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
        fn validate_binding(
            &self,
            _connection_ref: &ConnectionRef,
            _backend: &BackendProfile,
            _auth: &AuthProfile,
            _policy: &BindingPolicy,
        ) -> Result<ValidatedBinding, ProviderBindingError> {
            Err(ProviderBindingError::ProviderMismatch)
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
