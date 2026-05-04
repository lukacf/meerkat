#![allow(clippy::expect_used, clippy::panic)]

use meerkat_contracts::wire::WireAuthBindingRef;
use meerkat_contracts::wire::runtime::{
    WireProviderParamsOverride, WireRuntimeTurnMetadata, WireTurnMetadataOverride,
};
use meerkat_core::lifecycle::run_primitive::{
    ProviderParamsOverride, RuntimeTurnMetadata, TurnMetadataOverride,
};

fn wire_auth_binding() -> WireAuthBindingRef {
    WireAuthBindingRef {
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
    let auth_binding = wire_auth_binding();
    let meta = WireRuntimeTurnMetadata {
        provider_params: Some(WireTurnMetadataOverride::Set(provider_params.clone())),
        auth_binding: Some(WireTurnMetadataOverride::Set(auth_binding.clone())),
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
            "auth_binding": {
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
        parsed.auth_binding,
        Some(WireTurnMetadataOverride::Set(auth_binding))
    );
}

#[test]
fn wire_metadata_clear_overrides_round_trip() {
    let meta = WireRuntimeTurnMetadata {
        provider_params: Some(WireTurnMetadataOverride::Clear),
        auth_binding: Some(WireTurnMetadataOverride::Clear),
        ..Default::default()
    };

    let json = serde_json::to_value(&meta).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({
            "provider_params": { "action": "clear" },
            "auth_binding": { "action": "clear" },
        })
    );
    let parsed: WireRuntimeTurnMetadata = serde_json::from_value(json).expect("deserialize");
    assert_eq!(parsed, meta);
}

#[test]
fn wire_metadata_legacy_clear_only_deserializes_as_clear_overrides() {
    let parsed: WireRuntimeTurnMetadata = serde_json::from_value(serde_json::json!({
        "clear_provider_params": true,
        "clear_auth_binding": true,
    }))
    .expect("legacy clear-only payloads remain accepted");

    assert_eq!(
        parsed.provider_params,
        Some(WireTurnMetadataOverride::Clear)
    );
    assert_eq!(parsed.auth_binding, Some(WireTurnMetadataOverride::Clear));
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
        "auth_binding": {
            "realm": "dev",
            "binding": "default",
        },
        "clear_auth_binding": true,
    }))
    .expect_err("auth_binding set plus legacy clear must fail");
    assert!(
        err.to_string().contains("clear_auth_binding"),
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
        "auth_binding": {
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

#[test]
fn wire_metadata_provider_tag_preserves_provider_native_fields() {
    let provider_params = ProviderParamsOverride::from_legacy_provider_value(
        "anthropic",
        &serde_json::json!({
            "effort": "xhigh",
            "web_search": null,
        }),
    );
    let meta = RuntimeTurnMetadata {
        provider_params: Some(TurnMetadataOverride::Set(provider_params)),
        ..Default::default()
    };

    let wire: WireRuntimeTurnMetadata = meta.into();
    let json = serde_json::to_value(&wire).expect("serialize wire metadata");
    let provider_tag = json["provider_params"]["value"]["provider_tag"]
        .as_object()
        .expect("provider_tag object");
    assert_eq!(provider_tag["provider"], serde_json::json!("anthropic"));
    assert_eq!(provider_tag["effort"], serde_json::json!("xhigh"));
    assert!(
        provider_tag.contains_key("web_search"),
        "explicit provider-native null must not be dropped by wire projection"
    );
    assert!(provider_tag["web_search"].is_null());

    let parsed_wire: WireRuntimeTurnMetadata =
        serde_json::from_value(json).expect("deserialize wire metadata");
    let round_tripped: RuntimeTurnMetadata = parsed_wire.into();
    let Some(TurnMetadataOverride::Set(provider_params)) = round_tripped.provider_params else {
        panic!("provider params set override should survive wire round trip");
    };
    let legacy = provider_params.to_legacy_provider_value();
    assert_eq!(legacy["effort"], serde_json::json!("xhigh"));
    assert!(
        legacy
            .as_object()
            .is_some_and(|obj| obj.contains_key("web_search")),
        "explicit provider-native null must survive wire round trip"
    );
    assert!(legacy["web_search"].is_null());
}
