#![allow(clippy::expect_used)]

use meerkat_contracts::wire::WireConnectionRef;
use meerkat_contracts::wire::runtime::{
    WireProviderParamsOverride, WireRuntimeTurnMetadata, WireTurnMetadataOverride,
};
use meerkat_core::lifecycle::run_primitive::{
    ProviderParamsOverride, RuntimeTurnMetadata, TurnMetadataOverride,
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
fn wire_metadata_legacy_split_clear_payloads_fail_closed() {
    let err = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
        "clear_provider_params": true,
        "clear_connection_ref": true,
    }))
    .expect_err("legacy split clear fields must not be accepted inside turn_metadata");
    assert!(
        err.to_string().contains("clear_provider_params")
            || err.to_string().contains("unknown field"),
        "unexpected error: {err}"
    );

    let err = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
        "provider_params": {
            "action": "set",
            "value": { "temperature": 0.2 }
        },
        "clear_provider_params": true,
    }))
    .expect_err("provider_params set plus legacy clear must fail");
    assert!(
        err.to_string().contains("clear_provider_params"),
        "unexpected error: {err}"
    );

    let err = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
        "connection_ref": {
            "action": "set",
            "value": {
                "realm": "dev",
                "binding": "default",
            }
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
fn wire_metadata_unknown_nested_fields_fail_closed() {
    let err = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
        "skill_references": [],
        "flow_tool_overaly": {},
    }))
    .expect_err("misspelled nested turn metadata fields must fail closed");
    assert!(
        err.to_string().contains("flow_tool_overaly") || err.to_string().contains("unknown field"),
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
    assert!(
        err.to_string().contains("clear") || err.to_string().contains("value"),
        "unexpected error: {err}"
    );
}

#[test]
fn wire_metadata_rejects_runtime_owned_stamps() {
    for (field, value) in [
        ("execution_kind", serde_json::json!("content_turn")),
        (
            "peer_response_terminal_apply_intent",
            serde_json::json!("append_context_and_run"),
        ),
    ] {
        let err = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
            field: value,
        }))
        .expect_err("public wire turn_metadata must reject runtime-owned stamps");
        let message = err.to_string();
        assert!(
            message.contains(field) || message.contains("unknown field"),
            "unexpected error for {field}: {message}"
        );
    }
}

#[test]
fn wire_metadata_untagged_overrides_fail_closed() {
    for (field, value) in [
        (
            "provider_params",
            serde_json::json!({
                "temperature": 0.2,
            }),
        ),
        (
            "connection_ref",
            serde_json::json!({
                "realm": "dev",
                "binding": "default",
            }),
        ),
        (
            "keep_alive",
            serde_json::json!({
                "ttl_secs": 30,
                "policy": "pinned",
            }),
        ),
    ] {
        let result = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
            field: value,
        }));
        assert!(
            result.is_err(),
            "missing action must fail closed for turn_metadata.{field}"
        );
        let err = result.expect_err("result checked as error");
        let message = err.to_string();
        assert!(
            message.contains("action") || message.contains("tag"),
            "unexpected error for {field}: {message}"
        );
    }
}

#[test]
fn wire_metadata_override_wrapper_unknown_fields_fail_closed() {
    for (field, value, unknown) in [
        (
            "provider_params",
            serde_json::json!({
                "action": "set",
                "value": { "temperature": 0.2 },
                "clear_provider_params": true,
            }),
            "clear_provider_params",
        ),
        (
            "connection_ref",
            serde_json::json!({
                "action": "clear",
                "binding_id": "legacy-default",
            }),
            "binding_id",
        ),
        (
            "keep_alive",
            serde_json::json!({
                "action": "set",
                "value": {
                    "ttl_secs": 30,
                    "policy": "pinned",
                },
                "ttl": 30,
            }),
            "ttl",
        ),
    ] {
        let result = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
            field: value,
        }));
        assert!(
            result.is_err(),
            "unknown override wrapper field {unknown} must fail closed for turn_metadata.{field}"
        );
        let err = result.expect_err("result checked as error");
        let message = err.to_string();
        assert!(
            message.contains(unknown) || message.contains("unknown field"),
            "unexpected error for {field}: {message}"
        );
    }
}

#[test]
fn wire_metadata_nested_unknown_fields_fail_closed() {
    for (field, value, unknown) in [
        (
            "skill_references",
            serde_json::json!([
                {
                    "source_uuid": "dc256086-0d2f-4f61-a307-320d4148107f",
                    "skill_name": "email-extractor",
                    "source": "legacy",
                }
            ]),
            "source",
        ),
        (
            "flow_tool_overlay",
            serde_json::json!({
                "allowed_tools": ["shell"],
                "allow_tools": ["legacy-shell"],
            }),
            "allow_tools",
        ),
        (
            "additional_instructions",
            serde_json::json!([
                {
                    "kind": "user",
                    "body": "stay concise",
                    "role": "legacy-user",
                }
            ]),
            "role",
        ),
        (
            "provider_params",
            serde_json::json!({
                "action": "set",
                "value": {
                    "temperature": 0.2,
                    "temperatre": 0.7,
                }
            }),
            "temperatre",
        ),
        (
            "connection_ref",
            serde_json::json!({
                "action": "set",
                "value": {
                    "realm": "dev",
                    "binding": "default",
                    "binding_id": "legacy-default",
                }
            }),
            "binding_id",
        ),
        (
            "keep_alive",
            serde_json::json!({
                "action": "set",
                "value": {
                    "ttl_secs": 30,
                    "policy": "pinned",
                    "ttl": 30,
                }
            }),
            "ttl",
        ),
        (
            "render_metadata",
            serde_json::json!({
                "class": "user_prompt",
                "salience": "normal",
                "style": "legacy",
            }),
            "style",
        ),
    ] {
        let result = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
            field: value,
        }));
        assert!(
            result.is_err(),
            "unknown nested field {unknown} must fail closed for turn_metadata.{field}"
        );
        let err = result.expect_err("result checked as error");
        let message = err.to_string();
        assert!(
            message.contains(unknown) || message.contains("unknown field"),
            "unexpected error for {field}: {message}"
        );
    }
}

#[test]
fn wire_provider_tag_nested_unknown_fields_fail_closed() {
    for (value, unknown) in [
        (
            serde_json::json!({
                "provider": "anthropic",
                "thinking_budget_tokens": 2048,
                "thinking_budget": 4096,
            }),
            "thinking_budget",
        ),
        (
            serde_json::json!({
                "provider": "anthropic",
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 2048,
                    "budget": 4096,
                },
            }),
            "budget",
        ),
        (
            serde_json::json!({
                "provider": "anthropic",
                "compaction": {
                    "kind": "custom",
                    "edit": { "instructions": "compact" },
                    "edits": { "instructions": "legacy" },
                },
            }),
            "edits",
        ),
        (
            serde_json::json!({
                "provider": "open_ai",
                "reasoning_effort": "high",
                "effort": "legacy-high",
            }),
            "effort",
        ),
        (
            serde_json::json!({
                "provider": "gemini",
                "thinking": {
                    "include_thoughts": true,
                    "include_thinking": true,
                },
            }),
            "include_thinking",
        ),
        (
            serde_json::json!({
                "provider": "unknown",
                "bag": {
                    "namespace": "legacy",
                    "key": "knob",
                    "body": "{}",
                    "extra": true,
                },
            }),
            "extra",
        ),
    ] {
        let result = serde_json::from_value::<WireRuntimeTurnMetadata>(serde_json::json!({
            "provider_params": {
                "action": "set",
                "value": {
                    "provider_tag": value,
                },
            },
        }));
        assert!(
            result.is_err(),
            "unknown provider tag field {unknown} must fail closed"
        );
        let err = result.expect_err("result checked as error");
        let message = err.to_string();
        assert!(
            message.contains(unknown) || message.contains("unknown field"),
            "unexpected provider tag error: {message}"
        );
    }
}

#[cfg(feature = "schema")]
mod schema_emission {
    use super::*;
    use meerkat_contracts::wire::runtime::{
        StructuredProviderExtension, WireAnthropicCompactionConfig, WireAnthropicThinkingConfig,
        WireGeminiThinkingConfig, WireProviderTag,
    };

    fn schema_json<T: schemars::JsonSchema>() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(T)).unwrap()
    }

    fn assert_schema_closed(name: &str, schema: &serde_json::Value) {
        assert_eq!(
            schema.pointer("/additionalProperties"),
            Some(&serde_json::Value::Bool(false)),
            "{name} schema must be closed"
        );
    }

    fn assert_schema_variants_closed(name: &str, schema: &serde_json::Value) {
        let variants = schema
            .pointer("/oneOf")
            .or_else(|| schema.pointer("/anyOf"))
            .and_then(serde_json::Value::as_array)
            .expect("schema should expose variants");

        for variant in variants {
            assert_schema_closed(name, variant);
        }
    }

    #[test]
    fn nested_runtime_metadata_schemas_reject_additional_properties() {
        for (name, schema) in [
            (
                "WireProviderParamsOverride",
                schema_json::<WireProviderParamsOverride>(),
            ),
            ("WireConnectionRef", schema_json::<WireConnectionRef>()),
            (
                "TurnToolOverlay",
                schema_json::<meerkat_core::TurnToolOverlay>(),
            ),
            ("SkillKey", schema_json::<meerkat_core::skills::SkillKey>()),
            (
                "WireRenderMetadata",
                schema_json::<meerkat_contracts::wire::WireRenderMetadata>(),
            ),
            (
                "StructuredProviderExtension",
                schema_json::<StructuredProviderExtension>(),
            ),
            (
                "WireGeminiThinkingConfig",
                schema_json::<WireGeminiThinkingConfig>(),
            ),
        ] {
            assert_schema_closed(name, &schema);
        }
    }

    #[test]
    fn provider_tag_schemas_reject_additional_properties() {
        for (name, schema) in [
            ("WireProviderTag", schema_json::<WireProviderTag>()),
            (
                "WireAnthropicThinkingConfig",
                schema_json::<WireAnthropicThinkingConfig>(),
            ),
            (
                "WireAnthropicCompactionConfig",
                schema_json::<WireAnthropicCompactionConfig>(),
            ),
        ] {
            assert_schema_variants_closed(name, &schema);
        }
    }

    #[test]
    fn turn_metadata_override_schema_variants_reject_additional_properties() {
        let schema = schema_json::<WireTurnMetadataOverride<WireProviderParamsOverride>>();
        assert_schema_variants_closed("WireTurnMetadataOverride variant", &schema);
    }
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
