//! Fail-closed persistence-version contract.
//!
//! There is no migrating-read lane: every store read path deserializes
//! persisted rows through typed serde, and the generated
//! `session_persistence_version_authority` accepts exactly the current
//! version. A stored row with a missing, legacy (v0/v1), or future version
//! byte FAILS CLOSED with a typed rejection — it never silently defaults,
//! upgrades, or partially salvages on read.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_core::generated::session_persistence_version_authority::STORED_INPUT_STATE_VERSION;
use meerkat_core::{SESSION_METADATA_SCHEMA_VERSION, SESSION_VERSION, Session, SessionMetadata};
use serde_json::{Value, json};

const AUTHORITY_REJECTION: &str = "generated session persistence version authority rejected";

fn session_envelope(version: Option<Value>) -> Value {
    let mut envelope = json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "messages": [],
        "created_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
        "updated_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
    });
    if let Some(version) = version {
        envelope
            .as_object_mut()
            .unwrap()
            .insert("version".to_string(), version);
    }
    envelope
}

fn session_metadata_value(schema_version: Option<Value>) -> Value {
    let mut metadata = json!({
        "model": "gpt-5.4-mini",
        "max_tokens": 4096,
        "provider": "openai",
        "tooling": {},
        "comms_name": null,
    });
    if let Some(schema_version) = schema_version {
        metadata
            .as_object_mut()
            .unwrap()
            .insert("schema_version".to_string(), schema_version);
    }
    metadata
}

#[test]
fn current_version_session_envelope_round_trips() {
    let session = Session::new();
    let value = serde_json::to_value(&session).expect("serialize");
    assert_eq!(
        value.get("version").and_then(Value::as_u64),
        Some(u64::from(SESSION_VERSION)),
        "writer must stamp the current envelope version"
    );
    let parsed: Session = serde_json::from_value(value).expect("current version must round-trip");
    assert_eq!(parsed.version(), SESSION_VERSION);
}

#[test]
fn missing_session_envelope_version_fails_closed() {
    let err = serde_json::from_value::<Session>(session_envelope(None))
        .expect_err("missing envelope version must fail closed");
    assert!(
        err.to_string().contains("version"),
        "unexpected error: {err}"
    );
}

#[test]
fn legacy_v1_session_envelope_version_fails_closed() {
    let err = serde_json::from_value::<Session>(session_envelope(Some(json!(1))))
        .expect_err("legacy v1 envelope version must fail closed");
    assert!(
        err.to_string().contains(AUTHORITY_REJECTION),
        "unexpected error: {err}"
    );
}

#[test]
fn v0_zero_session_envelope_version_fails_closed() {
    let err = serde_json::from_value::<Session>(session_envelope(Some(json!(0))))
        .expect_err("v0 envelope version must fail closed");
    assert!(
        err.to_string().contains(AUTHORITY_REJECTION),
        "unexpected error: {err}"
    );
}

#[test]
fn future_session_envelope_version_fails_closed() {
    let err =
        serde_json::from_value::<Session>(session_envelope(Some(json!(SESSION_VERSION + 100))))
            .expect_err("future envelope version must fail closed");
    assert!(
        err.to_string().contains(AUTHORITY_REJECTION),
        "unexpected error: {err}"
    );
}

#[test]
fn missing_session_metadata_schema_version_fails_closed() {
    let err = serde_json::from_value::<SessionMetadata>(session_metadata_value(None))
        .expect_err("missing metadata schema version must fail closed");
    assert!(
        err.to_string().contains("schema_version"),
        "unexpected error: {err}"
    );
}

#[test]
fn legacy_v1_session_metadata_schema_version_fails_closed_on_session_read() {
    // The metadata bag is opaque at the Session envelope level; the typed
    // restore happens in `Session::try_session_metadata`, which must reject a
    // legacy schema-version byte through the generated authority.
    let mut envelope = session_envelope(Some(json!(SESSION_VERSION)));
    envelope["metadata"] = json!({
        "session_metadata": session_metadata_value(Some(json!(1))),
    });
    let session: Session =
        serde_json::from_value(envelope).expect("envelope itself is current-version");
    let err = session
        .try_session_metadata()
        .expect_err("legacy metadata schema version must fail closed");
    assert!(
        err.to_string().contains(AUTHORITY_REJECTION),
        "unexpected error: {err}"
    );
}

#[test]
fn future_session_metadata_schema_version_fails_closed_on_session_read() {
    let mut envelope = session_envelope(Some(json!(SESSION_VERSION)));
    envelope["metadata"] = json!({
        "session_metadata": session_metadata_value(Some(json!(
            SESSION_METADATA_SCHEMA_VERSION + 100
        ))),
    });
    let session: Session =
        serde_json::from_value(envelope).expect("envelope itself is current-version");
    let err = session
        .try_session_metadata()
        .expect_err("future metadata schema version must fail closed");
    assert!(
        err.to_string().contains(AUTHORITY_REJECTION),
        "unexpected error: {err}"
    );
}

#[test]
fn stored_input_state_version_is_pinned_to_current() {
    // The runtime crate owns `StoredInputState` serde; this pins the shared
    // constant so the rejection tests there and the authority here agree.
    // v3: persisted input content unified onto the single typed
    // `ContentInput` carrier (dual text/body/instructions + blocks deleted).
    assert_eq!(STORED_INPUT_STATE_VERSION, 3);
    assert_eq!(SESSION_VERSION, 2);
    assert_eq!(SESSION_METADATA_SCHEMA_VERSION, 2);
}
