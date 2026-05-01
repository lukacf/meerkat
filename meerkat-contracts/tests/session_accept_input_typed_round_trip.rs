#![allow(clippy::expect_used)]

//! Wave B (V8): the input-acceptance RPC method has a typed request shape.
//!
//! The ad-hoc raw-JSON ingress is gone; callers construct a
//! `SessionAcceptInputParams { session_id, primitive, idempotency_key }`
//! where `primitive: WireRunPrimitive` carries the typed payload. This test
//! round-trips the three primitive variants and asserts that staged turn
//! metadata + idempotency round-trip intact.

use meerkat_contracts::wire::runtime::{
    RuntimeAcceptParams, SessionAcceptInputParams, SessionExternalEventEnvelope,
    WireConversationAppend, WireConversationAppendRole, WireConversationContextAppend,
    WireRunPrimitive, WireRuntimeTurnMetadata, WireStagedRunInput,
};

#[test]
fn immediate_append_round_trip() {
    let params = SessionAcceptInputParams {
        session_id: "00000000-0000-0000-0000-000000000001".into(),
        primitive: WireRunPrimitive::ImmediateAppend(WireConversationAppend {
            role: WireConversationAppendRole::User,
            blocks: Vec::new(),
        }),
        idempotency_key: Some("key-1".into()),
    };
    let json = serde_json::to_string(&params).expect("serialize");
    assert!(json.contains("\"kind\":\"immediate_append\""));
    assert!(json.contains("\"idempotency_key\":\"key-1\""));
    let back: SessionAcceptInputParams = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, params);
}

#[test]
fn immediate_context_append_round_trip() {
    let params = SessionAcceptInputParams {
        session_id: "session-x".into(),
        primitive: WireRunPrimitive::ImmediateContextAppend(WireConversationContextAppend {
            key: "system-notice-1".into(),
            blocks: Vec::new(),
        }),
        idempotency_key: None,
    };
    let json = serde_json::to_string(&params).expect("serialize");
    assert!(json.contains("\"kind\":\"immediate_context_append\""));
    assert!(json.contains("\"key\":\"system-notice-1\""));
    let back: SessionAcceptInputParams = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, params);
}

#[test]
fn staged_input_round_trip_with_turn_metadata() {
    let staged = WireStagedRunInput {
        contributing_input_ids: vec!["i-1".into(), "i-2".into()],
        appends: vec![WireConversationAppend {
            role: WireConversationAppendRole::User,
            blocks: Vec::new(),
        }],
        context_appends: vec![],
        boundary: Some(42),
        turn_metadata: Some(WireRuntimeTurnMetadata::default()),
    };
    let params = SessionAcceptInputParams {
        session_id: "session-staged".into(),
        primitive: WireRunPrimitive::StagedInput(staged),
        idempotency_key: Some("once".into()),
    };
    let json = serde_json::to_string(&params).expect("serialize");
    assert!(json.contains("\"kind\":\"staged_input\""));
    assert!(json.contains("\"boundary\":42"));
    let back: SessionAcceptInputParams = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, params);
}

#[test]
fn session_accept_input_rejects_retired_top_level_turn_metadata() {
    let err = serde_json::from_value::<SessionAcceptInputParams>(serde_json::json!({
        "session_id": "session-staged",
        "primitive": {
            "kind": "staged_input",
            "contributing_input_ids": [],
            "appends": [],
            "context_appends": [],
            "turn_metadata": {
                "model": "metadata-model"
            }
        },
        "turn_metadata": {
            "model": "retired-top-level-model"
        }
    }))
    .expect_err("typed accept-input params must not accept a second top-level metadata carrier");

    let message = err.to_string();
    assert!(
        message.contains("turn_metadata") || message.contains("unknown field"),
        "unexpected error: {message}"
    );
}

#[test]
fn session_accept_input_rejects_retired_metadata_inside_immediate_primitives() {
    for primitive in [
        serde_json::json!({
            "kind": "immediate_append",
            "role": "user",
            "blocks": [],
            "turn_metadata": {
                "model": "retired-immediate-model"
            }
        }),
        serde_json::json!({
            "kind": "immediate_context_append",
            "key": "system-notice-1",
            "blocks": [],
            "provider_params": {
                "effort": "retired"
            }
        }),
    ] {
        let err = serde_json::from_value::<SessionAcceptInputParams>(serde_json::json!({
            "session_id": "session-staged",
            "primitive": primitive
        }))
        .expect_err("immediate primitives must reject stale metadata fields");

        let message = err.to_string();
        assert!(
            message.contains("turn_metadata")
                || message.contains("provider_params")
                || message.contains("unknown field"),
            "unexpected error: {message}"
        );
    }
}

#[test]
fn runtime_accept_params_rejects_retired_top_level_turn_metadata() {
    let err = serde_json::from_value::<RuntimeAcceptParams>(serde_json::json!({
        "session_id": "session-staged",
        "input": {
            "input_type": "prompt",
            "header": {
                "id": "00000000-0000-0000-0000-000000000001",
                "timestamp": "2026-05-01T00:00:00Z",
                "source": "operator",
                "durability": "durable",
                "visibility": {
                    "transcript_eligible": true,
                    "operator_eligible": true
                }
            },
            "text": "hello",
            "turn_metadata": {
                "model": "metadata-model"
            }
        },
        "turn_metadata": {
            "model": "retired-top-level-model"
        }
    }))
    .expect_err("runtime/session_submit must not accept top-level turn_metadata");

    let message = err.to_string();
    assert!(
        message.contains("turn_metadata") || message.contains("unknown field"),
        "unexpected error: {message}"
    );
}

#[test]
fn session_external_event_envelope_rejects_stale_metadata_fields() {
    let err = serde_json::from_value::<SessionExternalEventEnvelope>(serde_json::json!({
        "kind": "generic_json",
        "event_type": "webhook.created",
        "payload": {
            "ok": true
        },
        "turn_metadata": {
            "model": "retired-event-model"
        }
    }))
    .expect_err("external-event envelope must reject stale metadata fields");

    let message = err.to_string();
    assert!(
        message.contains("turn_metadata") || message.contains("unknown field"),
        "unexpected error: {message}"
    );
}
