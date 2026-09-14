use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

use meerkat_core::live_execution::profile::{
    LiveClientRequestPolicy, LiveManagedBackendModel, LiveProfileId,
};
use meerkat_core::model_profile::ModelInteractionKind;
use meerkat_core::model_profile::capabilities::ModelCapabilities;
use meerkat_core::provider_matrix::openai::OpenAiBackendKind;
use meerkat_core::{
    AuthBindingRef, AuthCredentialIdentity, AuthProfile, BackendProfile, Config,
    CredentialSourceSpec, ModelProfileWitness, ModelRegistry, Provider, ProviderBinding,
    RealmConnectionSet, SessionLlmIdentity,
};
use meerkat_llm_core::provider_runtime::errors::{ProviderAuthError, ProviderClientError};
use meerkat_llm_core::provider_runtime::{
    LiveConnectionResolutionError, LiveTargetError, ProviderRuntime, ProviderRuntimeRegistry,
    ResolvedConnection, ResolvedLiveConnection, ResolvedLiveExecution, ResolvedLiveTarget,
    ResolverEnvironment, StaticLease, ValidatedBinding,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const VOICE: &str = "gpt-live-1-codex";

#[test]
fn open_intent_matrix_keeps_public_profile_separate_from_legacy_defaults() -> TestResult {
    use meerkat_contracts::{
        RealtimeTurningMode as Mode, WireLiveExecutionIdentityOverrideV1,
        WireLiveExecutionIdentityVersion,
    };
    use meerkat_llm_core::provider_runtime::live::open::{
        LiveOpenIntent, LiveOpenIntentError as Error,
    };
    let public = LiveProfileId::parse("voice-public")?;
    let private = WireLiveExecutionIdentityOverrideV1 {
        version: WireLiveExecutionIdentityVersion::V1,
        profile_id: "separate-private-profile".into(),
    };
    for mode in [None, Some(Mode::Continuous)] {
        assert_eq!(
            LiveOpenIntent::select(Some(&public), None, mode, None)?,
            LiveOpenIntent::Continuous {
                profile_id: &public
            }
        );
        for seed in [0, 1, usize::MAX] {
            assert_eq!(
                LiveOpenIntent::select(Some(&public), None, mode, Some(seed)),
                Err(Error::UnsupportedSeedWindowForContinuous)
            );
        }
    }
    for mode in [Mode::ProviderManaged, Mode::ExplicitCommit] {
        assert_eq!(
            LiveOpenIntent::select(Some(&public), None, Some(mode), None),
            Err(Error::UnsupportedTurningMode)
        );
    }
    for mode in [
        None,
        Some(Mode::ProviderManaged),
        Some(Mode::ExplicitCommit),
        Some(Mode::Continuous),
    ] {
        for seed in [None, Some(0), Some(1024)] {
            assert_eq!(
                LiveOpenIntent::select(Some(&public), Some(&private), mode, seed),
                Err(Error::MutuallyExclusiveSelection)
            );
        }
    }
    for legacy in [None, Some(&private)] {
        for mode in [
            None,
            Some(Mode::ProviderManaged),
            Some(Mode::ExplicitCommit),
        ] {
            for seed in [None, Some(1), Some(usize::MAX)] {
                assert_eq!(
                    LiveOpenIntent::select(None, legacy, mode, seed)?,
                    LiveOpenIntent::Realtime {
                        execution_identity: legacy,
                        turning_mode: mode.unwrap_or(Mode::ProviderManaged),
                        seed_max_chars: seed.and_then(std::num::NonZeroUsize::new),
                    }
                );
            }
            assert_eq!(
                LiveOpenIntent::select(None, legacy, mode, Some(0)),
                Err(Error::ZeroSeedWindow)
            );
        }
        assert_eq!(
            LiveOpenIntent::select(None, legacy, Some(Mode::Continuous), None),
            Err(Error::UnsupportedTurningMode)
        );
    }
    Ok(())
}

fn registry() -> Result<ModelRegistry, meerkat_core::ConfigError> {
    static CAPABILITIES: LazyLock<Vec<ModelCapabilities>> = LazyLock::new(|| {
        let mut rows = meerkat_models::canonical().capabilities.to_vec();
        for row in &mut rows {
            if row.id == VOICE {
                row.interaction_kind = ModelInteractionKind::ContinuousLive;
            }
        }
        rows
    });
    let mut catalog = meerkat_models::canonical();
    catalog.capabilities = &CAPABILITIES;
    ModelRegistry::from_config(&Config::default(), catalog)
}

fn profile(
    model: &str,
    provider: Provider,
) -> Result<ModelProfileWitness, Box<dyn std::error::Error>> {
    registry()?
        .profile_witness_for_provider(provider, model)
        .ok_or_else(|| "missing fixture profile".into())
}

fn binding() -> Result<AuthBindingRef, serde_json::Error> {
    serde_json::from_value(json!({"realm": "voice-owner", "binding": "voice-key"}))
}

fn identity(model: &str) -> Result<SessionLlmIdentity, serde_json::Error> {
    Ok(SessionLlmIdentity {
        model: model.to_owned(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(binding()?),
    })
}

fn realm(account: Option<&str>) -> Result<RealmConnectionSet, Box<dyn std::error::Error>> {
    Ok(RealmConnectionSet {
        realm_id: binding()?.realm,
        backends: BTreeMap::from([(
            "voice-backend".into(),
            BackendProfile {
                id: "voice-backend".into(),
                provider: Provider::OpenAI,
                backend_kind: OpenAiBackendKind::OpenAiApi.as_str().into(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            },
        )]),
        auth_profiles: BTreeMap::from([(
            "voice-auth".into(),
            AuthProfile {
                id: "voice-auth".into(),
                provider: Provider::OpenAI,
                auth_method: "api_key".into(),
                source: CredentialSourceSpec::InlineSecret {
                    secret: "fixture-not-a-credential".into(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        )]),
        bindings: BTreeMap::from([(
            "voice-key".into(),
            ProviderBinding {
                id: "voice-key".into(),
                backend_profile: "voice-backend".into(),
                auth_profile: "voice-auth".into(),
                credential_account: account
                    .map(meerkat_core::connection::CredentialAccountId::parse)
                    .transpose()?,
                default_model: None,
                policy: Default::default(),
                provider_default: false,
            },
        )]),
        default_binding: None,
    })
}

struct FixtureRuntime {
    returned_identity: Option<AuthCredentialIdentity>,
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl ProviderRuntime for FixtureRuntime {
    fn provider_id(&self) -> Provider {
        Provider::OpenAI
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        _: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        Ok(ResolvedConnection {
            provider: binding.provider(),
            backend: binding.backend(),
            backend_profile: binding.backend_profile().clone(),
            credential_identity: self
                .returned_identity
                .clone()
                .unwrap_or_else(|| binding.credential_identity().clone()),
            auth_lease: Arc::new(StaticLease::empty_lease(Default::default(), "fixture")),
        })
    }

    fn build_client(
        &self,
        _: ResolvedConnection,
    ) -> Result<Arc<dyn meerkat_llm_core::LlmClient>, ProviderClientError> {
        Err(ProviderClientError::MissingFeature(
            "contract-fixture-does-not-build-clients",
        ))
    }
}

fn resolve_connection(
    realm: &RealmConnectionSet,
    binding: &AuthBindingRef,
    returned_identity: Option<AuthCredentialIdentity>,
) -> Result<ResolvedLiveConnection, LiveConnectionResolutionError> {
    futures::executor::block_on(
        ProviderRuntimeRegistry::empty()
            .with_runtime(Arc::new(FixtureRuntime { returned_identity }))
            .resolve_live_connection(realm, binding, &ResolverEnvironment::testing()),
    )
}

fn connection() -> Result<ResolvedLiveConnection, Box<dyn std::error::Error>> {
    Ok(resolve_connection(&realm(None)?, &binding()?, None)?)
}

fn client_mode() -> ResolvedLiveExecution {
    ResolvedLiveExecution::ClientContext {
        request_policy: LiveClientRequestPolicy::SnapshotAtDelegation,
    }
}

#[test]
fn live_target_requires_exact_continuous_registry_identity() -> TestResult {
    for (model, expected) in [
        ("gpt-5.5", LiveTargetError::VoiceNotContinuous),
        ("gpt-realtime-2", LiveTargetError::VoiceNotContinuous),
    ] {
        let result = ResolvedLiveTarget::new(
            LiveProfileId::parse("voice")?,
            identity(model)?,
            profile(model, Provider::OpenAI)?,
            connection()?,
            client_mode(),
        );
        assert_eq!(result.err(), Some(expected));
    }
    let result = ResolvedLiveTarget::new(
        LiveProfileId::parse("voice")?,
        identity("not-the-witness")?,
        profile(VOICE, Provider::OpenAI)?,
        connection()?,
        client_mode(),
    );
    assert_eq!(result.err(), Some(LiveTargetError::VoiceIdentityMismatch));
    Ok(())
}

#[test]
fn live_target_cannot_drop_or_change_the_owning_binding() -> TestResult {
    for field in ["realm", "binding", "profile"] {
        let mut altered = serde_json::to_value(binding()?)?;
        altered[field] = json!("different");
        let connection = connection()?;
        let mut identity = identity(VOICE)?;
        identity.auth_binding = Some(serde_json::from_value::<AuthBindingRef>(altered)?);
        let result = ResolvedLiveTarget::new(
            LiveProfileId::parse("voice")?,
            identity,
            profile(VOICE, Provider::OpenAI)?,
            connection,
            client_mode(),
        );
        assert_eq!(result.err(), Some(LiveTargetError::CredentialOwnerMismatch));
    }
    let mut identity = identity(VOICE)?;
    identity.auth_binding = None;
    assert_eq!(
        ResolvedLiveTarget::new(
            LiveProfileId::parse("voice")?,
            identity,
            profile(VOICE, Provider::OpenAI)?,
            connection()?,
            client_mode(),
        )
        .err(),
        Some(LiveTargetError::MissingResolvedBinding)
    );
    Ok(())
}

#[test]
fn managed_backend_shares_voice_connection_without_becoming_executor() -> TestResult {
    let connection = connection()?;
    let lease = Arc::clone(&connection.connection().auth_lease);
    let target = ResolvedLiveTarget::new(
        LiveProfileId::parse("voice")?,
        identity(VOICE)?,
        profile(VOICE, Provider::OpenAI)?,
        connection,
        ResolvedLiveExecution::function_bridge(
            &LiveManagedBackendModel {
                provider: Provider::OpenAI,
                model: "gpt-5.5".into(),
            },
            profile("gpt-5.5", Provider::OpenAI)?,
        )?,
    )?;
    let backend = target.managed_backend_target()?.ok_or("missing backend")?;
    assert_eq!(target.voice_identity().model, VOICE);
    assert_eq!(backend.identity().model, "gpt-5.5");
    assert_eq!(
        backend.identity().auth_binding,
        target.voice_identity().auth_binding
    );
    assert!(Arc::ptr_eq(&lease, &backend.connection().auth_lease));
    assert!(backend.identity().provider_params.is_none());
    assert!(!format!("{target:?}").contains("voice-key"));
    Ok(())
}

#[test]
fn binding_and_shared_account_credentials_keep_exact_registry_provenance() -> TestResult {
    for account in [None, Some("shared-voice-account")] {
        let realm = realm(account)?;
        let connection = resolve_connection(&realm, &binding()?, None)?;
        let expected = realm.bindings["voice-key"].credential_identity(&binding()?);
        let target = ResolvedLiveTarget::new(
            LiveProfileId::parse("voice")?,
            identity(VOICE)?,
            profile(VOICE, Provider::OpenAI)?,
            connection,
            client_mode(),
        )?;
        assert_eq!(target.connection().credential_identity, expected);
    }
    Ok(())
}

#[test]
fn registry_refuses_cross_owner_account_and_binding_substitution() -> TestResult {
    let realm = realm(Some("shared-voice-account"))?;
    for wrong in [
        json!({"realm": "other-owner", "account": "shared-voice-account"}),
        json!({"realm": "voice-owner", "account": "other-account"}),
        json!({"realm": "voice-owner", "binding": "voice-key"}),
    ] {
        let wrong: AuthCredentialIdentity = serde_json::from_value(wrong)?;
        assert!(matches!(
            resolve_connection(&realm, &binding()?, Some(wrong)),
            Err(LiveConnectionResolutionError::CredentialIdentityMismatch)
        ));
    }
    let wrong_binding: AuthBindingRef =
        serde_json::from_value(json!({"realm": "other-owner", "binding": "voice-key"}))?;
    assert!(resolve_connection(&realm, &wrong_binding, None).is_err());
    let realm = self::realm(None)?;
    let wrong: AuthCredentialIdentity =
        serde_json::from_value(json!({"realm": "voice-owner", "binding": "another-binding"}))?;
    assert!(matches!(
        resolve_connection(&realm, &binding()?, Some(wrong)),
        Err(LiveConnectionResolutionError::CredentialIdentityMismatch)
    ));
    Ok(())
}

#[test]
fn managed_backend_rejects_wrong_identity_protocol_and_provider() -> TestResult {
    let declaration = LiveManagedBackendModel {
        provider: Provider::OpenAI,
        model: "gpt-5.5".into(),
    };
    assert_eq!(
        ResolvedLiveExecution::function_bridge(
            &declaration,
            profile("gpt-realtime-2", Provider::OpenAI)?
        )
        .err(),
        Some(LiveTargetError::BackendIdentityMismatch)
    );
    for (model, provider, expected) in [
        (VOICE, Provider::OpenAI, LiveTargetError::BackendNotText),
        (
            "claude-opus-5",
            Provider::Anthropic,
            LiveTargetError::BackendProviderMismatch,
        ),
    ] {
        let result = ResolvedLiveTarget::new(
            LiveProfileId::parse("voice")?,
            identity(VOICE)?,
            profile(VOICE, Provider::OpenAI)?,
            connection()?,
            ResolvedLiveExecution::FunctionBridge {
                backend: profile(model, provider)?,
            },
        );
        assert_eq!(result.err(), Some(expected));
    }
    Ok(())
}
