#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Contract tests for the realtime tool-deadline wire surface (Item 3 of the
//! realtime hardening plan).

use meerkat_contracts::{
    RealtimeChannelConfig, RealtimeChannelEventFrame, RealtimeEvent, RealtimeOpenRequest,
    RealtimeServerFrame,
};

#[test]
fn channel_config_default_tool_timeout_resolves_to_15s() {
    let config = RealtimeChannelConfig::default();
    assert_eq!(
        config.tool_timeout_ms_or_default(),
        Some(RealtimeChannelConfig::DEFAULT_TOOL_TIMEOUT_MS),
        "default must be the 15 000 ms runtime-safe budget",
    );
}

#[test]
fn channel_config_zero_ms_disables_the_deadline() {
    // Operators that explicitly want unlimited tools can pass 0; product
    // code treats that as "no deadline" (distinguishable from "not set").
    let config = RealtimeChannelConfig {
        tool_timeout_ms: Some(0),
    };
    assert_eq!(config.tool_timeout_ms_or_default(), None);
}

#[test]
fn channel_config_explicit_value_is_honored() {
    let config = RealtimeChannelConfig {
        tool_timeout_ms: Some(7_500),
    };
    assert_eq!(config.tool_timeout_ms_or_default(), Some(7_500));
}

#[test]
fn channel_config_roundtrips_over_the_wire() {
    let config = RealtimeChannelConfig {
        tool_timeout_ms: Some(5_000),
    };
    let value = serde_json::to_value(&config).expect("channel config serializes");
    assert_eq!(value["tool_timeout_ms"], 5_000);

    let roundtrip: RealtimeChannelConfig =
        serde_json::from_value(value).expect("channel config deserializes");
    assert_eq!(roundtrip, config);
}

#[test]
fn channel_config_is_optional_on_open_request() {
    // Legacy open requests that do not carry channel_config deserialize cleanly
    // — the field is additive and the server falls back to defaults.
    let payload = serde_json::json!({
        "target": { "type": "session_target", "session_id": "s" },
        "role": "primary",
        "turning_mode": "provider_managed"
    });
    let request: RealtimeOpenRequest =
        serde_json::from_value(payload).expect("legacy open request deserializes");
    assert_eq!(request.channel_config, None);
}

#[test]
fn tool_call_timed_out_event_carries_typed_fields() {
    let event = RealtimeEvent::ToolCallTimedOut {
        call_id: "call_42".to_string(),
        elapsed_ms: 15_123,
    };
    let frame = RealtimeServerFrame::ChannelEvent(RealtimeChannelEventFrame { event });
    let value = serde_json::to_value(&frame).expect("frame serializes");
    assert_eq!(value["type"], "channel.event");
    assert_eq!(value["event"]["type"], "tool_call_timed_out");
    assert_eq!(value["event"]["call_id"], "call_42");
    assert_eq!(value["event"]["elapsed_ms"], 15_123);

    let roundtrip: RealtimeServerFrame = serde_json::from_value(value).expect("frame deserializes");
    match roundtrip {
        RealtimeServerFrame::ChannelEvent(ef) => match ef.event {
            RealtimeEvent::ToolCallTimedOut {
                call_id,
                elapsed_ms,
            } => {
                assert_eq!(call_id, "call_42");
                assert_eq!(elapsed_ms, 15_123);
            }
            other => panic!("expected ToolCallTimedOut, got {other:?}"),
        },
        other => panic!("expected ChannelEvent, got {other:?}"),
    }
}
