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
fn wire_metadata_legacy_clear_only_payloads_are_rejected_fail_closed() {
    let err = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
        "clear_provider_params": true,
        "clear_auth_binding": true,
    }))
    .expect_err("retired legacy split clear_* payloads must fail closed");
    let message = err.to_string();
    assert!(
        message.contains("unknown field") && message.contains("clear_"),
        "unexpected error: {err}"
    );
}

#[test]
fn wire_metadata_legacy_set_and_clear_payloads_fail_at_boundary() {
    let err = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
        "provider_params": { "temperature": 0.2 },
        "clear_provider_params": true,
    }))
    .expect_err("retired clear_provider_params field must fail closed");
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
    .expect_err("retired clear_auth_binding field must fail closed");
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
    // K2: the typed override is constructed directly — there is no legacy
    // JSON-bag projection on this seam anymore.
    let provider_params = ProviderParamsOverride {
        provider_tag: Some(
            meerkat_core::lifecycle::run_primitive::ProviderTag::Anthropic(
                meerkat_core::lifecycle::run_primitive::AnthropicProviderTag {
                    effort: Some(meerkat_core::lifecycle::run_primitive::AnthropicEffort::XHigh),
                    web_search: Some(
                        meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                            &serde_json::Value::Null,
                        ),
                    ),
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    };
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
    let Some(TurnMetadataOverride::Set(round_tripped_params)) = round_tripped.provider_params
    else {
        panic!("provider params set override should survive wire round trip");
    };
    let Some(meerkat_core::lifecycle::run_primitive::ProviderTag::Anthropic(tag)) =
        round_tripped_params.provider_tag
    else {
        panic!("anthropic provider tag should survive wire round trip");
    };
    assert_eq!(
        tag.effort,
        Some(meerkat_core::lifecycle::run_primitive::AnthropicEffort::XHigh)
    );
    let web_search = tag
        .web_search
        .expect("explicit provider-native null must survive wire round trip");
    assert!(web_search.as_value().is_null());
}

#[test]
fn wire_metadata_openai_reasoning_effort_none_and_xhigh_round_trip() {
    use meerkat_core::lifecycle::run_primitive::{OpenAiProviderTag, ProviderTag, ReasoningEffort};
    for (effort, wire_effort) in [
        (ReasoningEffort::None, "none"),
        (ReasoningEffort::XHigh, "xhigh"),
    ] {
        let provider_params = ProviderParamsOverride {
            provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                reasoning_effort: Some(effort),
                ..Default::default()
            })),
            ..Default::default()
        };
        let meta = RuntimeTurnMetadata {
            provider_params: Some(TurnMetadataOverride::Set(provider_params)),
            ..Default::default()
        };

        let wire: WireRuntimeTurnMetadata = meta.into();
        let json = serde_json::to_value(&wire).expect("serialize wire metadata");
        assert_eq!(
            json["provider_params"]["value"]["provider_tag"]["reasoning_effort"],
            serde_json::json!(wire_effort)
        );

        let parsed_wire: WireRuntimeTurnMetadata =
            serde_json::from_value(json).expect("deserialize wire metadata");
        let round_tripped: RuntimeTurnMetadata = parsed_wire.into();
        let Some(TurnMetadataOverride::Set(round_tripped_params)) = round_tripped.provider_params
        else {
            panic!("provider params set override should survive wire round trip");
        };
        let Some(ProviderTag::OpenAi(tag)) = round_tripped_params.provider_tag else {
            panic!("openai provider tag should survive wire round trip");
        };
        assert_eq!(tag.reasoning_effort, Some(effort));
    }
}
