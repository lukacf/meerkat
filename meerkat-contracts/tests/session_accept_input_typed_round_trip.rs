#![allow(clippy::expect_used)]

//! Wave B (V8): the input-acceptance RPC method has a typed request shape.
//!
//! The ad-hoc raw-JSON ingress is gone; callers construct a
//! `SessionAcceptInputParams { session_id, primitive, idempotency_key,
//! turn_metadata }` where `primitive: WireRunPrimitive` carries the typed
//! payload. This test round-trips the three primitive variants and asserts
//! that turn metadata + idempotency round-trip intact.

use meerkat_contracts::wire::runtime::{
    SessionAcceptInputParams, WireConversationAppend, WireConversationAppendRole,
    WireConversationContextAppend, WireRunPrimitive, WireRuntimeTurnMetadata, WireStagedRunInput,
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
        turn_metadata: None,
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
        turn_metadata: None,
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
        turn_metadata: Some(WireRuntimeTurnMetadata::default()),
    };
    let json = serde_json::to_string(&params).expect("serialize");
    assert!(json.contains("\"kind\":\"staged_input\""));
    assert!(json.contains("\"boundary\":42"));
    let back: SessionAcceptInputParams = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, params);
}
