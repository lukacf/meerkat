#![cfg(feature = "experimental-gpt-live")]

use oai_rt_rs::experimental::gpt_live::{
    ClientDelegation, ClientEvent, CodecError, ContextChannel, Delegation, DelegationContextAppend,
    DelegationFunctionCallOutput, EventCarrier, ExtraFields, FunctionCallId, FunctionCallOutput,
    FunctionTool, InputTextContent, MAX_BRIDGE_ARGUMENT_BYTES, MAX_FUNCTION_OUTPUT_BYTES,
    MAX_RAW_JSON_EVENT_BYTES, ResponsesConfig, ResponsesDelegation, ServerEvent,
    SessionContextAppend, decode_bridge_arguments, decode_received_server_event,
    decode_server_event, encode_client_event, encode_server_event,
};
use serde_json::{Value, json};
use std::error::Error;

const FIXTURE_DIR: &str = "tests/fixtures/gpt_live_v3";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURE_DIR}/{name}"))
        .unwrap_or_else(|error| panic!("read fixture {name}: {error}"))
}

fn assert_error_chain_redacted(error: &dyn Error, secrets: &[&str]) {
    let mut current = Some(error);
    while let Some(entry) = current {
        let diagnostics = format!("{entry}\n{entry:?}");
        for secret in secrets {
            assert!(
                !diagnostics.contains(secret),
                "error diagnostics leaked {secret}"
            );
        }
        current = entry.source();
    }
}

#[test]
fn verified_server_fixtures_decode_to_typed_events() {
    let cases = [
        ("session_started.json", "session.started"),
        ("session_context_appended.json", "session.context.appended"),
        ("input_transcript_added.json", "input_transcript.added"),
        ("output_transcript_added.json", "output_transcript.added"),
        ("turn_created.json", "turn.created"),
        ("turn_delta.json", "turn.delta"),
        ("turn_done.json", "turn.done"),
        ("delegation_created.json", "delegation.created"),
        (
            "delegation_context_appended.json",
            "delegation.context.appended",
        ),
    ];

    for (name, expected_kind) in cases {
        let event = decode_server_event(&fixture(name))
            .unwrap_or_else(|error| panic!("decode {name}: {error}"));
        assert_eq!(event.kind(), expected_kind);
        assert!(!matches!(event, ServerEvent::Unknown(_)));
    }
}

#[test]
fn unknown_event_round_trips_without_losing_json() {
    let input = fixture("unknown_event.json");
    let expected: Value = serde_json::from_str(&input).expect("fixture JSON");
    let event = decode_server_event(&input).expect("unknown event decode");
    let ServerEvent::Unknown(unknown) = &event else {
        panic!("unknown discriminant must remain unknown");
    };
    assert_eq!(unknown.raw(), &expected);
    let encoded = encode_server_event(&event).expect("unknown event encode");
    assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), expected);
}

#[test]
fn received_unknown_event_preserves_carrier_and_exact_wire_size() {
    let input = fixture("unknown_event.json");
    let observation = decode_received_server_event(EventCarrier::OrderedOaiEvents, &input)
        .expect("received unknown event");
    assert_eq!(observation.carrier(), EventCarrier::OrderedOaiEvents);
    assert_eq!(observation.byte_count(), input.len());
    assert!(matches!(observation.event(), ServerEvent::Unknown(_)));
}

#[test]
fn known_event_preserves_top_level_and_nested_extras() {
    let input = fixture("known_event_with_extras.json");
    let expected: Value = serde_json::from_str(&input).expect("fixture JSON");
    let event = decode_server_event(&input).expect("known event decode");
    let ServerEvent::DelegationCreated(created) = &event else {
        panic!("expected delegation.created");
    };
    assert_eq!(
        created.extra.get("future_top_level"),
        Some(&json!("preserve-me"))
    );
    assert_eq!(
        created.item.extra.get("future_nested"),
        Some(&json!({ "enabled": true }))
    );
    let encoded = encode_server_event(&event).expect("known event encode");
    assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), expected);
}

#[test]
fn malformed_known_event_is_not_downgraded_to_unknown() {
    let error = decode_server_event(&fixture("malformed_known_event.json"))
        .expect_err("malformed known event must fail");
    assert!(matches!(
        error,
        CodecError::MalformedKnownEvent {
            kind: "delegation.created",
            ..
        }
    ));
}

#[test]
fn malformed_known_event_debug_redacts_unexpected_private_values() {
    let error = decode_server_event(
        r#"{"type":"delegation.created","offset_ms":"FIXTURE_PRIVATE_MALFORMED_SECRET","item":{}}"#,
    )
    .expect_err("malformed known event must fail");
    let debug = format!("{error:?}");
    assert!(debug.contains("MalformedKnownEvent"));
    assert!(!debug.contains("FIXTURE_PRIVATE_MALFORMED_SECRET"));
    assert_error_chain_redacted(&error, &["FIXTURE_PRIVATE_MALFORMED_SECRET"]);
}

#[test]
fn invalid_discriminants_fail_closed() {
    assert!(matches!(
        decode_server_event(r#"{"value":1}"#),
        Err(CodecError::MissingDiscriminant)
    ));
    assert!(matches!(
        decode_server_event(r#"{"type":1}"#),
        Err(CodecError::InvalidDiscriminant)
    ));
    assert!(matches!(
        decode_server_event("[]"),
        Err(CodecError::NotAnObject)
    ));
}

#[test]
fn client_events_serialize_to_verified_wire_shapes() {
    let content = InputTextContent {
        content_type: "input_text".to_owned(),
        text: "FIXTURE_PRIVATE_SESSION_CONTEXT".to_owned(),
        extra: ExtraFields::new(),
    };
    let session = ClientEvent::SessionContextAppend(SessionContextAppend {
        channel: None,
        content: vec![content.clone()],
        extra: ExtraFields::new(),
    });
    let delegation = ClientEvent::DelegationContextAppend(DelegationContextAppend {
        delegation_item_id: "item_fixture_delegation".to_owned(),
        channel: Some(ContextChannel::Speakable),
        content: vec![InputTextContent {
            text: "FIXTURE_PRIVATE_DELEGATION_CONTEXT".to_owned(),
            ..content
        }],
        extra: ExtraFields::new(),
    });
    let function_output = ClientEvent::DelegationFunctionCallOutput(
        DelegationFunctionCallOutput::new(FunctionCallOutput::new(
            FunctionCallId::new("call_fixture_bridge"),
            "FIXTURE_PRIVATE_FUNCTION_OUTPUT",
        )),
    );

    for (event, name) in [
        (session, "session_context_append.json"),
        (delegation, "delegation_context_append.json"),
        (
            function_output,
            "delegation_function_call_output_create.json",
        ),
    ] {
        let encoded = encode_client_event(&event).expect("client event encode");
        let actual: Value = serde_json::from_str(&encoded).unwrap();
        let expected: Value = serde_json::from_str(&fixture(name)).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn client_and_responses_delegation_configs_are_typed_and_exact() {
    let client = Delegation::Client(ClientDelegation::default());
    let client_actual = serde_json::to_value(client).expect("client delegation");
    let client_expected: Value =
        serde_json::from_str(&fixture("delegation_client.json")).expect("client fixture");
    assert_eq!(client_actual, client_expected);

    let responses = Delegation::Responses(ResponsesDelegation::new(
        ResponsesConfig {
            model: "gpt-fixture-bridge".to_owned(),
            instructions: Some("FIXTURE_PRIVATE_BRIDGE_INSTRUCTIONS".to_owned()),
            tools: vec![FunctionTool::new(
                "invoke_meerkat",
                "Delegate to the channel-bound fixture agent.",
                json!({
                    "type": "object",
                    "properties": {
                        "request": { "type": "string" }
                    },
                    "required": ["request"],
                    "additionalProperties": false
                }),
                ExtraFields::new(),
            )],
            extra: ExtraFields::new(),
        },
        ExtraFields::new(),
    ));
    let responses_actual = serde_json::to_value(responses).expect("responses delegation");
    let responses_expected: Value =
        serde_json::from_str(&fixture("delegation_responses.json")).expect("responses fixture");
    assert_eq!(responses_actual, responses_expected);
}

#[test]
fn function_call_output_nested_type_is_not_caller_controlled() {
    let malformed = json!({
        "type": "not_function_call_output",
        "call_id": "call_fixture_bridge",
        "output": "FIXTURE_PRIVATE_FUNCTION_OUTPUT"
    });
    assert!(serde_json::from_value::<FunctionCallOutput>(malformed).is_err());
}

#[test]
fn qualified_hard_bounds_are_enforced_in_utf8_bytes() {
    assert_eq!(
        decode_bridge_arguments(&fixture("bridge_arguments.json"))
            .expect("bridge fixture")
            .request(),
        "FIXTURE_PRIVATE_BRIDGE_REQUEST"
    );

    let prefix = r#"{"type":"fixture.unknown","padding":""#;
    let suffix = r#""}"#;
    let exact = format!(
        "{prefix}{}{suffix}",
        "x".repeat(MAX_RAW_JSON_EVENT_BYTES - prefix.len() - suffix.len())
    );
    assert_eq!(exact.len(), MAX_RAW_JSON_EVENT_BYTES);
    assert!(matches!(
        decode_server_event(&exact),
        Ok(ServerEvent::Unknown(_))
    ));
    let oversized = format!("{exact} ");
    assert!(matches!(
        decode_server_event(&oversized),
        Err(CodecError::OversizedRawEvent)
    ));

    let exact = format!(
        r#"{{"request":"{}"}}"#,
        "x".repeat(MAX_BRIDGE_ARGUMENT_BYTES)
    );
    assert_eq!(
        decode_bridge_arguments(&exact)
            .expect("exact bridge argument bound")
            .request()
            .len(),
        MAX_BRIDGE_ARGUMENT_BYTES
    );
    let oversized = format!(
        r#"{{"request":"{}"}}"#,
        "x".repeat(MAX_BRIDGE_ARGUMENT_BYTES + 1)
    );
    assert!(matches!(
        decode_bridge_arguments(&oversized),
        Err(CodecError::OversizedBridgeArguments)
    ));
    let oversized_utf8 = format!(
        r#"{{"request":"{}"}}"#,
        "é".repeat(MAX_BRIDGE_ARGUMENT_BYTES / 2 + 1)
    );
    assert!(matches!(
        decode_bridge_arguments(&oversized_utf8),
        Err(CodecError::OversizedBridgeArguments)
    ));
    assert!(matches!(
        decode_bridge_arguments(r#"{"request":"ok","identity_id":"forbidden"}"#),
        Err(CodecError::MalformedBridgeArguments)
    ));

    let exact_output = "x".repeat(MAX_FUNCTION_OUTPUT_BYTES);
    let exact_event = ClientEvent::DelegationFunctionCallOutput(DelegationFunctionCallOutput::new(
        FunctionCallOutput::new(FunctionCallId::new("call_fixture_bound"), exact_output),
    ));
    encode_client_event(&exact_event).expect("exact output bound");

    let oversized_secret = "s".repeat(MAX_FUNCTION_OUTPUT_BYTES + 1);
    let oversized_event = ClientEvent::DelegationFunctionCallOutput(
        DelegationFunctionCallOutput::new(FunctionCallOutput::new(
            FunctionCallId::new("call_fixture_oversized"),
            oversized_secret.clone(),
        )),
    );
    let error = encode_client_event(&oversized_event).expect_err("oversized function output");
    assert!(matches!(error, CodecError::OversizedFunctionOutput));
    assert_error_chain_redacted(&error, &[&oversized_secret]);
}

#[test]
fn delegation_context_supports_all_verified_channel_shapes() {
    for (channel, fixture_name) in [
        (None, "delegation_context_append_omitted.json"),
        (
            Some(ContextChannel::Commentary),
            "delegation_context_append_commentary.json",
        ),
        (
            Some(ContextChannel::Speakable),
            "delegation_context_append.json",
        ),
    ] {
        let event = ClientEvent::DelegationContextAppend(DelegationContextAppend {
            delegation_item_id: "item_fixture_delegation".to_owned(),
            channel,
            content: vec![InputTextContent {
                content_type: "input_text".to_owned(),
                text: "FIXTURE_PRIVATE_DELEGATION_CONTEXT".to_owned(),
                extra: ExtraFields::new(),
            }],
            extra: ExtraFields::new(),
        });
        let actual: Value =
            serde_json::from_str(&encode_client_event(&event).expect("delegation context encode"))
                .expect("encoded JSON");
        let expected: Value = serde_json::from_str(&fixture(fixture_name)).expect("fixture JSON");
        assert_eq!(actual, expected);
    }
}

#[test]
fn session_context_channel_is_optional_and_top_level() {
    let event = ClientEvent::SessionContextAppend(SessionContextAppend {
        channel: Some(ContextChannel::Speakable),
        content: vec![InputTextContent {
            content_type: "input_text".to_owned(),
            text: "FIXTURE_PRIVATE_SESSION_CONTEXT".to_owned(),
            extra: ExtraFields::new(),
        }],
        extra: ExtraFields::new(),
    });
    let actual: Value =
        serde_json::from_str(&encode_client_event(&event).expect("session context encode"))
            .expect("encoded JSON");
    let expected: Value = serde_json::from_str(&fixture("session_context_append_speakable.json"))
        .expect("fixture JSON");
    assert_eq!(actual, expected);
    assert!(actual["content"][0].get("channel").is_none());
}

#[test]
fn client_event_top_level_extras_cannot_collide_with_reserved_fields() {
    for field in ["type", "channel", "content"] {
        let mut extra = ExtraFields::new();
        extra.insert(field.to_owned(), json!("FIXTURE_PRIVATE_COLLISION"));
        let event = ClientEvent::SessionContextAppend(SessionContextAppend {
            channel: None,
            content: vec![],
            extra,
        });
        assert!(matches!(
            encode_client_event(&event),
            Err(CodecError::ReservedExtraField {
                scope: "session context append",
                field: rejected,
            }) if rejected == field
        ));
    }

    for field in ["type", "delegation_item_id", "channel", "content"] {
        let mut extra = ExtraFields::new();
        extra.insert(field.to_owned(), json!("FIXTURE_PRIVATE_COLLISION"));
        let event = ClientEvent::DelegationContextAppend(DelegationContextAppend {
            delegation_item_id: "item_fixture_delegation".to_owned(),
            channel: None,
            content: vec![],
            extra,
        });
        assert!(matches!(
            encode_client_event(&event),
            Err(CodecError::ReservedExtraField {
                scope: "delegation context append",
                field: rejected,
            }) if rejected == field
        ));
    }
}

#[test]
fn input_content_extras_cannot_inject_nested_channel_or_known_fields() {
    for field in ["type", "text", "channel"] {
        let mut extra = ExtraFields::new();
        extra.insert(field.to_owned(), json!("FIXTURE_PRIVATE_NESTED_COLLISION"));
        let event = ClientEvent::SessionContextAppend(SessionContextAppend {
            channel: None,
            content: vec![InputTextContent {
                content_type: "input_text".to_owned(),
                text: "FIXTURE_PRIVATE_CONTEXT".to_owned(),
                extra,
            }],
            extra: ExtraFields::new(),
        });
        assert!(matches!(
            encode_client_event(&event),
            Err(CodecError::ReservedExtraField {
                scope: "input text content",
                field: rejected,
            }) if rejected == field
        ));
    }
}
