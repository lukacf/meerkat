//! Per-open realtime session-factory wiring for the `rkat-rpc` binary.
//!
//! `main.rs` stays a thin caller: the composition of the live config truth
//! (durable head store + active realm parent chain) with the facade-owned
//! per-open resolving factory lives here so the exact wiring the BINARY
//! ships is testable. Every `live/open` re-resolves the owning session's
//! auth binding against the CURRENT config and fails closed with a typed
//! error — there is no fallback to a process-default credential.

use std::sync::Arc;

use meerkat::AgentFactory;
use meerkat_core::{ConfigStore, RealmConfigSource, RealmId};

/// Compose the per-open resolving OpenAI realtime session factory that
/// `rkat-rpc` wires when a live transport is enabled.
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
    realm_config_source: Arc<dyn RealmConfigSource>,
    realm: RealmId,
) -> Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory> {
    let realtime_config_source = Arc::new(
        meerkat::session_runtime::realtime_credentials::StoreBackedRealtimeConfigSource::new(
            config_store,
            Some(meerkat::RealmInheritance::new(realm_config_source, realm)),
        ),
    );
    factory.build_openai_realtime_session_factory(realtime_config_source)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use meerkat_client::LlmError;
    use meerkat_client::realtime_session::{RealtimeSessionFactory, RealtimeSessionOpenConfig};
    use meerkat_contracts::RealtimeTurningMode;
    use meerkat_core::provider_matrix::openai::{OpenAiAuthMethod, OpenAiBackendKind};
    use meerkat_core::{
        AuthBindingRef, AuthProfileConfig, BackendProfileConfig, BindingId, Config,
        CredentialSourceSpec, MemoryConfigStore, Provider, ProviderBindingConfig,
        RealmConfigSection, SessionLlmIdentity,
    };

    const TEST_REALM: &str = "rt-live-wiring";
    const TEST_BINDING: &str = "openai-main";
    /// Unique env var deliberately never set anywhere. The binding below
    /// names it as its ONLY credential source, so per-open resolution is
    /// deterministic regardless of any real `OPENAI_API_KEY` in the
    /// developer/CI environment — and the failure assertion proves the
    /// resolution never falls back to a process-default credential.
    const ABSENT_ENV_VAR: &str = "MEERKAT_RPC_LIVE_WIRING_TEST_ABSENT_KEY";

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

    fn config_with_openai_binding() -> Config {
        let mut config = Config::default();
        config
            .realm
            .insert(TEST_REALM.to_string(), openai_env_binding_section());
        config
    }

    fn openai_identity() -> SessionLlmIdentity {
        SessionLlmIdentity {
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
        }
    }

    fn open_config() -> RealtimeSessionOpenConfig {
        RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ProviderManaged,
            openai_identity(),
            Vec::new(),
            Vec::new(),
        )
    }

    /// The credential chain's binary link: the factory `rkat-rpc` wires must
    /// resolve realtime credentials PER OPEN against the LIVE config source,
    /// failing each open closed with the typed resolution error — never a
    /// default-credential fallback, never a startup snapshot.
    ///
    /// The positive far end (a resolvable binding minting the provider
    /// factory and erroring only at the socket) is deliberately not asserted
    /// here: it would dial the real realtime endpoint, which is not
    /// deterministic offline. Getting past binding selection into credential
    /// resolution (phase 2 below) is the deterministic-offline maximum.
    #[tokio::test]
    async fn binary_wired_factory_resolves_per_open_from_live_config_and_fails_closed() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let factory =
            AgentFactory::new(temp.path().join("sessions")).without_provider_auth_persistence();
        let store = Arc::new(MemoryConfigStore::new(
            Config::default(),
            meerkat_models::canonical(),
        ));
        // Production realm-chain source over an empty state root: every
        // ancestor doc (including `global`) is genuinely absent.
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

        // Phase 1 — fail closed: the session identity's auth binding does
        // not exist in the current config (no OpenAI credentials anywhere),
        // so the open must fail with the typed resolution error instead of
        // falling back to any default credential.
        let err = wired
            .open_session(&open_config())
            .await
            .err()
            .expect("live/open without a resolvable binding must fail closed");
        assert!(
            matches!(err, LlmError::InvalidConfig { .. }),
            "unknown realm/binding must surface as the typed InvalidConfig \
             per-open resolution failure, got: {err:?}"
        );

        // Phase 2 — live-config pin: mutate the SAME store the wired factory
        // holds (no rebuild) and observe the next open reading the change:
        // binding selection now succeeds and the open advances into
        // credential resolution, which fails closed on the deliberately
        // absent env var — proving both that the factory reads the live
        // config source per open and that a missing credential never falls
        // back to a process-default key.
        store
            .set(config_with_openai_binding())
            .await
            .expect("config store update");
        let err = wired
            .open_session(&open_config())
            .await
            .err()
            .expect("live/open without resolvable credential material must fail closed");
        assert!(
            matches!(err, LlmError::AuthenticationFailed { .. }),
            "missing credential material must surface as the typed \
             AuthenticationFailed per-open resolution failure, got: {err:?}"
        );
    }
}
