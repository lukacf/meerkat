//! Per-open realtime credential resolution.
//!
//! Realtime sockets authenticate with the credential of the OWNING session
//! identity, resolved at open time — never a process-lifetime secret baked
//! at surface startup. [`PerOpenCredentialRealtimeSessionFactory`] is the
//! facade-owned composition seam: on every open it reads the CURRENT config
//! from a [`RealtimeCurrentConfigSource`], resolves the session's
//! `SessionLlmIdentity.auth_binding` through
//! [`AgentFactory::resolve_realtime_session_factory_for_identity`] (the same
//! owning-realm resolver path the text client uses), and delegates the open
//! to the provider-minted factory. Credential material never surfaces here:
//! the provider runtime applies its realtime backend/auth gating and
//! constructs the concrete adapter behind
//! `ProviderRuntime::build_realtime_session_factory`. Any resolution or
//! gating failure fails the open closed with a typed [`LlmError`] — there is
//! no fallback to a default credential.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_client::{FactoryError, LlmError};
use meerkat_contracts::RealtimeCapabilities;
use meerkat_core::{Config, ConfigError, ConfigStore, Provider, SessionLlmIdentity};
use meerkat_llm_core::realtime_session::{
    RealtimeExternalSessionTarget, RealtimeSession, RealtimeSessionFactory,
    RealtimeSessionOpenConfig,
};

use crate::{AgentFactory, RealmInheritance};

/// Compose the per-open resolving OpenAI realtime session factory a
/// live-capable surface wires (DEC-P6B-L9: promoted from
/// `meerkat-rpc::live_wiring` so `rkat-rpc` and the `rkat mob host` daemon
/// run the IDENTICAL composition, each over ITS OWN realm chain — §16.5).
///
/// Per-open realtime credential resolution reads the SAME live config truth
/// the session runtime composes for agent builds: the durable head store
/// (`config_store`) plus the active realm's parent chain
/// (`realm_config_source` + `realm`). The returned factory holds NO
/// credential material; resolution happens on every open against the
/// then-current config, so a `config/set` that rotates a binding's
/// credential source is visible to the very next `live/open`.
pub fn build_per_open_realtime_session_factory(
    factory: &AgentFactory,
    config_store: Arc<dyn ConfigStore>,
    realm_config_source: Arc<dyn meerkat_core::RealmConfigSource>,
    realm: meerkat_core::connection::RealmId,
) -> Arc<dyn RealtimeSessionFactory> {
    let realtime_config_source = Arc::new(StoreBackedRealtimeConfigSource::new(
        config_store,
        Some(RealmInheritance::new(realm_config_source, realm)),
    ));
    factory.build_openai_realtime_session_factory(realtime_config_source)
}

/// Source of the CURRENT config consulted on every realtime open.
///
/// Surfaces thread their live config truth (config store + realm
/// inheritance) through this seam so per-open credential resolution never
/// runs against a startup `Config` clone: a `config/set` that rotates a
/// binding's credential source is visible to the very next `live/open`.
#[async_trait]
pub trait RealtimeCurrentConfigSource: Send + Sync {
    /// Fetch the current effective config.
    async fn current_config(&self) -> Result<Config, ConfigError>;
}

/// [`RealtimeCurrentConfigSource`] backed by a surface's live
/// [`ConfigStore`], optionally composing the head realm's parent chain via
/// [`RealmInheritance`] — the same head document + chain composition the
/// surface's agent builds consume (`FactoryAgentBuilder::resolve_config`).
pub struct StoreBackedRealtimeConfigSource {
    store: Arc<dyn ConfigStore>,
    inheritance: Option<RealmInheritance>,
}

impl StoreBackedRealtimeConfigSource {
    /// Wrap a live config store and optional realm inheritance chain.
    pub fn new(store: Arc<dyn ConfigStore>, inheritance: Option<RealmInheritance>) -> Self {
        Self { store, inheritance }
    }
}

#[async_trait]
impl RealtimeCurrentConfigSource for StoreBackedRealtimeConfigSource {
    async fn current_config(&self) -> Result<Config, ConfigError> {
        let head = self.store.get().await?;
        match &self.inheritance {
            Some(inheritance) => inheritance.compose_over(head).await,
            None => Ok(head),
        }
    }
}

/// Lower a per-open resolution fault into the typed realtime open error.
///
/// Auth/credential faults surface as `AuthenticationFailed`; every other
/// resolution fault (realm/binding selection, provider gating such as an
/// Azure OpenAI binding on the realtime transport) is a channel-terminal
/// `InvalidConfig` — the caller must fix the binding and reopen, never
/// retry into a default credential.
fn realtime_open_resolution_error(error: FactoryError) -> LlmError {
    match error {
        FactoryError::ProviderAuth(error) => LlmError::AuthenticationFailed {
            message: format!("realtime credential resolution failed: {error}"),
        },
        FactoryError::TokenStore(error) => LlmError::AuthenticationFailed {
            message: format!("realtime credential store unavailable: {error}"),
        },
        other => LlmError::InvalidConfig {
            message: format!("realtime provider resolution failed: {other}"),
        },
    }
}

/// Facade-owned realtime session factory that resolves the provider
/// credential per open from the owning session identity.
///
/// Holds an [`AgentFactory`] (provider-runtime registry, token store
/// attachment, refresh coordinator, external auth resolvers — the exact
/// resolver-environment assembly the text path uses) and a
/// [`RealtimeCurrentConfigSource`]. It never extracts or stores credential
/// material: minting happens inside the owning provider runtime.
pub struct PerOpenCredentialRealtimeSessionFactory {
    factory: AgentFactory,
    config_source: Arc<dyn RealtimeCurrentConfigSource>,
}

impl PerOpenCredentialRealtimeSessionFactory {
    /// Compose the per-open resolving factory from the shared agent factory
    /// and the surface's current-config source.
    pub fn new(factory: AgentFactory, config_source: Arc<dyn RealtimeCurrentConfigSource>) -> Self {
        Self {
            factory,
            config_source,
        }
    }

    async fn resolve_provider_factory(
        &self,
        identity: &SessionLlmIdentity,
    ) -> Result<Arc<dyn RealtimeSessionFactory>, LlmError> {
        let config =
            self.config_source
                .current_config()
                .await
                .map_err(|error| LlmError::InvalidConfig {
                    message: format!(
                        "realtime credential resolution could not read the current config: {error}"
                    ),
                })?;
        self.factory
            .resolve_realtime_session_factory_for_identity(&config, identity)
            .await
            .map_err(realtime_open_resolution_error)
    }
}

#[async_trait]
impl RealtimeSessionFactory for PerOpenCredentialRealtimeSessionFactory {
    fn capabilities(&self) -> RealtimeCapabilities {
        // Identity-free advertisement is the provider-owned OpenAI answer;
        // per-open backend/auth gating happens at mint time inside
        // `OpenAiProviderRuntime::build_realtime_session_factory`.
        meerkat_openai::live::openai_realtime_capabilities_default()
    }

    fn supports_provider(&self, provider: Provider) -> bool {
        // Composition-seam fact: this facade factory wires the OpenAI
        // realtime lane (the only provider runtime that overrides
        // `build_realtime_session_factory` today). Mint-time gating still
        // owns the per-connection accept/reject decision.
        provider == Provider::OpenAI
    }

    async fn open_session(
        &self,
        open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Box<dyn RealtimeSession>, LlmError> {
        let provider_factory = self
            .resolve_provider_factory(&open_config.llm_identity)
            .await?;
        provider_factory.open_session(open_config).await
    }

    async fn attach_external_session(
        &self,
        target: &RealtimeExternalSessionTarget,
        open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Box<dyn RealtimeSession>, LlmError> {
        let provider_factory = self
            .resolve_provider_factory(&open_config.llm_identity)
            .await?;
        provider_factory
            .attach_external_session(target, open_config)
            .await
    }

    async fn open_live_adapter(
        &self,
        open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Arc<dyn meerkat_core::live_adapter::LiveAdapter>, LlmError> {
        let provider_factory = self
            .resolve_provider_factory(&open_config.llm_identity)
            .await?;
        provider_factory.open_live_adapter(open_config).await
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_contracts::RealtimeTurningMode;
    use meerkat_core::MemoryConfigStore;
    use meerkat_core::connection::{RealmConfigSection, RealmId};

    struct MapRealmSource {
        docs: std::collections::BTreeMap<String, Config>,
    }

    #[async_trait]
    impl meerkat_core::RealmConfigSource for MapRealmSource {
        async fn config_for_realm(&self, realm: &RealmId) -> Result<Option<Config>, ConfigError> {
            Ok(self.docs.get(realm.as_str()).cloned())
        }
    }

    #[tokio::test]
    async fn store_backed_source_serves_current_store_config_not_a_snapshot() {
        let store = Arc::new(MemoryConfigStore::new(
            Config::default(),
            meerkat_models::canonical(),
        ));
        let source =
            StoreBackedRealtimeConfigSource::new(Arc::clone(&store) as Arc<dyn ConfigStore>, None);

        let initial = source.current_config().await.expect("initial config");
        assert_eq!(initial.agent.model, Config::default().agent.model);

        let mut updated = Config::default();
        updated.agent.model = "gpt-5.5".to_string();
        store
            .set(updated)
            .await
            .expect("store update should persist");

        let current = source.current_config().await.expect("current config");
        assert_eq!(
            current.agent.model, "gpt-5.5",
            "per-open config source must read the live store, not a startup clone"
        );
    }

    #[tokio::test]
    async fn store_backed_source_composes_realm_inheritance_over_head() {
        let mut global = Config::default();
        global.models.openai = "g-openai".to_string();
        global
            .realm
            .insert("global".to_string(), RealmConfigSection::default());
        let mut docs = std::collections::BTreeMap::new();
        docs.insert("global".to_string(), global);

        let mut head = Config::default();
        head.realm.insert(
            "child".to_string(),
            RealmConfigSection {
                parent: Some(RealmId::global()),
                ..Default::default()
            },
        );
        let store = Arc::new(MemoryConfigStore::new(head, meerkat_models::canonical()));
        let source = StoreBackedRealtimeConfigSource::new(
            store as Arc<dyn ConfigStore>,
            Some(RealmInheritance::new(
                Arc::new(MapRealmSource { docs }),
                RealmId::parse("child").expect("valid realm"),
            )),
        );

        let composed = source.current_config().await.expect("composed config");
        assert_eq!(
            composed.models.openai, "g-openai",
            "per-open config source must compose the realm parent chain over the head document"
        );
    }

    /// T-L12 — facade twin of the `rkat-rpc` binary wiring pin: the
    /// factory [`build_per_open_realtime_session_factory`] composes must
    /// resolve realtime credentials PER OPEN against the LIVE config
    /// source, failing each open closed with the typed resolution error —
    /// never a default-credential fallback, never a startup snapshot. The
    /// member host runs this exact composition over its own realm chain.
    #[tokio::test]
    async fn facade_per_open_factory_resolves_from_live_config_and_fails_closed() {
        use meerkat_core::provider_matrix::openai::{OpenAiAuthMethod, OpenAiBackendKind};
        use meerkat_core::{
            AuthBindingRef, AuthProfileConfig, BackendProfileConfig, BindingId,
            CredentialSourceSpec, ProviderBindingConfig,
        };

        const TEST_REALM: &str = "rt-facade-credentials";
        const TEST_BINDING: &str = "openai-main";
        // Deliberately never set anywhere: the binding names it as its ONLY
        // credential source, so resolution is deterministic regardless of
        // any real `OPENAI_API_KEY` in the environment.
        const ABSENT_ENV_VAR: &str = "MEERKAT_FACADE_RT_CREDENTIALS_TEST_ABSENT_KEY";

        fn openai_env_binding_section() -> RealmConfigSection {
            let mut section = RealmConfigSection::default();
            section.backend.insert(
                "openai-backend".to_string(),
                BackendProfileConfig {
                    provider: Provider::OpenAI.as_str().to_string(),
                    backend_kind: OpenAiBackendKind::OpenAiApi.as_str().to_string(),
                    base_url: None,
                    options: serde_json::Value::Null,
                },
            );
            section.auth.insert(
                "openai-auth".to_string(),
                AuthProfileConfig {
                    provider: Provider::OpenAI.as_str().to_string(),
                    auth_method: OpenAiAuthMethod::ApiKey.as_str().to_string(),
                    source: CredentialSourceSpec::Env {
                        env: ABSENT_ENV_VAR.to_string(),
                        fallback: Vec::new(),
                    },
                    constraints: Default::default(),
                    metadata_defaults: Default::default(),
                },
            );
            section.binding.insert(
                TEST_BINDING.to_string(),
                ProviderBindingConfig {
                    backend_profile: "openai-backend".to_string(),
                    auth_profile: "openai-auth".to_string(),
                    default_model: None,
                    policy: Default::default(),
                    provider_default: false,
                },
            );
            section
        }

        let identity = SessionLlmIdentity {
            model: "gpt-realtime".to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(AuthBindingRef {
                realm: RealmId::parse(TEST_REALM).expect("valid realm slug"),
                binding: BindingId::parse(TEST_BINDING).expect("valid binding id"),
                profile: None,
                origin: Default::default(),
            }),
        };
        let open_config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ProviderManaged,
            identity,
            Vec::new(),
            Vec::new(),
        );

        let temp = tempfile::TempDir::new().expect("tempdir");
        let factory = AgentFactory::new(temp.path().join("sessions")).without_token_store();
        let store = Arc::new(MemoryConfigStore::new(
            Config::default(),
            meerkat_models::canonical(),
        ));
        let realm_source = Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
            temp.path().join("state"),
            temp.path().join("state").join("global-config.toml"),
            meerkat_models::canonical(),
        ));
        let wired = build_per_open_realtime_session_factory(
            &factory,
            Arc::clone(&store) as Arc<dyn ConfigStore>,
            realm_source,
            RealmId::parse(TEST_REALM).expect("valid realm slug"),
        );

        // Phase 1 — fail closed: the identity's auth binding does not exist
        // in the current config.
        let err = wired
            .open_session(&open_config)
            .await
            .err()
            .expect("live open without a resolvable binding must fail closed");
        assert!(
            matches!(err, LlmError::InvalidConfig { .. }),
            "unknown realm/binding must surface as the typed InvalidConfig \
             per-open resolution failure, got: {err:?}"
        );

        // Phase 2 — live-config pin: mutate the SAME store (no rebuild);
        // the next open reads the change, advances into credential
        // resolution, and fails closed on the absent env var.
        let mut config = Config::default();
        config
            .realm
            .insert(TEST_REALM.to_string(), openai_env_binding_section());
        store.set(config).await.expect("config store update");
        let err = wired
            .open_session(&open_config)
            .await
            .err()
            .expect("live open without resolvable credential material must fail closed");
        assert!(
            matches!(err, LlmError::AuthenticationFailed { .. }),
            "missing credential material must surface as the typed \
             AuthenticationFailed per-open resolution failure, got: {err:?}"
        );
    }
}
