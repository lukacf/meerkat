//! Handlers for `live/*` RPC methods.
//!
//! These expose the live adapter MVP surface: open/status/close/send_input.
//! The transport bootstrap is tagged (websocket/webrtc), not a bare ws_url.

use std::sync::Arc;

use meerkat_client::realtime_session::RealtimeSessionFactory;
use meerkat_contracts::{
    LiveChannelParams, LiveInputChunkWire, LiveOpenParams, LiveOpenResult, LiveSendInputParams,
    LiveStatusResult, LiveTruncateParams, RealtimeTurningMode,
};
use meerkat_core::live_adapter::{
    LiveAdapterCommand, LiveChannelCapabilities, LiveContinuityMode, LiveInputChunk,
    LiveTransportBootstrap,
};
use meerkat_core::types::SessionId;
use meerkat_live::LiveWsState;
use meerkat_runtime::live_adapter_host::{LiveAdapterHost, LiveAdapterHostError, LiveChannelId};
use serde::{Deserialize, Serialize};

use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::{LiveOpenPrecheckError, SessionRuntime};

/// Capabilities reported when the adapter has not yet declared its real
/// capability set. All fields default `false` so callers cannot mistake an
/// unknown adapter for a fully-featured one (dogma sin: no resume/feature
/// lies).
fn unknown_capabilities() -> LiveChannelCapabilities {
    LiveChannelCapabilities {
        audio_input: false,
        audio_output: false,
        text_input: false,
        text_output: false,
        barge_in: false,
        transcript: false,
        provider_native_resume: false,
    }
}

pub async fn handle_live_open(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    host: &LiveAdapterHost,
    live_ws: Option<&LiveWsState>,
    live_ws_base_url: Option<&str>,
    runtime: &Arc<SessionRuntime>,
    session_factory: Option<&dyn RealtimeSessionFactory>,
) -> RpcResponse {
    let parsed: LiveOpenParams = match super::parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let session_id = match SessionId::parse(&parsed.session_id) {
        Ok(id) => id,
        Err(err) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("invalid session_id: {err}"),
            );
        }
    };

    // B17: validate the session exists before minting a channel.
    // Without this, a nonexistent-session call mints a channel that the
    // adapter then can't bind to anything truthful, leaving stale infra
    // handles behind.
    if runtime.session_state(&session_id).await.is_none()
        && !runtime.pending_session_exists(&session_id).await
    {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("session {session_id} not found"),
        );
    }

    let channel_id = match host.open_channel(session_id.clone()).await {
        Ok(ch) => ch,
        Err(LiveAdapterHostError::SessionAlreadyBound(sid)) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("session {sid} already has an active live channel"),
            );
        }
        Err(err) => {
            return RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string());
        }
    };

    if let Some(factory) = session_factory {
        // B18: refuse providers without a wired live adapter and models that
        // lack realtime capability before reaching the factory. Without this,
        // a Gemini Live session would route through the OpenAI realtime
        // factory and a non-realtime model would silently bind to live
        // transport, surfacing only as an opaque WebSocket handshake error
        // at provider connect time.
        if let Err(precheck_err) = runtime.precheck_live_open(&session_id).await {
            let _ = host.close_channel(&channel_id).await;
            let (code, message) = match &precheck_err {
                LiveOpenPrecheckError::ModelNotRealtime { model, provider } => (
                    error::INVALID_PARAMS,
                    format!("model {model} (provider {provider}) does not support realtime"),
                ),
                LiveOpenPrecheckError::ProviderHasNoLiveAdapter { provider } => (
                    error::INTERNAL_ERROR,
                    format!("provider {provider} has no live adapter wired in this build"),
                ),
                // SessionLookup at this point is unexpected — B17 already
                // rejected missing sessions. Surface as INTERNAL_ERROR.
                LiveOpenPrecheckError::SessionLookup { .. } => {
                    (error::INTERNAL_ERROR, precheck_err.to_string())
                }
            };
            return RpcResponse::error(id, code, message);
        }

        let open_config = match runtime
            .live_open_config_for_session(&session_id, RealtimeTurningMode::ProviderManaged)
            .await
        {
            Ok(config) => config,
            Err(err) => {
                let _ = host.close_channel(&channel_id).await;
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("failed to build session config: {err}"),
                );
            }
        };
        match factory.open_session(&open_config).await {
            Ok(session) => {
                let adapter =
                    std::sync::Arc::new(meerkat_live::ProviderSessionAdapter::new(session));
                if let Err(err) = host.attach_adapter(&channel_id, adapter).await {
                    let _ = host.close_channel(&channel_id).await;
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("failed to attach adapter: {err}"),
                    );
                }
            }
            Err(err) => {
                let _ = host.close_channel(&channel_id).await;
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("failed to open provider session: {err}"),
                );
            }
        }
    }

    // B16: with router-level refusal of `live/*` when `with_live_ws` was not
    // called, both `live_ws` and `live_ws_base_url` are guaranteed `Some` by
    // the time the handler runs. The previous empty-URL/token fallback was
    // dead code that masked the unreachable case as a falsehood; treat it as
    // an internal invariant violation instead.
    let (ws_state, base_url) = match (live_ws, live_ws_base_url) {
        (Some(ws), Some(url)) => (ws, url),
        _ => {
            let _ = host.close_channel(&channel_id).await;
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                "live/open reached handler without live WS state; router gate is broken"
                    .to_string(),
            );
        }
    };
    let token = ws_state.mint_token(channel_id.clone()).await;
    let token_str = token.to_string();
    // G38: pin the bearer token to the channel via a `channel` query param so
    // a leaked token cannot be replayed against a different channel. The WS
    // upgrade handler validates equality via `LiveWsState::consume_token`.
    // Channel ids are URL-safe (G41: v4 UUID) so direct interpolation is
    // sound; if a future channel-id shape ever introduces non-URL-safe bytes
    // this `format!` must percent-encode.
    let transport = LiveTransportBootstrap::Websocket {
        url: format!(
            "{base_url}{path}?token={token_str}&channel={channel_id}",
            path = meerkat_live::LIVE_WS_PATH
        ),
        token: token_str,
    };

    // C21: do not advertise the static `LiveChannelCapabilities::default()`
    // (which lies "everything is supported"). Until the adapter exposes its
    // real capability set, return an all-`false` placeholder so consumers do
    // not assume features that may not exist.
    // TODO(C21): wire from adapter via `host.channel_capabilities(&channel_id)`
    // once the runtime agent adds that method.
    //
    // C22: do not unconditionally claim `Fresh` — until
    // `live_open_config_for_session` returns continuity metadata, return
    // `Degraded` rather than a falsehood.
    // TODO(C22): plumb continuity from `live_open_config_for_session`
    // (surface agent owns `session_runtime.rs`).
    let result = LiveOpenResult {
        channel_id: channel_id.to_string(),
        transport,
        capabilities: unknown_capabilities(),
        continuity: LiveContinuityMode::Degraded,
    };

    // N75: `LiveOpenResult` is a fixed-shape struct of `Serialize`-clean
    // fields; serialization is effectively infallible. On the unexpected
    // path return a typed `INTERNAL_ERROR` rather than silently returning
    // `null` (the previous `unwrap_or_default()` antipattern) — the
    // workspace lint forbids `expect()` in production code, so map the
    // serde error explicitly.
    match serde_json::to_value(result) {
        Ok(value) => RpcResponse::success(id, value),
        Err(err) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("failed to serialize LiveOpenResult: {err}"),
        ),
    }
}

pub async fn handle_live_status(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    host: &LiveAdapterHost,
) -> RpcResponse {
    let parsed: LiveChannelParams = match super::parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);

    match host.channel_status(&channel_id).await {
        Ok(status) => {
            let result = LiveStatusResult {
                channel_id: parsed.channel_id,
                status,
            };
            // N75: see `handle_live_open` — same INTERNAL_ERROR fallback for
            // a serialization failure that should never happen in practice.
            match serde_json::to_value(result) {
                Ok(value) => RpcResponse::success(id, value),
                Err(err) => RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("failed to serialize LiveStatusResult: {err}"),
                ),
            }
        }
        Err(LiveAdapterHostError::ChannelNotFound(_)) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("channel {} not found", parsed.channel_id),
        ),
        Err(err) => RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
    }
}

pub async fn handle_live_close(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    host: &LiveAdapterHost,
) -> RpcResponse {
    let parsed: LiveChannelParams = match super::parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);

    match host.close_channel(&channel_id).await {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"closed": true})),
        Err(LiveAdapterHostError::ChannelNotFound(_)) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("channel {} not found", parsed.channel_id),
        ),
        Err(err) => RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
    }
}

/// `live/send_input` parameters.
///
/// **`BREAKING_LIVE_WIRE_FORMAT_V1`** (H48): the chunk previously appeared as
/// flattened sibling fields next to `channel_id`. It is now a nested object
/// under the `chunk` field. WS protocol clients that piggyback on this shape
/// must update accordingly.
/// Errors returned when admitting a wire-format input chunk.
///
/// D24: previously a malformed base64 audio payload silently decoded to a
/// zero-length chunk indistinguishable from real silence. Decode failures
/// now propagate as a typed error and the RPC handler returns
/// `INVALID_PARAMS` to the caller instead of swallowing the malformed input.
#[derive(Debug, thiserror::Error)]
pub enum LiveSendInputError {
    #[error("invalid base64 audio payload: {0}")]
    InvalidAudioBase64(base64::DecodeError),
}

/// Decode a wire-format input chunk into the core `LiveInputChunk` shape.
///
/// Handler-local conversion (the wire type lives in `meerkat-contracts`, the
/// core domain type lives in `meerkat-core::live_adapter`; the base64 decode
/// is the only step that needs to fail with a typed error and is therefore
/// kept here in the handler crate).
fn live_input_chunk_from_wire(
    wire: LiveInputChunkWire,
) -> Result<LiveInputChunk, LiveSendInputError> {
    match wire {
        LiveInputChunkWire::Audio {
            data,
            sample_rate_hz,
            channels,
        } => {
            use base64::Engine;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(&data)
                .map_err(LiveSendInputError::InvalidAudioBase64)?;
            Ok(LiveInputChunk::Audio {
                data: decoded,
                sample_rate_hz,
                channels,
            })
        }
        LiveInputChunkWire::Text { text } => Ok(LiveInputChunk::Text { text }),
    }
}

pub async fn handle_live_send_input(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    host: &LiveAdapterHost,
) -> RpcResponse {
    let parsed: LiveSendInputParams = match super::parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);
    let chunk = match live_input_chunk_from_wire(parsed.chunk) {
        Ok(chunk) => chunk,
        Err(err) => {
            return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string());
        }
    };

    match host.send_input(&channel_id, chunk).await {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"sent": true})),
        Err(LiveAdapterHostError::ChannelNotFound(_)) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("channel {} not found", parsed.channel_id),
        ),
        Err(LiveAdapterHostError::NoAdapter(_)) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("channel {} has no adapter attached", parsed.channel_id),
        ),
        Err(err) => RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
    }
}

/// I50: `live/commit_input` — flush any buffered uncommitted input on the
/// channel. Maps to `LiveAdapterCommand::CommitInput`.
pub async fn handle_live_commit_input(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    host: &LiveAdapterHost,
) -> RpcResponse {
    let parsed: LiveChannelParams = match super::parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);

    match host
        .send_command(&channel_id, LiveAdapterCommand::CommitInput)
        .await
    {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"committed": true})),
        Err(LiveAdapterHostError::ChannelNotFound(_)) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("channel {} not found", parsed.channel_id),
        ),
        Err(LiveAdapterHostError::NoAdapter(_)) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("channel {} has no adapter attached", parsed.channel_id),
        ),
        Err(err) => RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// A7 — public interrupt / truncate surface
// ---------------------------------------------------------------------------
//
// Barge-in is advertised on `LiveChannelCapabilities` but, prior to A7, was
// reachable only via provider-native VAD. `live/interrupt` and
// `live/truncate` give the caller an explicit handle.
//
// **Playback-cursor model.** There is no `live/playback_cursor` read API.
// Playback is fundamentally a *client* fact: the WS endpoint knows what it
// rendered, the host only knows what it sent. A server-side cursor would
// either count bytes sent (diverges from played by jitter buffers and
// end-of-stream silence trim) or pretend a provider counter exists where
// none does — both are dogma sins in the C21/C22 "no lies" family. The
// canonical seam is the `audio_played_ms` parameter on `live/truncate`:
// clients track playback locally and pass the cursor in when they want to
// truncate. No separate read endpoint is needed because the only consumer
// of "what's the cursor right now?" is the truncate caller, who already has
// the answer.

/// A7: `live/interrupt` — signal the adapter to interrupt the in-progress
/// assistant turn. Maps to `LiveAdapterCommand::Interrupt`.
pub async fn handle_live_interrupt(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    host: &LiveAdapterHost,
) -> RpcResponse {
    let parsed: LiveChannelParams = match super::parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);

    match host
        .send_command(&channel_id, LiveAdapterCommand::Interrupt)
        .await
    {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"interrupted": true})),
        Err(LiveAdapterHostError::ChannelNotFound(_)) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("channel {} not found", parsed.channel_id),
        ),
        Err(LiveAdapterHostError::NoAdapter(_)) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("channel {} has no adapter attached", parsed.channel_id),
        ),
        Err(err) => RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
    }
}

/// A7: `live/truncate` — truncate an assistant item at the given playback
/// cursor. Maps to `LiveAdapterCommand::TruncateAssistantOutput`.
pub async fn handle_live_truncate(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    host: &LiveAdapterHost,
) -> RpcResponse {
    let parsed: LiveTruncateParams = match super::parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Validate item_id is non-empty. content_index is `u32` and
    // audio_played_ms is `u64`, so the type system already rejects negatives
    // at deserialization (`>= 0` is satisfied by construction).
    if parsed.item_id.is_empty() {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            "item_id must be non-empty".to_string(),
        );
    }

    let channel_id = LiveChannelId::new(&parsed.channel_id);
    let command = LiveAdapterCommand::TruncateAssistantOutput {
        item_id: parsed.item_id.clone(),
        content_index: parsed.content_index,
        audio_played_ms: parsed.audio_played_ms,
    };

    match host.send_command(&channel_id, command).await {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"truncated": true})),
        Err(LiveAdapterHostError::ChannelNotFound(_)) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("channel {} not found", parsed.channel_id),
        ),
        Err(LiveAdapterHostError::NoAdapter(_)) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("channel {} has no adapter attached", parsed.channel_id),
        ),
        Err(err) => RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    //! H46 round-trip tests + D24 base64-decode error tests + H48 nested
    //! `chunk` shape tests for the `live/*` wire types.

    use super::*;
    use meerkat_core::live_adapter::{
        LiveAdapterStatus, LiveChannelCapabilities, LiveContinuityMode, LiveTransportBootstrap,
    };

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + serde::de::DeserializeOwned,
    {
        let s = serde_json::to_string(value).expect("serialize");
        serde_json::from_str(&s).expect("deserialize")
    }

    #[test]
    fn live_open_params_roundtrip() {
        let v = LiveOpenParams {
            session_id: "sess-123".into(),
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn live_open_result_roundtrip() {
        let v = LiveOpenResult {
            channel_id: "live_42".into(),
            transport: LiveTransportBootstrap::Websocket {
                url: "ws://x/y?token=t".into(),
                token: "t".into(),
            },
            capabilities: unknown_capabilities(),
            continuity: LiveContinuityMode::Degraded,
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn live_channel_params_roundtrip() {
        let v = LiveChannelParams {
            channel_id: "live_1".into(),
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn live_status_result_roundtrip_idle() {
        let v = LiveStatusResult {
            channel_id: "live_1".into(),
            status: LiveAdapterStatus::Idle,
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn live_status_result_roundtrip_ready() {
        let v = LiveStatusResult {
            channel_id: "live_1".into(),
            status: LiveAdapterStatus::Ready,
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn live_input_chunk_wire_text_roundtrip() {
        let v = LiveInputChunkWire::Text {
            text: "hello".into(),
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn live_input_chunk_wire_audio_roundtrip() {
        let v = LiveInputChunkWire::Audio {
            data: "AAAA".into(),
            sample_rate_hz: 24_000,
            channels: 1,
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn live_send_input_params_chunk_is_nested_not_flat() {
        // H48: chunk must be a nested object under `chunk`, not flattened
        // siblings of `channel_id`.
        let v = LiveSendInputParams {
            channel_id: "live_1".into(),
            chunk: LiveInputChunkWire::Text { text: "hi".into() },
        };
        let json: serde_json::Value = serde_json::to_value(&v).unwrap();
        assert!(json.get("chunk").is_some(), "chunk must be nested");
        assert!(
            json.get("kind").is_none(),
            "wire format must NOT flatten kind to top-level"
        );
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn into_chunk_audio_decodes_valid_base64() {
        let wire = LiveInputChunkWire::Audio {
            data: "AAEC".into(), // base64 for [0, 1, 2]
            sample_rate_hz: 24_000,
            channels: 1,
        };
        let chunk = live_input_chunk_from_wire(wire).expect("valid base64 should decode");
        match chunk {
            LiveInputChunk::Audio { data, .. } => assert_eq!(data, vec![0u8, 1, 2]),
            other => panic!("expected Audio, got {other:?}"),
        }
    }

    #[test]
    fn into_chunk_audio_rejects_invalid_base64() {
        // D24: malformed base64 must NOT silently produce empty PCM.
        let wire = LiveInputChunkWire::Audio {
            data: "!!!not-base64!!!".into(),
            sample_rate_hz: 24_000,
            channels: 1,
        };
        let err = live_input_chunk_from_wire(wire).expect_err("invalid base64 must error");
        assert!(matches!(err, LiveSendInputError::InvalidAudioBase64(_)));
    }

    #[test]
    fn into_chunk_text_passes_through() {
        let wire = LiveInputChunkWire::Text {
            text: "hello".into(),
        };
        let chunk = live_input_chunk_from_wire(wire).unwrap();
        match chunk {
            LiveInputChunk::Text { text } => assert_eq!(text, "hello"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn unknown_capabilities_is_all_false() {
        let caps = unknown_capabilities();
        // C21: an "unknown" adapter must not advertise any capability.
        assert_eq!(
            caps,
            LiveChannelCapabilities {
                audio_input: false,
                audio_output: false,
                text_input: false,
                text_output: false,
                barge_in: false,
                transcript: false,
                provider_native_resume: false,
            }
        );
    }

    // -- A7 wire round-trips --

    #[test]
    fn live_truncate_params_roundtrip() {
        let v = LiveTruncateParams {
            channel_id: "live_1".into(),
            item_id: "item_xyz".into(),
            content_index: 3,
            audio_played_ms: 1_234,
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn live_truncate_params_fields_are_top_level() {
        // A7 / Rule 14: no flatten — every field is a sibling of channel_id.
        let v = LiveTruncateParams {
            channel_id: "live_1".into(),
            item_id: "item_xyz".into(),
            content_index: 0,
            audio_played_ms: 0,
        };
        let json: serde_json::Value = serde_json::to_value(&v).unwrap();
        for key in ["channel_id", "item_id", "content_index", "audio_played_ms"] {
            assert!(
                json.get(key).is_some(),
                "expected top-level field {key} on LiveTruncateParams"
            );
        }
    }
}
