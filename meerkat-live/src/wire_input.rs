//! Shared live-input wire-contract decode owner (D243).
//!
//! Every Meerkat live transport — the RPC `live/send_input` JSON-RPC method,
//! the direct `/live/ws` WebSocket text path, and the WebRTC data channel —
//! receives client input as the wire-format [`LiveInputChunkWire`]
//! (`meerkat-contracts`) and must convert it into the core
//! [`LiveInputChunk`] (`meerkat-core::live_adapter`) through a **single**
//! conversion + validation step before handing it to the host.
//!
//! Previously each direct transport deserialized the core `LiveInputChunk`
//! shape directly (`serde_json::from_slice::<LiveInputChunk>`), bypassing the
//! generated `LiveInputChunkWire` boundary that the RPC path runs. That split
//! meant a payload rejected by `live/send_input` wire validation could still be
//! accepted on the WebRTC / WS direct paths. This module is the one
//! conversion owner so all transports validate identically; the base64 decode
//! is the only step that can fail and it fails with a typed
//! [`LiveInputChunkDecodeError`] rather than a free-form provider string.

use base64::Engine;
use meerkat_contracts::LiveInputChunkWire;
use meerkat_core::live_adapter::LiveInputChunk;

/// Typed failure decoding a wire-format [`LiveInputChunkWire`] into the core
/// [`LiveInputChunk`]. Each base64 field that can be malformed lands in a
/// distinct variant so callers route on the typed class instead of reparsing a
/// prose string.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LiveInputChunkDecodeError {
    /// The audio chunk's base64 `data` field was not valid base64.
    #[error("invalid base64 in live audio input chunk: {0}")]
    InvalidAudioBase64(#[source] base64::DecodeError),
    /// The image chunk's base64 `data` field was not valid base64.
    #[error("invalid base64 in live image input chunk: {0}")]
    InvalidImageBase64(#[source] base64::DecodeError),
    /// The video-frame chunk's base64 `data` field was not valid base64.
    #[error("invalid base64 in live video-frame input chunk: {0}")]
    InvalidVideoFrameBase64(#[source] base64::DecodeError),
}

/// Decode a wire-format input chunk into the core [`LiveInputChunk`] shape.
///
/// This is the canonical conversion owner shared by every live transport so
/// the wire contract validation (base64 decoding of `data`) is identical
/// everywhere. The RPC `live/send_input` handler and the direct WS / WebRTC
/// transports route through this function.
pub fn live_input_chunk_from_wire(
    wire: LiveInputChunkWire,
) -> Result<LiveInputChunk, LiveInputChunkDecodeError> {
    match wire {
        LiveInputChunkWire::Audio {
            data,
            sample_rate_hz,
            channels,
        } => {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(&data)
                .map_err(LiveInputChunkDecodeError::InvalidAudioBase64)?;
            Ok(LiveInputChunk::Audio {
                data: decoded,
                sample_rate_hz,
                channels,
            })
        }
        LiveInputChunkWire::Text { text } => Ok(LiveInputChunk::Text { text }),
        LiveInputChunkWire::Image { mime, data } => {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(&data)
                .map_err(LiveInputChunkDecodeError::InvalidImageBase64)?;
            Ok(LiveInputChunk::Image {
                mime,
                data: decoded,
            })
        }
        LiveInputChunkWire::VideoFrame {
            codec,
            data,
            timestamp_ms,
        } => {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(&data)
                .map_err(LiveInputChunkDecodeError::InvalidVideoFrameBase64)?;
            Ok(LiveInputChunk::VideoFrame {
                codec,
                data: decoded,
                timestamp_ms,
            })
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // D243: a chunk rejected by the wire decode owner (malformed base64) is
    // rejected by the single shared helper that every transport runs — the WS
    // text path, the WebRTC data channel, and RPC `live/send_input` all route
    // through `live_input_chunk_from_wire`, so a payload that fails here fails
    // on every transport.
    #[test]
    fn malformed_audio_base64_is_rejected_with_typed_variant() {
        let wire = LiveInputChunkWire::Audio {
            data: "not valid base64!!!".to_string(),
            sample_rate_hz: 24_000,
            channels: 1,
        };
        match live_input_chunk_from_wire(wire) {
            Err(LiveInputChunkDecodeError::InvalidAudioBase64(_)) => {}
            other => panic!("expected InvalidAudioBase64, got {other:?}"),
        }
    }

    #[test]
    fn malformed_image_base64_is_rejected_with_typed_variant() {
        let wire = LiveInputChunkWire::Image {
            mime: "image/png".to_string(),
            data: "@@@not-base64@@@".to_string(),
        };
        match live_input_chunk_from_wire(wire) {
            Err(LiveInputChunkDecodeError::InvalidImageBase64(_)) => {}
            other => panic!("expected InvalidImageBase64, got {other:?}"),
        }
    }

    #[test]
    fn malformed_video_frame_base64_is_rejected_with_typed_variant() {
        let wire = LiveInputChunkWire::VideoFrame {
            codec: "vp8".to_string(),
            data: "%%%".to_string(),
            timestamp_ms: 42,
        };
        match live_input_chunk_from_wire(wire) {
            Err(LiveInputChunkDecodeError::InvalidVideoFrameBase64(_)) => {}
            other => panic!("expected InvalidVideoFrameBase64, got {other:?}"),
        }
    }

    #[test]
    fn well_formed_wire_chunks_decode_to_core_chunks() {
        let encoded = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3, 4]);

        let audio = live_input_chunk_from_wire(LiveInputChunkWire::Audio {
            data: encoded.clone(),
            sample_rate_hz: 24_000,
            channels: 1,
        })
        .expect("valid audio wire chunk decodes");
        assert!(matches!(
            audio,
            LiveInputChunk::Audio { ref data, .. } if data == &[1u8, 2, 3, 4]
        ));

        let text = live_input_chunk_from_wire(LiveInputChunkWire::Text {
            text: "hello".to_string(),
        })
        .expect("text wire chunk decodes");
        assert!(matches!(text, LiveInputChunk::Text { text } if text == "hello"));

        let image = live_input_chunk_from_wire(LiveInputChunkWire::Image {
            mime: "image/png".to_string(),
            data: encoded.clone(),
        })
        .expect("valid image wire chunk decodes");
        assert!(matches!(
            image,
            LiveInputChunk::Image { ref mime, ref data } if mime == "image/png" && data == &[1u8, 2, 3, 4]
        ));

        let frame = live_input_chunk_from_wire(LiveInputChunkWire::VideoFrame {
            codec: "vp8".to_string(),
            data: encoded,
            timestamp_ms: 7,
        })
        .expect("valid video-frame wire chunk decodes");
        assert!(matches!(
            frame,
            LiveInputChunk::VideoFrame { ref codec, ref data, timestamp_ms }
                if codec == "vp8" && data == &[1u8, 2, 3, 4] && timestamp_ms == 7
        ));
    }
}
