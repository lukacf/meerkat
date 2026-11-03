//! The `ProviderRuntime` trait — per-provider implementation of credential
//! resolution and client construction.

use std::sync::Arc;

use async_trait::async_trait;

use meerkat_core::{ImageGenerationProviderProfile, Provider};

use crate::provider_runtime::binding::{
    ResolvedConnection, ResolvedRealtimeTarget, ResolvedTextTarget, ValidatedBinding,
};
use crate::provider_runtime::errors::{
    AccountModelRefusal, ProviderAuthError, ProviderClientError,
};
use crate::provider_runtime::registry::ResolverEnvironment;
use crate::realtime_session::RealtimeSessionFactory;
use crate::{ImageGenerationExecutor, LlmClient};

/// What the account behind a resolved connection states about one model,
/// for a backend whose accounts state the models they serve (a GitHub
/// Copilot account's model list). Asked only about a model the model
/// registry does not define itself.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AccountModelLookup {
    /// The backend's accounts state no model list: the model registry alone
    /// defines its models.
    NotReported,
    /// The account offers the model: its definition for the model registry,
    /// every value taken from the account's model list. Its `provider` is
    /// the provider family whose protocol the account serves the model on.
    Offered(meerkat_core::config::CustomModelConfig),
    /// The account cannot serve the model.
    Refused(AccountModelRefusal),
}

/// Non-mutating readiness of one binding's credential, observed without
/// materializing, refreshing, or persisting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CredentialReadiness {
    /// Credential material is present and usable without a refresh or login.
    Ready,
    /// No credential material is present for the binding.
    Missing,
    /// The binding's credential lifecycle requires an interactive login.
    NeedsReauth,
    /// The source can only be observed by materializing it (an external
    /// resolver, a credential command, a host file descriptor, an
    /// operating-system keychain), so the open resolves it and reports any
    /// failure there.
    MaterializedAtOpen,
    /// The credential store reported that the user denied access. Not absence.
    AccessDenied,
    /// The credential store reported that reading needs user interaction.
    InteractionRequired,
    /// The credential store or the credential's owner could not answer now
    /// (a lookup is pending, the owner is busy, or the store failed). Not
    /// absence.
    Unavailable,
}

impl CredentialReadiness {
    /// Whether a readiness probe may report the binding as usable.
    #[must_use]
    pub const fn admits_open(self) -> bool {
        matches!(self, Self::Ready | Self::MaterializedAtOpen)
    }
}

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

    /// Observe whether this binding's credential is usable, without
    /// materializing, refreshing, or persisting anything.
    ///
    /// Readiness probes call this instead of [`Self::resolve_binding`] so a
    /// poll never refreshes a token, runs a credential command, or asks an
    /// external resolver. The default reports
    /// [`CredentialReadiness::MaterializedAtOpen`]: a runtime that does not
    /// implement the observation leaves the verdict to the open, which
    /// resolves the credential through [`Self::resolve_binding`].
    async fn observe_credential_readiness(
        &self,
        _binding: &ValidatedBinding,
        _env: &ResolverEnvironment,
    ) -> Result<CredentialReadiness, ProviderAuthError> {
        Ok(CredentialReadiness::MaterializedAtOpen)
    }

    /// What the account behind `connection` states about `model`, for a
    /// backend whose accounts state the models they serve. Asked after
    /// `connection` resolved, and only about a model the model registry does
    /// not define. The default reports [`AccountModelLookup::NotReported`]:
    /// the backend's accounts state no model list.
    fn account_model(&self, _connection: &ResolvedConnection, _model: &str) -> AccountModelLookup {
        AccountModelLookup::NotReported
    }

    /// Construct a concrete LlmClient from the resolved connection.
    /// Phase 2 shim: extracts shim_credential and hands it to the legacy
    /// provider constructors.
    fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError>;

    fn build_text_client(
        &self,
        target: ResolvedTextTarget,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        let (_, _, connection) = target.into_parts();
        self.build_client(connection)
    }

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
        _target: ResolvedRealtimeTarget,
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

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn realtime_session_factory_defaults_to_missing_feature() {
        use crate::provider_runtime::binding::StaticLease;
        use meerkat_core::{Config, ModelRegistry, SessionLlmIdentity};

        let runtime: Arc<dyn ProviderRuntime> = Arc::new(MockRuntime);
        let registry = ModelRegistry::from_config(&Config::default(), meerkat_models::canonical())
            .expect("canonical model registry");
        let entry = registry
            .entries_for_provider(Provider::OpenAI)
            .find(|entry| {
                registry
                    .profile_for_provider(Provider::OpenAI, &entry.id)
                    .is_some_and(|profile| profile.realtime)
            })
            .expect("canonical realtime model");
        let identity = SessionLlmIdentity {
            model: entry.id.clone(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let profile = registry
            .profile_witness_for_provider(Provider::OpenAI, &entry.id)
            .expect("registry witness");
        let connection = ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(
                meerkat_core::provider_matrix::OpenAiBackendKind::OpenAiApi,
            ),
            backend_profile: Arc::new(meerkat_core::BackendProfile {
                id: "b".into(),
                provider: Provider::OpenAI,
                backend_kind: "openai_api".into(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            }),
            credential_identity: meerkat_core::AuthCredentialIdentity::from_auth_binding(
                &meerkat_core::AuthBindingRef {
                    realm: meerkat_core::RealmId::parse("dev").expect("valid realm"),
                    binding: meerkat_core::BindingId::parse("openai").expect("valid binding"),
                    profile: None,
                    origin: meerkat_core::BindingOrigin::Configured,
                },
            ),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "secret".into(),
                AuthMetadata::default(),
                None,
                "test",
            )),
        };
        let target = ResolvedRealtimeTarget::new(identity, profile, connection)
            .expect("matching realtime target");
        let err = match runtime.build_realtime_session_factory(target) {
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

    /// A runtime whose backend states no account model list leaves the model
    /// registry the only definition of its models, and so does a registry
    /// with no runtime for the connection's provider.
    #[test]
    fn a_runtime_reports_no_account_models_by_default() {
        use crate::provider_runtime::binding::StaticLease;
        use crate::provider_runtime::registry::ProviderRuntimeRegistry;

        let connection = ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(
                meerkat_core::provider_matrix::OpenAiBackendKind::OpenAiApi,
            ),
            backend_profile: Arc::new(meerkat_core::BackendProfile {
                id: "b".into(),
                provider: Provider::OpenAI,
                backend_kind: "openai_api".into(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            }),
            credential_identity: meerkat_core::AuthCredentialIdentity::from_auth_binding(
                &meerkat_core::AuthBindingRef {
                    realm: meerkat_core::RealmId::parse("dev").expect("valid realm"),
                    binding: meerkat_core::BindingId::parse("openai").expect("valid binding"),
                    profile: None,
                    origin: meerkat_core::BindingOrigin::Configured,
                },
            ),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "secret".into(),
                AuthMetadata::default(),
                None,
                "test",
            )),
        };
        assert_eq!(
            MockRuntime.account_model(&connection, "account-model-1"),
            AccountModelLookup::NotReported
        );
        assert_eq!(
            ProviderRuntimeRegistry::empty().account_model(&connection, "account-model-1"),
            AccountModelLookup::NotReported
        );
    }
}
