//! `live/*` RPC wire contracts.
//!
//! Typed parameter and result shapes for the `live/open`, `live/status`,
//! `live/close`, `live/send_input`, `live/commit_input`, `live/interrupt`,
//! and `live/truncate` JSON-RPC methods. Promoted from
//! `meerkat-rpc/src/handlers/live.rs` (I52) so SDKs and the protocol catalog
//! can reference them as typed contracts rather than free-form `basic`
//! entries.
//!
//! Result shapes that carry semantic adapter state (`transport`,
//! `capabilities`, `continuity`, `status`) reference the canonical types in
//! `meerkat_core::live_adapter`. The RPC catalog publishes the types under
//! the `LiveOpenResult` / `LiveStatusResult` titles so SDK codegen sees a
//! single source of truth.

use serde::{Deserialize, Serialize};

use meerkat_core::live_adapter::{
    LiveAdapterStatus, LiveChannelCapabilities, LiveContinuityMode, LiveTransportBootstrap,
};

/// Request payload for `live/open`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveOpenParams {
    pub session_id: String,
}

/// Response payload for `live/open`.
///
/// `transport`, `capabilities`, `continuity` are typed in Rust as
/// `meerkat_core::live_adapter::*` shapes; for schema codegen they are
/// projected as opaque JSON objects (the core crate is not `schemars`-aware
/// today and adding `JsonSchema` derives to it would propagate a heavy
/// optional-feature surface across the entire workspace; the wire-side
/// schema therefore documents the field names and types via the
/// `Live{TransportBootstrap,ChannelCapabilities,ContinuityMode}` Rust types
/// the SDK codegen sees through the `meerkat-core` re-exports).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveOpenResult {
    pub channel_id: String,
    #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
    pub transport: LiveTransportBootstrap,
    #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
    pub capabilities: LiveChannelCapabilities,
    #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
    pub continuity: LiveContinuityMode,
}

/// Request payload for `live/status`, `live/close`, `live/commit_input`, and
/// `live/interrupt`. They all take the same `{channel_id}` shape; this
/// struct is the typed name for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveChannelParams {
    pub channel_id: String,
}

/// Response payload for `live/status`. See `LiveOpenResult` for the
/// rationale on the schema-side opaque projection of the core type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveStatusResult {
    pub channel_id: String,
    #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
    pub status: LiveAdapterStatus,
}

/// Modality-tagged input chunk for `live/send_input`.
///
/// Audio payloads are base64 strings (`data`) plus stamped sample-rate /
/// channel metadata so the adapter layer can validate against the negotiated
/// provider format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiveInputChunkWire {
    Audio {
        data: String,
        sample_rate_hz: u32,
        channels: u16,
    },
    Text {
        text: String,
    },
}

/// Request payload for `live/send_input`.
///
/// **`BREAKING_LIVE_WIRE_FORMAT_V1`** (H48): `chunk` is a nested object, not
/// a flattened sibling of `channel_id`. WS protocol clients that piggyback on
/// this shape must use the nested form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveSendInputParams {
    pub channel_id: String,
    pub chunk: LiveInputChunkWire,
}

/// Request payload for `live/truncate`.
///
/// `item_id` and `content_index` are the provider-side handle for the
/// assistant item being truncated; `audio_played_ms` is the client-tracked
/// playback cursor at the moment of truncation. There is no server-side
/// playback-cursor read API — clients track playback locally and pass the
/// cursor in here when they want to truncate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveTruncateParams {
    pub channel_id: String,
    pub item_id: String,
    pub content_index: u32,
    pub audio_played_ms: u64,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn live_open_params_round_trip() {
        let v = LiveOpenParams {
            session_id: "session-1".into(),
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        let back: LiveOpenParams = serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn live_send_input_params_audio_chunk_round_trip() {
        let v = LiveSendInputParams {
            channel_id: "live_1".into(),
            chunk: LiveInputChunkWire::Audio {
                data: "AQID".into(),
                sample_rate_hz: 24_000,
                channels: 1,
            },
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        let back: LiveSendInputParams =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn live_send_input_params_text_chunk_round_trip() {
        let v = LiveSendInputParams {
            channel_id: "live_1".into(),
            chunk: LiveInputChunkWire::Text {
                text: "hello".into(),
            },
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        let back: LiveSendInputParams =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn live_truncate_params_round_trip() {
        let v = LiveTruncateParams {
            channel_id: "live_1".into(),
            item_id: "item_42".into(),
            content_index: 0,
            audio_played_ms: 1_234,
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        let back: LiveTruncateParams =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }
}
