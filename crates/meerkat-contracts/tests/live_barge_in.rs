#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! K61 contract test (port of the deleted `realtime_barge_in` deterministic
//! test). Pins the `TurnInterrupted` observation shape so the projection
//! contract's barge-in signal cannot be silently renamed or restructured.
//!
//! Pairs with `live_audio_format.rs` as the fast-lane sentinel; the rich
//! integration coverage of barge-in (TTS audio + cross-turn recall) lives in
//! the e2e-smoke `e2e_scenario_71` test in `meerkat-integration-tests`,
//! which exercises the full provider path.

use meerkat_core::live_adapter::LiveAdapterObservation;

/// `TurnInterrupted` is the canonical signal that the user has barged in on
/// the assistant's current turn. The variant carries an optional
/// `response_id` (G4 P1) so projection sinks can bind the truncation to the
/// in-flight provider response even when the barge-in lands before any
/// transcript delta has been staged.
#[test]
fn realtime_barge_in() {
    let obs = LiveAdapterObservation::TurnInterrupted { response_id: None };
    let value = serde_json::to_value(&obs).unwrap();
    assert_eq!(
        value.get("observation").and_then(serde_json::Value::as_str),
        Some("turn_interrupted"),
        "TurnInterrupted observation tag must remain stable: {value}"
    );

    // `response_id` defaults to `None` and is `skip_serializing_if`-elided —
    // the unit-shaped wire form is preserved when no id is plumbed through.
    let object = value
        .as_object()
        .expect("TurnInterrupted must serialize as a JSON object");
    assert_eq!(
        object.len(),
        1,
        "TurnInterrupted with no response_id must serialize as a single-field \
         JSON object (just the tag); got {value}"
    );
}

/// Round-trip: a serialized `TurnInterrupted` deserializes back to the same
/// variant.
#[test]
fn turn_interrupted_round_trips_through_serde_json() {
    let obs = LiveAdapterObservation::TurnInterrupted { response_id: None };
    let json = serde_json::to_string(&obs).unwrap();
    let back: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        back,
        LiveAdapterObservation::TurnInterrupted { response_id: None }
    ));
}

/// Locally-confirmed wire literal — if this changes, every external client
/// (Python/TS/Web SDK, MCP transcript consumers) must be updated in lockstep.
/// `skip_serializing_if = "Option::is_none"` keeps the no-id wire form
/// backwards-compatible with consumers compiled against the pre-G4 shape.
#[test]
fn turn_interrupted_wire_literal_is_pinned() {
    let obs = LiveAdapterObservation::TurnInterrupted { response_id: None };
    let json = serde_json::to_string(&obs).unwrap();
    assert_eq!(json, r#"{"observation":"turn_interrupted"}"#);
}

/// G4 (P1): when the in-flight response id is plumbed through, it round-trips
/// through serde and shows up as a populated `response_id` field.
#[test]
fn turn_interrupted_carries_response_id() {
    let obs = LiveAdapterObservation::TurnInterrupted {
        response_id: Some("resp_42".into()),
    };
    let json = serde_json::to_string(&obs).unwrap();
    assert_eq!(
        json,
        r#"{"observation":"turn_interrupted","response_id":"resp_42"}"#
    );
    let back: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        back,
        LiveAdapterObservation::TurnInterrupted {
            response_id: Some(ref id),
        } if id == "resp_42"
    ));
}
