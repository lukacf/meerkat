//! ProviderRuntimeRegistry — maps Provider → Arc<dyn ProviderRuntime>.
//!
//! Also houses `ResolverEnvironment`, the explicit env-injection seam that
//! replaces direct `std::env::var` calls in the new resolution stack.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use meerkat_core::{
    AuthError, Provider, RealmConnectionSet, ResolvedAuthEnvelope, connection::ConnectionRef,
    handles::AuthLeaseHandle,
};

use crate::provider_runtime::binding::{ResolvedConnection, ValidatedBinding};
use crate::provider_runtime::errors::{
    ProviderAuthError, ProviderBindingError, ProviderClientError,
};
use crate::provider_runtime::runtime::ProviderRuntime;
use crate::{ImageGenerationExecutor, LlmClient};

// Provider runtimes live in per-provider crates. Callers (typically the
// `meerkat` facade's `default_provider_registry()`) register them via
// `with_runtime()` after constructing an empty registry.

/// Closure that looks up an environment variable by name. Injected into
/// [`ResolverEnvironment`] so the new resolution stack never calls
/// `std::env::var` directly.
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Closure that returns the current time. Injected for test determinism.
pub type NowFn = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Explicit environment passed to provider resolvers.
///
/// `env_lookup` replaces direct `std::env::var` calls — the resolution
/// stack reads env only through this closure. Tests seed a closure that
/// returns programmed values; production wiring uses
/// [`ResolverEnvironment::with_process_env`].
///
/// `token_store` + `refresh_coord` are consulted by OAuth-backed auth
/// methods (Claude.ai, ChatGPT, Google OAuth-personal) to find a
/// persisted access/refresh token. Absent store → OAuth paths return
/// `AuthError::InteractiveLoginRequired`.
pub struct ResolverEnvironment {
    pub env_lookup: EnvLookup,
    pub external_resolvers: BTreeMap<String, Arc<dyn ExternalAuthResolverHandle>>,
    pub now: NowFn,
    pub auth_lease_handle: Option<Arc<dyn AuthLeaseHandle>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub token_store: Option<Arc<dyn meerkat_core::auth::TokenStore>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub refresh_coord: Option<Arc<dyn meerkat_core::auth::RefreshCoordinator>>,
}

impl ResolverEnvironment {
    /// Testing default — env lookup always returns None, no external
    /// resolvers, `now` returns the current UTC time.
    pub fn testing() -> Self {
        Self {
            env_lookup: Arc::new(|_| None),
            external_resolvers: BTreeMap::new(),
            now: Arc::new(Utc::now),
            auth_lease_handle: None,
            #[cfg(not(target_arch = "wasm32"))]
            token_store: None,
            #[cfg(not(target_arch = "wasm32"))]
            refresh_coord: None,
        }
    }

    /// Wraps `std::env::var` in a closure so the new stack never reads env
    /// directly.
    pub fn with_process_env() -> Self {
        Self {
            env_lookup: Arc::new(|key| std::env::var(key).ok()),
            external_resolvers: BTreeMap::new(),
            now: Arc::new(Utc::now),
            auth_lease_handle: None,
            #[cfg(not(target_arch = "wasm32"))]
            token_store: None,
            #[cfg(not(target_arch = "wasm32"))]
            refresh_coord: None,
        }
    }

    /// Attach a token store for OAuth-backed auth methods.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_token_store(mut self, store: Arc<dyn meerkat_core::auth::TokenStore>) -> Self {
        self.token_store = Some(store);
        self
    }

    /// Attach a refresh coordinator. If `with_token_store` is set but no
    /// coordinator is, callers fall back to `InMemoryCoordinator::new()`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_refresh_coordinator(
        mut self,
        coord: Arc<dyn meerkat_core::auth::RefreshCoordinator>,
    ) -> Self {
        self.refresh_coord = Some(coord);
        self
    }

    /// Register an external auth resolver under a handle name.
    pub fn with_external_resolver(
        mut self,
        handle: impl Into<String>,
        resolver: Arc<dyn ExternalAuthResolverHandle>,
    ) -> Self {
        self.external_resolvers.insert(handle.into(), resolver);
        self
    }

    /// Attach the session-owned auth lease handle so dynamic cloud
    /// authorizers can publish observed token freshness into AuthMachine
    /// truth after a lazy token exchange.
    pub fn with_auth_lease_handle(mut self, handle: Arc<dyn AuthLeaseHandle>) -> Self {
        self.auth_lease_handle = Some(handle);
        self
    }

    /// Seed a custom env lookup closure (for tests).
    pub fn with_env_lookup<F>(mut self, lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        self.env_lookup = Arc::new(lookup);
        self
    }
}

impl Default for ResolverEnvironment {
    fn default() -> Self {
        Self::testing()
    }
}

/// Minimal Phase-2 external-auth handle. Just `resolve` — refresh/status
/// surfaces land later.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait ExternalAuthResolverHandle: Send + Sync {
    async fn resolve(&self, binding: &ValidatedBinding) -> Result<ResolvedAuthEnvelope, AuthError>;
}

/// Registry mapping Provider → runtime implementation.
pub struct ProviderRuntimeRegistry {
    runtimes: BTreeMap<Provider, Arc<dyn ProviderRuntime>>,
}

impl ProviderRuntimeRegistry {
    /// Empty registry — no runtimes registered.
    pub fn empty() -> Self {
        Self {
            runtimes: BTreeMap::new(),
        }
    }

    /// Install a custom runtime, replacing any previously registered
    /// runtime for that provider.
    pub fn with_runtime(mut self, runtime: Arc<dyn ProviderRuntime>) -> Self {
        self.runtimes.insert(runtime.provider_id(), runtime);
        self
    }

    pub fn get(&self, provider: Provider) -> Option<&Arc<dyn ProviderRuntime>> {
        self.runtimes.get(&provider)
    }

    /// Resolve a binding from a realm connection set through the matching
    /// provider runtime. Dispatches to `validate_binding` then
    /// `resolve_binding` on the runtime registered for the backend's
    /// provider.
    pub async fn resolve(
        &self,
        realm: &RealmConnectionSet,
        connection_ref: &ConnectionRef,
        env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        let (binding, backend, auth) = realm
            .lookup_connection_ref(connection_ref)
            .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?;
        if connection_ref.realm.as_str() != realm.realm_id {
            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                "connection_ref realm '{}' does not match resolved realm '{}'",
                connection_ref.realm, realm.realm_id
            )));
        }
        let runtime = self
            .runtimes
            .get(&backend.provider)
            .ok_or(ProviderAuthError::NoRuntimeRegistered(backend.provider))?;
        let validated = runtime
            .validate_binding(connection_ref, backend, auth, &binding.policy)
            .map_err(ProviderAuthError::Binding)?;
        runtime.resolve_binding(&validated, env).await
    }

    /// Build a client from a resolved connection through the matching
    /// provider runtime.
    pub fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        let runtime =
            self.runtimes
                .get(&connection.provider)
                .ok_or(ProviderClientError::MissingFeature(
                    "runtime-not-registered",
                ))?;
        runtime.build_client(connection)
    }

    /// Build the optional image-generation executor owned by the same
    /// provider runtime and resolved connection.
    pub fn build_image_generation_executor(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Option<Arc<dyn ImageGenerationExecutor>>, ProviderClientError> {
        let runtime =
            self.runtimes
                .get(&connection.provider)
                .ok_or(ProviderClientError::MissingFeature(
                    "runtime-not-registered",
                ))?;
        runtime.build_image_generation_executor(connection)
    }

    pub fn image_generation_profiles(
        &self,
    ) -> Vec<Arc<dyn meerkat_core::ImageGenerationProviderProfile>> {
        self.runtimes
            .values()
            .filter_map(|runtime| runtime.image_generation_profile())
            .collect()
    }
}

/// Also expose `ProviderBindingError` so callers can map validate failures
/// alongside the other registry errors.
pub use crate::provider_runtime::errors::ProviderBindingError as RegistryBindingError;

// Top-level resolve expects certain error variants — ensure they compile.
#[allow(dead_code)]
fn _compile_proof_of_error_wiring() -> ProviderBindingError {
    ProviderBindingError::UnsupportedCombination {
        backend: "x".into(),
        auth: "y".into(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn testing_env_has_none_lookup() {
        let env = ResolverEnvironment::testing();
        assert!((env.env_lookup)("ANYTHING").is_none());
    }

    #[test]
    fn custom_env_lookup() {
        let env = ResolverEnvironment::testing().with_env_lookup(|k| {
            if k == "OPENAI_API_KEY" {
                Some("sk-fake".into())
            } else {
                None
            }
        });
        assert_eq!(
            (env.env_lookup)("OPENAI_API_KEY").as_deref(),
            Some("sk-fake")
        );
        assert!((env.env_lookup)("OTHER").is_none());
    }
}
