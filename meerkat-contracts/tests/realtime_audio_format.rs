#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Realtime audio format contract tests (Item 1 of the realtime hardening plan).
//!
//! These tests lock in the wire shape and typed-error semantics introduced by
//! bumping `RealtimeProtocolVersion::CURRENT` to v2: audio chunks must ship
//! sample rate + channel count, mismatches must surface as typed
//! `audio_format_mismatch` errors with a structured details payload, and the
//! protocol version handshake must expose the canonical enum.

use meerkat_contracts::{
    AudioFormatMismatchContext, RealtimeAudioChunk, RealtimeAudioFormat, RealtimeChannelErrorFrame,
    RealtimeErrorCode, RealtimeErrorDetails, RealtimeInputChunk, RealtimeProtocolVersion,
};

#[test]
fn realtime_audio_chunk_carries_typed_format_fields() {
    let chunk = RealtimeAudioChunk {
        mime_type: "audio/pcm".to_string(),
        sample_rate_hz: 24_000,
        channels: 1,
        data: "AQID".to_string(),
    };

    let value = serde_json::to_value(&chunk).expect("audio chunk serializes");
    assert_eq!(value["mime_type"], "audio/pcm");
    assert_eq!(value["sample_rate_hz"], 24_000);
    assert_eq!(value["channels"], 1);
    assert_eq!(value["data"], "AQID");

    let roundtrip: RealtimeAudioChunk =
        serde_json::from_value(value).expect("audio chunk deserializes");
    assert_eq!(roundtrip, chunk);

    // Projected format descriptor is a structured wire type, not a folk
    // serialization of `(rate, channels)` tuples.
    assert_eq!(
        roundtrip.format(),
        RealtimeAudioFormat::pcm(24_000, 1),
        "format() projection must roundtrip to the canonical descriptor",
    );
}

#[test]
fn audio_format_mismatch_error_is_typed_and_structured() {
    let expected = RealtimeAudioFormat::pcm(24_000, 1);
    let actual = RealtimeAudioFormat::pcm(16_000, 1);

    let frame = RealtimeChannelErrorFrame {
        code: RealtimeErrorCode::AudioFormatMismatch,
        message: "audio input format does not match provider negotiated format".to_string(),
        details: Some(RealtimeErrorDetails::AudioFormatMismatch(
            AudioFormatMismatchContext {
                expected: expected.clone(),
                actual: actual.clone(),
            },
        )),
    };

    let value = serde_json::to_value(&frame).expect("error frame serializes");
    assert_eq!(value["code"], "audio_format_mismatch");
    assert_eq!(value["details"]["kind"], "audio_format_mismatch");
    assert_eq!(value["details"]["expected"]["sample_rate_hz"], 24_000);
    assert_eq!(value["details"]["actual"]["sample_rate_hz"], 16_000);

    let roundtrip: RealtimeChannelErrorFrame =
        serde_json::from_value(value).expect("error frame deserializes");
    assert_eq!(roundtrip, frame);

    // SDKs match on the typed code enum, not on the string form.
    assert_eq!(roundtrip.code, RealtimeErrorCode::AudioFormatMismatch);
    match roundtrip.details {
        Some(RealtimeErrorDetails::AudioFormatMismatch(ctx)) => {
            assert_eq!(ctx.expected, expected);
            assert_eq!(ctx.actual, actual);
        }
        other => panic!("expected audio_format_mismatch details, got {other:?}"),
    }
}

#[test]
fn channel_error_frame_keeps_typed_code_on_the_wire() {
    let frame = RealtimeChannelErrorFrame {
        code: RealtimeErrorCode::InvalidFrame,
        message: "bad frame".to_string(),
        details: None,
    };
    let value = serde_json::to_value(&frame).expect("frame serializes");
    assert_eq!(value["code"], "invalid_frame");
    assert!(
        value.get("details").is_none() || value["details"].is_null(),
        "details must be omitted when None to keep wire payloads terse"
    );
}

#[test]
fn realtime_protocol_version_exposes_typed_current() {
    assert_eq!(RealtimeProtocolVersion::CURRENT.as_str(), "2");
    assert!(
        RealtimeProtocolVersion::CURRENT.is_supported(),
        "CURRENT must appear in SUPPORTED",
    );
    assert_eq!(
        RealtimeProtocolVersion::parse("2"),
        Some(RealtimeProtocolVersion::V2),
    );
    assert_eq!(RealtimeProtocolVersion::parse("legacy"), None);
}

#[test]
fn audio_chunk_without_rate_or_channels_fails_deserialization() {
    let legacy = serde_json::json!({
        "mime_type": "audio/pcm",
        "data": "AQID",
    });
    let result: Result<RealtimeAudioChunk, _> = serde_json::from_value(legacy);
    assert!(
        result.is_err(),
        "v1-shape audio chunks without sample_rate_hz/channels must fail closed"
    );
}

#[test]
fn input_chunk_audio_variant_roundtrips_with_format() {
    let chunk = RealtimeInputChunk::AudioChunk(RealtimeAudioChunk {
        mime_type: "audio/pcm".to_string(),
        sample_rate_hz: 24_000,
        channels: 1,
        data: "AAEC".to_string(),
    });
    let value = serde_json::to_value(&chunk).expect("input chunk serializes");
    assert_eq!(value["kind"], "audio_chunk");
    assert_eq!(value["sample_rate_hz"], 24_000);
    assert_eq!(value["channels"], 1);

    let roundtrip: RealtimeInputChunk =
        serde_json::from_value(value).expect("input chunk deserializes");
    assert_eq!(roundtrip, chunk);
}
