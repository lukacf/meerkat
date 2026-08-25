use meerkat_contracts::wire::runtime::WireTurnMetadataOverride;
use meerkat_contracts::{
    LIVE_EXECUTION_IDENTITY_V1_CAPABILITY, LiveOpenParams, WireAuthBindingRef,
    WireLiveExecutionIdentityOverrideV1, WireLiveIdentityOverride,
};
use serde_json::json;

#[test]
fn live_execution_identity_v1_decodes_exact_set_and_clear_shapes()
-> Result<(), Box<dyn std::error::Error>> {
    let set: WireLiveExecutionIdentityOverrideV1 = serde_json::from_value(json!({
        "version": "v1",
        "model": "gpt-live-1-codex",
        "provider": "openai",
        "auth_binding": {
            "action": "set",
            "value": {
                "realm": "global",
                "binding": "chatgpt",
                "profile": "default"
            }
        }
    }))?;

    let Some(WireLiveIdentityOverride::Set(binding)) = set.auth_binding else {
        return Err(std::io::Error::other("set shape must decode as an explicit set").into());
    };
    let set_binding = WireAuthBindingRef::from(binding);
    assert_eq!(
        serde_json::to_value(set_binding)?,
        json!({
            "realm": "global",
            "binding": "chatgpt",
            "profile": "default"
        })
    );

    let shared_override: WireTurnMetadataOverride<u8> = WireLiveIdentityOverride::Set(7).into();
    assert_eq!(shared_override, WireTurnMetadataOverride::Set(7));

    let clear: WireLiveExecutionIdentityOverrideV1 = serde_json::from_value(json!({
        "version": "v1",
        "auth_binding": { "action": "clear" }
    }))?;
    assert!(matches!(
        clear.auth_binding,
        Some(WireLiveIdentityOverride::Clear)
    ));
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
            "model": "gpt-live-1-codex",
            "provider": "openai",
            "auth_binding": {
                "action": "set",
                "value": { "realm": "global", "binding": "chatgpt" }
            }
        }
    }))?;
    assert!(parsed.execution_identity.is_some());

    assert!(
        serde_json::from_value::<LiveOpenParams>(json!({
            "session_id": "session-1",
            "execution_identit": {
                "version": "v1",
                "model": "gpt-live-1-codex"
            }
        }))
        .is_err(),
        "an older or misspelled live/open contract must not ignore execution identity"
    );
    Ok(())
}

#[test]
fn live_execution_identity_v1_rejects_unknown_fields_at_each_object_level() {
    for invalid in [
        json!({
            "version": "v1",
            "model": "gpt-live-1-codex",
            "modle": "typo"
        }),
        json!({
            "version": "v1",
            "auth_binding": {
                "action": "set",
                "value": { "realm": "global", "binding": "chatgpt" },
                "ignored": true
            }
        }),
        json!({
            "version": "v1",
            "auth_binding": {
                "action": "set",
                "value": {
                    "realm": "global",
                    "binding": "chatgpt",
                    "origin": "configured"
                }
            }
        }),
    ] {
        assert!(
            serde_json::from_value::<WireLiveExecutionIdentityOverrideV1>(invalid).is_err(),
            "unknown live execution identity fields must fail closed"
        );
    }
}

#[test]
fn live_execution_identity_v1_rejects_invalid_tri_state_shapes() {
    for invalid_auth_binding in [
        json!({ "action": "set" }),
        json!({ "action": "clear", "value": { "realm": "global", "binding": "chatgpt" } }),
        json!({ "action": "clear", "value": null }),
        json!({ "action": "set", "value": { "realm": "global", "binding": "chatgpt" }, "extra": true }),
        json!({ "action": "clear", "extra": true }),
        json!({ "realm": "global", "binding": "chatgpt" }),
        json!({ "action": "inherit" }),
    ] {
        assert!(
            serde_json::from_value::<WireLiveExecutionIdentityOverrideV1>(json!({
                "version": "v1",
                "auth_binding": invalid_auth_binding
            }))
            .is_err(),
            "invalid live binding override must fail closed"
        );
    }
}

#[test]
fn live_execution_identity_v1_rejects_ambiguous_nulls() {
    for (field, invalid) in [
        ("model", json!(null)),
        ("provider", json!(null)),
        ("self_hosted_server_id", json!(null)),
        ("auth_binding", json!(null)),
    ] {
        let mut request = json!({ "version": "v1" });
        request[field] = invalid;
        assert!(
            serde_json::from_value::<WireLiveExecutionIdentityOverrideV1>(request).is_err(),
            "explicit null must not collapse into inherit"
        );
    }
}

#[test]
fn live_execution_identity_v1_rejects_empty_identity_strings() {
    for (field, invalid) in [("model", json!("")), ("self_hosted_server_id", json!("  "))] {
        let mut request = json!({ "version": "v1" });
        request[field] = invalid;
        assert!(
            serde_json::from_value::<WireLiveExecutionIdentityOverrideV1>(request).is_err(),
            "empty execution identity strings must fail closed"
        );
    }
}

#[test]
fn live_execution_identity_v1_rejects_missing_or_unknown_versions() {
    for invalid in [
        json!({ "model": "gpt-live-1-codex" }),
        json!({ "version": "v2", "model": "gpt-live-1-codex" }),
    ] {
        assert!(
            serde_json::from_value::<WireLiveExecutionIdentityOverrideV1>(invalid).is_err(),
            "live execution identity version must be exact"
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
