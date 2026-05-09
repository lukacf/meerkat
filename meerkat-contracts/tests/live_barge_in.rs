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
/// the assistant's current turn. The variant must remain unit-shaped (no
/// payload) and serialize as a single-field JSON object tagged
/// `"observation": "turn_interrupted"`.
#[test]
fn turn_interrupted_serializes_as_tagged_unit_variant() {
    let obs = LiveAdapterObservation::TurnInterrupted;
    let value = serde_json::to_value(&obs).unwrap();
    assert_eq!(
        value.get("observation").and_then(serde_json::Value::as_str),
        Some("turn_interrupted"),
        "TurnInterrupted observation tag must remain stable: {value}"
    );

    // No payload fields — barge-in is currently signal-only. If a future
    // change adds payload fields (e.g. cursor offsets), it must update this
    // test deliberately.
    let object = value
        .as_object()
        .expect("TurnInterrupted must serialize as a JSON object");
    assert_eq!(
        object.len(),
        1,
        "TurnInterrupted must currently carry exactly one field (`observation`); got {value}"
    );
}

/// Round-trip: a serialized `TurnInterrupted` deserializes back to the same
/// variant.
#[test]
fn turn_interrupted_round_trips_through_serde_json() {
    let obs = LiveAdapterObservation::TurnInterrupted;
    let json = serde_json::to_string(&obs).unwrap();
    let back: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
    assert!(matches!(back, LiveAdapterObservation::TurnInterrupted));
}

/// Locally-confirmed wire literal — if this changes, every external client
/// (Python/TS/Web SDK, MCP transcript consumers) must be updated in lockstep.
#[test]
fn turn_interrupted_wire_literal_is_pinned() {
    let obs = LiveAdapterObservation::TurnInterrupted;
    let json = serde_json::to_string(&obs).unwrap();
    assert_eq!(json, r#"{"observation":"turn_interrupted"}"#);
}
