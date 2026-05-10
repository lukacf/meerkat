#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! K61 contract test (port of the deleted `realtime_audio_format` deterministic
//! test). Pins the wire shape of live-adapter audio observations so a future
//! refactor cannot regress D23 (no JSON int arrays for audio) without tripping
//! a fast-lane gate.
//!
//! The richer round-trip + base64 unit tests live alongside the type in
//! `meerkat-core::live_adapter::tests`; this file is the equivalent contract
//! sentinel from the contracts crate's perspective so wire-format regressions
//! show up in `cargo nextest run -p meerkat-contracts` even if the core unit
//! tests are skipped.

use meerkat_core::live_adapter::LiveAdapterObservation;

/// D23: assistant audio chunks must serialize the PCM bytes as a base64
/// string, not a JSON array of integers. JSON integer arrays bloat the wire
/// ~6× and force a JSON parse on every audio frame.
#[test]
fn realtime_audio_format() {
    let obs = LiveAdapterObservation::AssistantAudioChunk {
        data: vec![1u8, 2, 3, 4, 5],
        sample_rate_hz: 24_000,
        channels: 1,
        response_id: None,
        item_id: None,
        content_index: None,
    };
    let value = serde_json::to_value(&obs).unwrap();

    assert_eq!(
        value.get("observation").and_then(|v| v.as_str()),
        Some("assistant_audio_chunk"),
        "observation tag must remain stable: {value}"
    );

    let data = value
        .get("data")
        .expect("AssistantAudioChunk must serialize a `data` field");
    assert!(
        data.is_string(),
        "AssistantAudioChunk.data MUST be a base64 string (D23); got {data} in {value}"
    );

    // Round-trip: deserialize back and recover the original bytes.
    let json = serde_json::to_string(&obs).unwrap();
    let back: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
    match back {
        LiveAdapterObservation::AssistantAudioChunk {
            data,
            sample_rate_hz,
            channels,
            ..
        } => {
            assert_eq!(data, vec![1u8, 2, 3, 4, 5]);
            assert_eq!(sample_rate_hz, 24_000);
            assert_eq!(channels, 1);
        }
        other => panic!("expected AssistantAudioChunk after round-trip, got {other:?}"),
    }
}

/// Companion sanity: the `sample_rate_hz` and `channels` fields are required
/// scalars, not optional. Future negotiation work (H43) must not silently
/// turn these into `Option` without a deliberate wire bump.
#[test]
fn assistant_audio_chunk_carries_sample_rate_and_channels() {
    let obs = LiveAdapterObservation::AssistantAudioChunk {
        data: vec![0u8; 4],
        sample_rate_hz: 48_000,
        channels: 2,
        response_id: None,
        item_id: None,
        content_index: None,
    };
    let value = serde_json::to_value(&obs).expect("audio chunk must serialize");
    assert_eq!(
        value
            .get("sample_rate_hz")
            .and_then(serde_json::Value::as_u64),
        Some(48_000)
    );
    assert_eq!(
        value.get("channels").and_then(serde_json::Value::as_u64),
        Some(2)
    );
}
