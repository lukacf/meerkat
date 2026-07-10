//! Shared live-input wire-contract decode owner (D243).
//!
//! Every Meerkat live transport — the RPC `live/send_input` JSON-RPC method,
//! the direct `/live/ws` WebSocket text path, and the WebRTC data channel —
//! parses client input as the wire-format [`LiveInputChunkWire`]
//! (`meerkat-contracts`). Supported transport inputs convert into the core
//! [`LiveInputChunk`] (`meerkat-core::live_adapter`) through this **single**
//! conversion + validation step before reaching the host. Direct WebSocket
//! and WebRTC deliberately reject image envelopes before conversion; images
//! use the JSON-RPC control plane because those transports cannot safely carry
//! the product's bounded image envelope.
//!
//! Previously each direct transport deserialized the core `LiveInputChunk`
//! shape directly (`serde_json::from_slice::<LiveInputChunk>`), bypassing the
//! generated `LiveInputChunkWire` boundary that the RPC path runs. That split
//! meant a payload rejected by `live/send_input` wire validation could still be
//! accepted on the WebRTC / WS direct paths. This module is the one conversion
//! owner for transport-supported envelopes. Base64 decode, bounded image
//! metadata/payload checks, and required idempotency-identity validation all
//! fail through a typed [`LiveInputChunkDecodeError`] rather than a free-form
//! provider string.

use base64::Engine;
use meerkat_contracts::LiveInputChunkWire;
use meerkat_core::live_adapter::{
    LiveAdapterErrorCode, LiveConfigRejectionReason, LiveInputChunk, MAX_LIVE_IMAGE_BASE64_BYTES,
    MAX_LIVE_IMAGE_BYTES, MAX_LIVE_IMAGE_IDEMPOTENCY_KEY_BYTES, MAX_LIVE_IMAGE_MIME_BYTES,
    live_image_idempotency_key_is_valid,
};

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
    /// The encoded image would exceed the decoded product safety ceiling.
    /// This check runs before base64 decoding so untrusted transports cannot
    /// force an arbitrarily large decode allocation.
    #[error(
        "live image input encoded payload is too large: {actual_bytes} bytes (maximum {max_bytes})"
    )]
    ImageEncodedTooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
    /// Defensive decoded-size backstop. Padded base64 length predicts this
    /// bound, but the post-decode check keeps the invariant explicit.
    #[error(
        "live image input decoded payload is too large: {actual_bytes} bytes (maximum {max_bytes})"
    )]
    ImageDecodedTooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
    /// The MIME declaration itself exceeded the bounded metadata envelope.
    #[error(
        "live image input MIME declaration is too large: {actual_bytes} bytes (maximum {max_bytes})"
    )]
    ImageMimeTooLong {
        max_bytes: usize,
        actual_bytes: usize,
    },
    /// The required session-scoped image idempotency identity was empty,
    /// exceeded the bounded metadata envelope, had surrounding whitespace,
    /// or contained control characters.
    #[error(
        "live image idempotency key is invalid: {actual_bytes} bytes (maximum {max_bytes}; empty is not allowed)"
    )]
    ImageInputIdempotencyKeyInvalid {
        max_bytes: usize,
        actual_bytes: usize,
    },
    /// The video-frame chunk's base64 `data` field was not valid base64.
    #[error("invalid base64 in live video-frame input chunk: {0}")]
    InvalidVideoFrameBase64(#[source] base64::DecodeError),
}

/// Decode a wire-format input chunk into the core [`LiveInputChunk`] shape.
///
/// This is the canonical conversion owner shared by every live transport so
/// wire contract validation (required image identity, bounded metadata/payload,
/// and base64 decoding of `data`) is identical everywhere. The RPC
/// `live/send_input` handler and the direct WS / WebRTC transports route
/// through this function.
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
        LiveInputChunkWire::Image {
            idempotency_key,
            mime,
            data,
        } => {
            if !live_image_idempotency_key_is_valid(&idempotency_key) {
                return Err(LiveInputChunkDecodeError::ImageInputIdempotencyKeyInvalid {
                    max_bytes: MAX_LIVE_IMAGE_IDEMPOTENCY_KEY_BYTES,
                    actual_bytes: idempotency_key.len(),
                });
            }
            if mime.len() > MAX_LIVE_IMAGE_MIME_BYTES {
                return Err(LiveInputChunkDecodeError::ImageMimeTooLong {
                    max_bytes: MAX_LIVE_IMAGE_MIME_BYTES,
                    actual_bytes: mime.len(),
                });
            }
            if data.len() > MAX_LIVE_IMAGE_BASE64_BYTES {
                return Err(LiveInputChunkDecodeError::ImageEncodedTooLarge {
                    max_bytes: MAX_LIVE_IMAGE_BASE64_BYTES,
                    actual_bytes: data.len(),
                });
            }
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(&data)
                .map_err(LiveInputChunkDecodeError::InvalidImageBase64)?;
            if decoded.len() > MAX_LIVE_IMAGE_BYTES {
                return Err(LiveInputChunkDecodeError::ImageDecodedTooLarge {
                    max_bytes: MAX_LIVE_IMAGE_BYTES,
                    actual_bytes: decoded.len(),
                });
            }
            Ok(LiveInputChunk::Image {
                idempotency_key,
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

/// Project shared image admission failures onto the scoped typed rejection
/// used uniformly by RPC, WebSocket, and WebRTC surfaces.
#[must_use]
pub fn live_input_chunk_decode_rejection(
    error: &LiveInputChunkDecodeError,
) -> Option<LiveAdapterErrorCode> {
    let reason = match error {
        LiveInputChunkDecodeError::InvalidImageBase64(_) => {
            LiveConfigRejectionReason::ImageInputInvalidBase64
        }
        LiveInputChunkDecodeError::ImageEncodedTooLarge { actual_bytes, .. } => {
            LiveConfigRejectionReason::ImageInputTooLarge {
                max_bytes: MAX_LIVE_IMAGE_BYTES as u64,
                actual_bytes: actual_bytes.div_ceil(4).saturating_mul(3) as u64,
            }
        }
        LiveInputChunkDecodeError::ImageDecodedTooLarge { actual_bytes, .. } => {
            LiveConfigRejectionReason::ImageInputTooLarge {
                max_bytes: MAX_LIVE_IMAGE_BYTES as u64,
                actual_bytes: *actual_bytes as u64,
            }
        }
        LiveInputChunkDecodeError::ImageMimeTooLong { actual_bytes, .. } => {
            LiveConfigRejectionReason::ImageInputUnsupportedMime {
                mime_type: format!("<too-long:{actual_bytes}-bytes>"),
            }
        }
        LiveInputChunkDecodeError::ImageInputIdempotencyKeyInvalid { actual_bytes, .. } => {
            LiveConfigRejectionReason::ImageInputIdempotencyKeyInvalid {
                max_bytes: MAX_LIVE_IMAGE_IDEMPOTENCY_KEY_BYTES as u64,
                actual_bytes: *actual_bytes as u64,
            }
        }
        _ => return None,
    };
    Some(LiveAdapterErrorCode::ConfigRejected { reason })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // D243: malformed base64 for a transport-supported envelope is rejected by
    // the shared conversion owner used by RPC, direct WebSocket, and WebRTC.
    // Image envelopes reach this conversion through RPC; direct WebSocket and
    // WebRTC reject that whole input kind at their transport boundary.
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
            idempotency_key: "image-request-malformed".to_string(),
            mime: "image/png".to_string(),
            data: "@@@not-base64@@@".to_string(),
        };
        match live_input_chunk_from_wire(wire) {
            Err(error @ LiveInputChunkDecodeError::InvalidImageBase64(_)) => {
                assert!(matches!(
                    live_input_chunk_decode_rejection(&error),
                    Some(LiveAdapterErrorCode::ConfigRejected {
                        reason: LiveConfigRejectionReason::ImageInputInvalidBase64,
                    })
                ));
            }
            other => panic!("expected InvalidImageBase64, got {other:?}"),
        }
    }

    #[test]
    fn oversized_image_is_rejected_before_base64_decode_allocation() {
        let wire = LiveInputChunkWire::Image {
            idempotency_key: "image-request-oversized".to_string(),
            mime: "image/png".to_string(),
            data: "A".repeat(MAX_LIVE_IMAGE_BASE64_BYTES + 1),
        };
        assert!(matches!(
            live_input_chunk_from_wire(wire),
            Err(LiveInputChunkDecodeError::ImageEncodedTooLarge {
                max_bytes: MAX_LIVE_IMAGE_BASE64_BYTES,
                actual_bytes,
            }) if actual_bytes == MAX_LIVE_IMAGE_BASE64_BYTES + 1
        ));
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
            idempotency_key: "image-request-valid".to_string(),
            mime: "image/png".to_string(),
            data: encoded.clone(),
        })
        .expect("valid image wire chunk decodes");
        assert!(matches!(
            image,
            LiveInputChunk::Image {
                ref mime,
                ref data,
                ref idempotency_key,
            } if mime == "image/png"
                && data == &[1u8, 2, 3, 4]
                && idempotency_key == "image-request-valid"
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
