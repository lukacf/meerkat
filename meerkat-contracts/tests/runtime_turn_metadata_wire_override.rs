#![allow(clippy::expect_used)]

use meerkat_contracts::wire::WireConnectionRef;
use meerkat_contracts::wire::runtime::{
    WireProviderParamsOverride, WireRuntimeTurnMetadata, WireTurnMetadataOverride,
};

fn wire_connection_ref() -> WireConnectionRef {
    WireConnectionRef {
        realm: meerkat_core::connection::RealmId::parse("dev").expect("valid realm"),
        binding: meerkat_core::connection::BindingId::parse("default").expect("valid binding"),
        profile: None,
    }
}

#[test]
fn wire_metadata_tagged_overrides_round_trip() {
    let provider_params = WireProviderParamsOverride {
        temperature: Some(0.25),
        ..Default::default()
    };
    let connection_ref = wire_connection_ref();
    let meta = WireRuntimeTurnMetadata {
        provider_params: Some(WireTurnMetadataOverride::Set(provider_params.clone())),
        connection_ref: Some(WireTurnMetadataOverride::Set(connection_ref.clone())),
        ..Default::default()
    };

    let json = serde_json::to_value(&meta).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({
            "provider_params": {
                "action": "set",
                "value": { "temperature": 0.25 },
            },
            "connection_ref": {
                "action": "set",
                "value": {
                    "realm": "dev",
                    "binding": "default",
                },
            },
        })
    );
    let parsed: WireRuntimeTurnMetadata = serde_json::from_value(json).expect("deserialize");
    assert_eq!(
        parsed.provider_params,
        Some(WireTurnMetadataOverride::Set(provider_params))
    );
    assert_eq!(
        parsed.connection_ref,
        Some(WireTurnMetadataOverride::Set(connection_ref))
    );
}

#[test]
fn wire_metadata_clear_overrides_round_trip() {
    let meta = WireRuntimeTurnMetadata {
        provider_params: Some(WireTurnMetadataOverride::Clear),
        connection_ref: Some(WireTurnMetadataOverride::Clear),
        ..Default::default()
    };

    let json = serde_json::to_value(&meta).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({
            "provider_params": { "action": "clear" },
            "connection_ref": { "action": "clear" },
        })
    );
    let parsed: WireRuntimeTurnMetadata = serde_json::from_value(json).expect("deserialize");
    assert_eq!(parsed, meta);
}

#[test]
fn wire_metadata_legacy_clear_only_deserializes_as_clear_overrides() {
    let parsed: WireRuntimeTurnMetadata = serde_json::from_value(serde_json::json!({
        "clear_provider_params": true,
        "clear_connection_ref": true,
    }))
    .expect("legacy clear-only payloads remain accepted");

    assert_eq!(
        parsed.provider_params,
        Some(WireTurnMetadataOverride::Clear)
    );
    assert_eq!(parsed.connection_ref, Some(WireTurnMetadataOverride::Clear));
}

#[test]
fn wire_metadata_legacy_set_and_clear_payloads_fail_at_boundary() {
    let err = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
        "provider_params": { "temperature": 0.2 },
        "clear_provider_params": true,
    }))
    .expect_err("provider_params set plus legacy clear must fail");
    assert!(
        err.to_string().contains("clear_provider_params"),
        "unexpected error: {err}"
    );

    let err = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
        "connection_ref": {
            "realm": "dev",
            "binding": "default",
        },
        "clear_connection_ref": true,
    }))
    .expect_err("connection_ref set plus legacy clear must fail");
    assert!(
        err.to_string().contains("clear_connection_ref"),
        "unexpected error: {err}"
    );
}

#[test]
fn wire_metadata_malformed_tagged_override_payloads_fail_at_boundary() {
    let err = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
        "provider_params": {
            "action": 1,
            "value": { "temperature": 0.2 },
        },
    }))
    .expect_err("non-string override action must fail");
    assert!(
        err.to_string().contains("action"),
        "unexpected error: {err}"
    );

    let err = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
        "connection_ref": {
            "action": "clear",
            "value": {
                "realm": "dev",
                "binding": "default",
            },
        },
    }))
    .expect_err("clear override with value must fail");
    assert!(err.to_string().contains("clear"), "unexpected error: {err}");
}
