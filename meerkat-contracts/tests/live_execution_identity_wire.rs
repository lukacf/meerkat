use meerkat_contracts::wire::runtime::WireTurnMetadataOverride;
use meerkat_contracts::{
    LIVE_EXECUTION_IDENTITY_V1_CAPABILITY, LiveOpenParams, WireAuthBindingRef,
    WireLiveExecutionIdentityOverrideV1,
};
use serde_json::json;

const CLIENT_CONTEXT_PROFILE: &str = "openai.gpt-live-1-codex.client-context.v1";

#[test]
fn live_execution_identity_v1_decodes_exact_profile_selector()
-> Result<(), Box<dyn std::error::Error>> {
    let selector: WireLiveExecutionIdentityOverrideV1 = serde_json::from_value(json!({
        "version": "v1",
        "profile_id": CLIENT_CONTEXT_PROFILE
    }))?;

    assert_eq!(selector.profile_id, CLIENT_CONTEXT_PROFILE);
    assert_eq!(
        LIVE_EXECUTION_IDENTITY_V1_CAPABILITY,
        "live.execution_identity.v1"
    );
    Ok(())
}

#[test]
fn shipping_live_open_contract_accepts_exact_execution_identity_and_rejects_typos()
-> Result<(), Box<dyn std::error::Error>> {
    let parsed: LiveOpenParams = serde_json::from_value(json!({
        "session_id": "session-1",
        "transport": "webrtc",
        "execution_identity": {
            "version": "v1",
            "profile_id": CLIENT_CONTEXT_PROFILE
        }
    }))?;
    assert_eq!(
        parsed
            .execution_identity
            .as_ref()
            .map(|selector| selector.profile_id.as_str()),
        Some(CLIENT_CONTEXT_PROFILE)
    );

    assert!(
        serde_json::from_value::<LiveOpenParams>(json!({
            "session_id": "session-1",
            "execution_identit": {
                "version": "v1",
                "profile_id": CLIENT_CONTEXT_PROFILE
            }
        }))
        .is_err(),
        "an older or misspelled live/open contract must not ignore execution identity"
    );
    Ok(())
}

#[test]
fn live_execution_identity_v1_rejects_all_caller_supplied_identity_authority() {
    for (field, value) in [
        ("model", json!("gpt-live-1-codex")),
        ("provider", json!("openai")),
        ("self_hosted_server_id", json!("host-1")),
        (
            "auth_binding",
            json!({
                "action": "set",
                "value": { "realm": "global", "binding": "chatgpt" }
            }),
        ),
    ] {
        let mut request = json!({
            "version": "v1",
            "profile_id": CLIENT_CONTEXT_PROFILE
        });
        request[field] = value;
        assert!(
            serde_json::from_value::<WireLiveExecutionIdentityOverrideV1>(request).is_err(),
            "caller-supplied `{field}` authority must fail closed"
        );
    }
}

#[cfg(feature = "schema")]
#[test]
fn live_execution_identity_v1_schema_advertises_only_version_and_profile()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = serde_json::to_value(schemars::schema_for!(WireLiveExecutionIdentityOverrideV1))?;
    let properties = schema
        .pointer("/properties")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| std::io::Error::other("profile selector schema properties"))?;

    assert_eq!(properties.len(), 2);
    assert!(properties.contains_key("version"));
    assert!(properties.contains_key("profile_id"));
    for forbidden in ["model", "provider", "self_hosted_server_id", "auth_binding"] {
        assert!(!properties.contains_key(forbidden));
    }
    Ok(())
}

#[test]
fn live_execution_identity_v1_rejects_missing_unknown_or_empty_profile() {
    for invalid in [
        json!({ "profile_id": CLIENT_CONTEXT_PROFILE }),
        json!({ "version": "v2", "profile_id": CLIENT_CONTEXT_PROFILE }),
        json!({ "version": "v1" }),
        json!({ "version": "v1", "profile_id": "" }),
        json!({ "version": "v1", "profile_id": "  " }),
        json!({ "version": "v1", "profile_id": null }),
    ] {
        assert!(
            serde_json::from_value::<WireLiveExecutionIdentityOverrideV1>(invalid).is_err(),
            "live execution profile selector must be exact"
        );
    }
}

#[test]
fn legacy_auth_and_turn_override_decoders_remain_compatible()
-> Result<(), Box<dyn std::error::Error>> {
    let legacy_binding: WireAuthBindingRef = serde_json::from_value(json!({
        "realm": "global",
        "binding": "chatgpt",
        "origin": "configured"
    }))?;

    let legacy_override: WireTurnMetadataOverride<WireAuthBindingRef> =
        serde_json::from_value(json!({
            "realm": "global",
            "binding": "chatgpt"
        }))?;

    assert!(matches!(
        legacy_override,
        WireTurnMetadataOverride::Set(binding) if binding == legacy_binding
    ));
    Ok(())
}
