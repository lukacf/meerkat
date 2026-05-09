#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Contract tests for `RealtimeChannelStatus::deadline_at` (Item 4).
//!
//! The runtime-level jitter + per-session nudge knobs have behavioral
//! regression tests in their owning crates (meerkat-rpc, meerkat-client); this
//! file pins the wire-visible shape.

use meerkat_contracts::{RealtimeChannelState, RealtimeChannelStatus};

#[test]
fn channel_status_emits_deadline_at_when_present() {
    let status = RealtimeChannelStatus {
        state: RealtimeChannelState::Reconnecting,
        attempt_count: 2,
        next_retry_at: Some("2026-04-17T12:00:01Z".to_string()),
        deadline_at: Some("2026-04-17T12:00:30Z".to_string()),
        reason: Some("realtime attachment requires reattach".to_string()),
    };
    let value = serde_json::to_value(&status).expect("status serializes");
    assert_eq!(value["deadline_at"], "2026-04-17T12:00:30Z");
    assert_eq!(value["next_retry_at"], "2026-04-17T12:00:01Z");
    assert_eq!(value["state"], "reconnecting");
}

#[test]
fn channel_status_omits_deadline_at_when_none() {
    let status = RealtimeChannelStatus {
        state: RealtimeChannelState::Ready,
        attempt_count: 0,
        next_retry_at: None,
        deadline_at: None,
        reason: None,
    };
    let value = serde_json::to_value(&status).expect("status serializes");
    assert!(
        value.get("deadline_at").is_none() || value["deadline_at"].is_null(),
        "deadline_at must be skipped on the wire when there is no active reconnect cycle",
    );
}

#[test]
fn channel_status_deserializes_without_deadline_at_for_forward_compat() {
    // Legacy clients that do not understand `deadline_at` still produce valid
    // status payloads; the server-side deserializer treats the field as
    // optional additively.
    let payload = serde_json::json!({
        "state": "reconnecting",
        "attempt_count": 1,
        "next_retry_at": "2026-04-17T12:00:01Z"
    });
    let status: RealtimeChannelStatus =
        serde_json::from_value(payload).expect("legacy status deserializes");
    assert_eq!(status.state, RealtimeChannelState::Reconnecting);
    assert_eq!(status.deadline_at, None);
}
