use meerkat_core::live_execution::LiveExecutionMode;
use meerkat_core::live_execution::profile::{
    LiveClientRequestPolicy, LiveContextProjectionPolicy, LiveManagedBackendModel,
    LiveProfileDefinition, LiveProfileEntry, LiveProfileExecution, LiveProfileId,
};
use meerkat_core::{Provider, SessionLlmIdentity};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn definition(execution: LiveProfileExecution) -> LiveProfileDefinition {
    LiveProfileDefinition {
        voice_identity: SessionLlmIdentity {
            provider: Provider::Other,
            model: "voice-fixture".into(),
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        execution,
        voice: None,
        instructions: Some("private-voice-instructions".into()),
        context_projection: LiveContextProjectionPolicy::RecentAuthorizedSuffix,
    }
}

fn compose_profiles(
    mut parent: meerkat_core::Config,
    mut child: meerkat_core::Config,
) -> Result<meerkat_core::Config, Box<dyn std::error::Error>> {
    let parent_id: meerkat_core::RealmId = serde_json::from_value(json!("parent"))?;
    let child_id: meerkat_core::RealmId = serde_json::from_value(json!("child"))?;
    parent.realm.insert("parent".into(), Default::default());
    child.realm.insert(
        "child".into(),
        serde_json::from_value(json!({"parent": "parent"}))?,
    );
    Ok(meerkat_core::config::compose_effective_config(
        &std::collections::BTreeMap::from([(parent_id, parent), (child_id.clone(), child)]),
        &std::collections::BTreeMap::new(),
        &child_id,
    )?)
}

#[test]
fn profile_entry_distinguishes_disabled_from_configured() -> TestResult {
    let disabled = serde_json::to_value(LiveProfileEntry::Disabled)?;
    assert_eq!(disabled, json!({"state": "disabled"}));
    assert!(
        serde_json::from_value::<LiveProfileEntry>(
            json!({"state": "disabled", "permission": "must-not-disappear"})
        )
        .is_err()
    );
    let configured =
        LiveProfileEntry::Configured(Box::new(definition(LiveProfileExecution::ClientContext {
            request_policy: LiveClientRequestPolicy::SnapshotAtDelegation,
        })));
    let encoded = serde_json::to_value(&configured)?;
    assert_eq!(encoded["state"], "configured");
    assert!(encoded.get("grant").is_none());
    assert!(encoded.get("permission").is_none());
    assert_eq!(
        serde_json::from_value::<LiveProfileEntry>(encoded)?,
        configured
    );
    Ok(())
}

#[test]
fn execution_modes_require_their_distinct_inputs() -> TestResult {
    let client = LiveProfileExecution::ClientContext {
        request_policy: LiveClientRequestPolicy::SnapshotAtDelegation,
    };
    let bridge = LiveProfileExecution::FunctionBridge {
        backend: LiveManagedBackendModel {
            provider: Provider::Other,
            model: "backend-fixture".into(),
        },
    };
    assert_eq!(client.mode(), LiveExecutionMode::ClientContext);
    assert_eq!(bridge.mode(), LiveExecutionMode::FunctionBridge);
    for value in [client, bridge] {
        assert_eq!(
            serde_json::from_value::<LiveProfileExecution>(serde_json::to_value(&value)?)?,
            value
        );
    }
    for invalid in [
        json!({"mode": "client_context"}),
        json!({"mode": "function_bridge"}),
        json!({"mode": "client_context", "request_policy": "confirmed_final_transcript"}),
    ] {
        assert!(serde_json::from_value::<LiveProfileExecution>(invalid).is_err());
    }
    Ok(())
}

#[test]
fn profile_separates_voice_and_backend_without_selecting_executor_or_backend_auth() -> TestResult {
    let profile = definition(LiveProfileExecution::FunctionBridge {
        backend: LiveManagedBackendModel {
            provider: Provider::Other,
            model: "backend-fixture".into(),
        },
    });
    let mut encoded = serde_json::to_value(profile)?;
    assert_eq!(encoded["voice_identity"]["model"], "voice-fixture");
    assert_eq!(encoded["execution"]["backend"]["model"], "backend-fixture");
    assert!(encoded.get("executor").is_none());
    encoded["execution"]["backend"]["auth_binding"] = json!({"realm": "other", "binding": "key"});
    assert!(serde_json::from_value::<LiveProfileDefinition>(encoded).is_err());
    Ok(())
}

#[test]
fn profile_identity_is_validated_and_stable() -> TestResult {
    let profile = LiveProfileId::parse("voice.client-context.v1")?;
    assert_eq!(profile.as_str(), "voice.client-context.v1");
    assert_eq!(
        serde_json::from_value::<LiveProfileId>(serde_json::to_value(&profile)?)?,
        profile
    );
    for value in ["", "has space", "../path", "colon:name"] {
        assert!(LiveProfileId::parse(value).is_err());
        assert!(serde_json::from_value::<LiveProfileId>(json!(value)).is_err());
    }
    assert!(LiveProfileId::parse("a".repeat(129)).is_err());
    Ok(())
}

#[test]
fn profile_debug_redacts_voice_guidance() {
    let profile = definition(LiveProfileExecution::ClientContext {
        request_policy: LiveClientRequestPolicy::ExplicitApplicationRequest,
    });
    assert!(!format!("{profile:?}").contains("private-voice-instructions"));
}

#[test]
fn config_profiles_inherit_by_id_and_explicit_disable_wins() -> TestResult {
    let profile_id = LiveProfileId::parse("voice")?;
    let other_id = LiveProfileId::parse("other")?;
    let configured =
        LiveProfileEntry::Configured(Box::new(definition(LiveProfileExecution::ClientContext {
            request_policy: LiveClientRequestPolicy::SnapshotAtDelegation,
        })));
    let mut parent = meerkat_core::Config::default();
    parent
        .live
        .profiles
        .insert(profile_id.clone(), configured.clone());
    parent
        .live
        .profiles
        .insert(other_id.clone(), configured.clone());
    let inherited = compose_profiles(parent.clone(), meerkat_core::Config::default())?;
    assert_eq!(inherited.live.profiles[&profile_id], configured);
    let mut child = meerkat_core::Config::default();
    child
        .live
        .profiles
        .insert(profile_id.clone(), LiveProfileEntry::Disabled);
    let effective = compose_profiles(parent, child)?;
    assert_eq!(
        effective.live.profiles[&profile_id],
        LiveProfileEntry::Disabled
    );
    assert_eq!(effective.live.profiles[&other_id], configured);
    Ok(())
}

#[test]
fn child_profile_replaces_whole_definition_without_inheriting_private_guidance() -> TestResult {
    let profile_id = LiveProfileId::parse("voice")?;
    let mut parent = meerkat_core::Config::default();
    parent.live.profiles.insert(
        profile_id.clone(),
        LiveProfileEntry::Configured(Box::new(definition(LiveProfileExecution::ClientContext {
            request_policy: LiveClientRequestPolicy::SnapshotAtDelegation,
        }))),
    );
    let mut replacement = definition(LiveProfileExecution::ClientContext {
        request_policy: LiveClientRequestPolicy::ExplicitApplicationRequest,
    });
    replacement.instructions = None;
    let expected = LiveProfileEntry::Configured(Box::new(replacement));
    let mut child = meerkat_core::Config::default();
    child
        .live
        .profiles
        .insert(profile_id.clone(), expected.clone());
    let effective = compose_profiles(parent, child)?;
    assert_eq!(effective.live.profiles[&profile_id], expected);
    Ok(())
}

#[test]
fn ordinary_live_config_cannot_carry_activations_or_grants() -> TestResult {
    let default = meerkat_core::Config::default();
    assert!(default.live.is_empty());
    assert!(serde_json::to_value(default)?.get("live").is_none());
    for field in ["activations", "grants", "permission"] {
        let mut value = json!({"live": {}});
        value["live"][field] = json!({});
        assert!(serde_json::from_value::<meerkat_core::Config>(value).is_err());
    }
    let parsed: meerkat_core::Config = toml::from_str(
        r#"
        [live.profiles.voice]
        state = "disabled"
        "#,
    )?;
    assert_eq!(
        parsed.live.profiles[&LiveProfileId::parse("voice")?],
        LiveProfileEntry::Disabled
    );
    Ok(())
}

fn configured_profile_json() -> serde_json::Value {
    json!({
        "live": {
            "profiles": {
                "voice": {
                    "state": "configured",
                    "definition": {
                        "voice_identity": {"provider": "openai", "model": "voice-fixture"},
                        "execution": {
                            "mode": "client_context",
                            "request_policy": "snapshot_at_delegation",
                        },
                        "context_projection": "recent_authorized_suffix",
                    },
                },
            },
        },
    })
}

#[test]
fn json_config_rejects_nested_voice_identity_and_binding_typos() {
    for field in ["auth_bindng", "provider_paramss", "self_hosted_server"] {
        let mut value = configured_profile_json();
        value["live"]["profiles"]["voice"]["definition"]["voice_identity"][field] =
            json!({"realm": "owner", "binding": "voice"});
        assert!(serde_json::from_value::<meerkat_core::Config>(value).is_err());
    }
    let mut value = configured_profile_json();
    value["live"]["profiles"]["voice"]["definition"]["voice_identity"]["auth_binding"] =
        json!({"realm": "owner", "binding": "voice", "profil": "specific"});
    assert!(serde_json::from_value::<meerkat_core::Config>(value).is_err());
}

#[test]
fn toml_config_rejects_nested_voice_identity_and_binding_typos() {
    let prefix = r#"
        [live.profiles.voice]
        state = "configured"
        [live.profiles.voice.definition]
        context_projection = "recent_authorized_suffix"
        [live.profiles.voice.definition.execution]
        mode = "client_context"
        request_policy = "snapshot_at_delegation"
        [live.profiles.voice.definition.voice_identity]
        provider = "openai"
        model = "voice-fixture"
    "#;
    for entry in [
        r#"auth_bindng = { realm = "owner", binding = "voice" }"#,
        "provider_paramss = { temperature = 0.5 }",
        r#"auth_binding = { realm = "owner", binding = "voice", profil = "specific" }"#,
    ] {
        assert!(toml::from_str::<meerkat_core::Config>(&format!("{prefix}\n{entry}")).is_err());
    }
}

#[test]
fn strict_profile_binding_preserves_valid_selectors_without_changing_durable_identity() -> TestResult
{
    let binding = json!({"realm": "owner", "binding": "voice", "profile": "specific"});
    let mut value = configured_profile_json();
    value["live"]["profiles"]["voice"]["definition"]["voice_identity"]["auth_binding"] =
        binding.clone();
    let config: meerkat_core::Config = serde_json::from_value(value)?;
    let LiveProfileEntry::Configured(profile) =
        &config.live.profiles[&LiveProfileId::parse("voice")?]
    else {
        return Err("configured profile required".into());
    };
    assert_eq!(
        serde_json::to_value(&profile.voice_identity.auth_binding)?,
        binding
    );

    let durable: SessionLlmIdentity = serde_json::from_value(json!({
        "provider": "openai",
        "model": "voice-fixture",
        "future_durable_metadata": true,
    }))?;
    assert_eq!(durable.model, "voice-fixture");
    Ok(())
}

#[test]
#[cfg(feature = "schema")]
fn profile_voice_identity_schema_rejects_unknown_nested_fields() -> TestResult {
    let schema = serde_json::to_value(schemars::schema_for!(LiveProfileDefinition))?;
    assert_eq!(
        schema["$defs"]["LiveProfileVoiceIdentity"]["additionalProperties"],
        false
    );
    assert_eq!(
        schema["$defs"]["LiveProfileAuthBinding"]["additionalProperties"],
        false
    );
    Ok(())
}
