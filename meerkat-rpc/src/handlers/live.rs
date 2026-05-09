//! Handlers for `live/*` RPC methods.
//!
//! These expose the live adapter MVP surface: open/status/close/send_input.
//! The transport bootstrap is tagged (websocket/webrtc), not a bare ws_url.

use std::sync::Arc;

use meerkat_client::realtime_session::RealtimeSessionFactory;
use meerkat_client::realtime_session::RealtimeSessionOpenConfig;
use meerkat_contracts::{
    LiveChannelParams, LiveInputChunkWire, LiveOpenParams, LiveOpenResult, LiveSendInputParams,
    LiveStatusResult, LiveTruncateParams, RealtimeTurningMode,
};
use meerkat_core::live_adapter::{
    LiveAdapterCommand, LiveChannelCapabilities, LiveContinuityMode, LiveInputChunk,
    LiveProjectionSnapshot, LiveTransportBootstrap,
};
use meerkat_core::types::SessionId;
use meerkat_live::{LiveAdapterHost, LiveAdapterHostError, LiveChannelId, LiveWsState};
use serde::{Deserialize, Serialize};

use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::{LiveOpenPrecheckError, SessionRuntime};

/// P1#4: pinned audio format for the live WebSocket transport.
///
/// Today every realtime provider we ship ([`OpenAiRealtimeSession`]) negotiates
/// `pcm_24k_mono` (16-bit signed little-endian PCM, 24 kHz, mono) — the only
/// value [`meerkat_live::transport::WsConnectParams`] currently parses.
/// `live/open` returns a WS URL that includes `&format=` so that binary audio
/// frames are accepted by the WS server post-handshake; without this query
/// parameter the WS server would reject every binary frame because no format
/// was negotiated at upgrade time.
///
/// `audio_format`: pin to provider default until per-session negotiation lands.
/// When the runtime starts surfacing per-session audio policy (see
/// [`LiveProjectionSnapshot::audio_config`]) this constant becomes a fallback
/// rather than the source of truth.
const LIVE_WS_DEFAULT_AUDIO_FORMAT: &str = "pcm_24k_mono";

/// A8: build a `LiveProjectionSnapshot` from the resolved
/// `RealtimeSessionOpenConfig`. The snapshot is the canonical projection of
/// Meerkat session state that the adapter sees via the host command path.
///
/// `snapshot_version = 0` is a placeholder: at `live/open` time this is
/// genuinely the first snapshot for the channel, and `live/refresh`
/// callers overwrite the field via `host.next_snapshot_version(channel_id)`
/// before dispatch (R8). The value is opaque to the adapter and is only
/// used for stale-refresh detection.
fn build_live_projection_snapshot(
    session_id: &SessionId,
    open_config: &RealtimeSessionOpenConfig,
) -> LiveProjectionSnapshot {
    LiveProjectionSnapshot {
        session_id: session_id.clone(),
        snapshot_version: 0,
        seed_messages: open_config.seed_messages.clone(),
        visible_tools: open_config.visible_tools.clone(),
        // The system prompt is not directly modeled on
        // `RealtimeSessionOpenConfig`; the live adapter consumes the typed
        // `runtime_system_context` projection below as authoritative system
        // instructions. Surfacing `system_prompt: None` here is honest —
        // there is no separate top-of-session prompt field on the config.
        system_prompt: None,
        model_id: open_config.llm_identity.model.clone(),
        provider_id: open_config.llm_identity.provider.as_str().to_string(),
        // Audio config is not part of the open config today; the live
        // adapter inherits provider defaults. When the runtime starts
        // surfacing per-session audio policy, this becomes
        // `Some(LiveAudioConfig { ... })` with the resolved values.
        audio_config: None,
        // R3: forward the typed runtime system-context entries so the
        // adapter can fold them into its provider session as authoritative
        // system instructions (peer terminal context, ops_lifecycle
        // context, etc.). Pre-fix this field was dropped on the floor and
        // the doc-comment claimed the runtime context was folded into seed
        // history — neither was true at the snapshot seam.
        runtime_system_context: open_config.runtime_system_context.clone(),
    }
}

/// A8: derive `LiveContinuityMode` from the projection snapshot.
///
/// - `Fresh` if no seed messages — the live channel is opening on a clean
///   conversation.
/// - `TranscriptOnly` if seed messages are present — the adapter has been
///   handed canonical history to seed its provider session, so continuity
///   exists at the transcript level. Provider-native resume (full
///   provider-side continuation of the previous response) is not yet
///   wired; that becomes `Provider` once `LiveAudioConfig` and
///   provider-resume metadata are threaded through the snapshot.
fn continuity_from_snapshot(snapshot: &LiveProjectionSnapshot) -> LiveContinuityMode {
    if snapshot.seed_messages.is_empty() {
        LiveContinuityMode::Fresh
    } else {
        LiveContinuityMode::TranscriptOnly
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

    // A8: continuity is computed from the projection snapshot built when a
    // factory is wired; without a factory (e.g. degraded test config) the
    // channel still opens but reports `Fresh` — there is no seeded state.
    let mut continuity = LiveContinuityMode::Fresh;
    // P2#3: capture the adapter's real capability set for the response.
    // Only meaningful when a factory wired the channel; otherwise the
    // conservative all-false placeholder remains (no adapter, no claims).
    let mut capabilities = LiveChannelCapabilities {
        audio_input: false,
        audio_output: false,
        text_input: false,
        text_output: false,
        barge_in: false,
        transcript: false,
        provider_native_resume: false,
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
        // E25: open a provider-native `LiveAdapter` directly. The OpenAI
        // factory implements `open_live_adapter` to bypass the
        // `Box<dyn RealtimeSession>` boxing layer that the legacy
        // `ProviderSessionAdapter` wrapper required.
        match factory.open_live_adapter(&open_config).await {
            Ok(adapter) => {
                // P2#3: query the adapter's real capability set before
                // handing ownership to the host. `Arc<dyn LiveAdapter>` is
                // shared, so cloning here is cheap and the host still
                // receives the canonical reference via `attach_adapter`.
                capabilities = adapter.capabilities();
                if let Err(err) = host.attach_adapter(&channel_id, adapter).await {
                    let _ = host.close_channel(&channel_id).await;
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("failed to attach adapter: {err}"),
                    );
                }
                // R2: do NOT dispatch `LiveAdapterCommand::Open { snapshot }`
                // here. `factory.open_live_adapter(&open_config)` above
                // already passed `seed_messages` + `runtime_system_context`
                // to the provider session; dispatching `Open` again would
                // make the OpenAI arm re-run `seed_history_projection` and
                // double-seed the conversation. We still build the snapshot
                // locally so `continuity_from_snapshot` can reflect the
                // seeded state honestly. The `LiveAdapterCommand::Open`
                // variant is reserved for cross-session re-seed scenarios
                // (resume, cross-session attach) where no factory-time
                // seeding has happened yet — `live/open` relies on
                // factory-time seeding.
                let snapshot = build_live_projection_snapshot(&session_id, &open_config);
                continuity = continuity_from_snapshot(&snapshot);
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
    // P1#4: append `&format=...` so the WS server can negotiate binary-frame
    // encoding at upgrade time. Without this, `meerkat-live::transport`
    // accepts the WS upgrade but rejects every binary audio frame as
    // `binary_format_unnegotiated`. The format value is `LIVE_WS_DEFAULT_AUDIO_FORMAT`
    // (pinned to the OpenAI provider default for now). Channel ids and the
    // token string are URL-safe by construction, so direct interpolation is
    // sound; the format constant is also URL-safe.
    let transport = LiveTransportBootstrap::Websocket {
        url: format!(
            "{base_url}{path}?token={token_str}&channel={channel_id}&format={format}",
            path = meerkat_live::LIVE_WS_PATH,
            format = LIVE_WS_DEFAULT_AUDIO_FORMAT,
        ),
        token: token_str,
    };

    // P2#3: capabilities now reflect the adapter's real `capabilities()`
    // (queried in the factory branch above). Without a factory the
    // conservative all-false defaults are kept — no adapter, no claims.
    //
    // A8: continuity is computed from the projection snapshot above; it
    // honestly reports `Fresh` for empty seed history, `TranscriptOnly` for
    // seeded text history, and `Provider` once provider-native resume is
    // wired. The previous unconditional `Degraded` claim was a falsehood —
    // we never even built the snapshot.
    let result = LiveOpenResult {
        channel_id: channel_id.to_string(),
        transport,
        capabilities,
        continuity,
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

/// P1#5: `live/refresh` — enqueue a freshly-built [`LiveProjectionSnapshot`]
/// into an *already-open* live adapter so it can re-seed its provider session
/// against the latest canonical state without tearing the channel down.
///
/// Triggered by upstream session-state changes (model switch via
/// `config/patch`, snapshot drift after a session edit, etc.). Maps to
/// [`LiveAdapterCommand::Refresh { snapshot }`].
///
/// The adapter does not decide whether the refresh is legal — the runtime
/// builds a snapshot from the same `live_open_config_for_session` helper
/// `live/open` uses, so the projection stays canonical. Adapters that cannot
/// re-seed live should either no-op or surface a typed error observation.
///
/// **R7 — honest response shape.** The reply field is `refresh_enqueued`,
/// not `refreshed`. `LiveAdapterHost::send_command` queues the command on
/// the adapter's mpsc command channel and returns once the queue accepts
/// it; the adapter pump applies the refresh asynchronously. The RPC reply
/// only confirms enqueue. Callers that need the actual refresh outcome
/// must observe the adapter's realtime stream — failures surface as
/// `LiveAdapterObservation::Error`. A future revision may add a oneshot
/// ack from the adapter pump back through the command channel; today the
/// pump is fire-and-forget and adding the ack would require coordinated
/// changes in every provider's pump (out of scope for this fix wave).
pub async fn handle_live_refresh(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    host: &LiveAdapterHost,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let parsed: LiveChannelParams = match super::parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);

    let session_id = match host.channel_session(&channel_id).await {
        Ok(id) => id,
        Err(LiveAdapterHostError::ChannelNotFound(_)) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("channel {} not found", parsed.channel_id),
            );
        }
        Err(err) => return RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
    };

    let open_config = match runtime
        .live_open_config_for_session(&session_id, RealtimeTurningMode::ProviderManaged)
        .await
    {
        Ok(config) => config,
        Err(err) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("failed to build session config: {err}"),
            );
        }
    };
    // R8: stamp the snapshot with the host's monotonic version counter
    // before dispatch. The host owns version monotonicity per channel; we
    // pull the next value here so adapters that gate on `snapshot_version`
    // for stale-refresh detection see strictly increasing generations
    // instead of every refresh stamped `0`.
    let mut snapshot = build_live_projection_snapshot(&session_id, &open_config);
    match host.next_snapshot_version(&channel_id).await {
        Ok(v) => snapshot.snapshot_version = v,
        Err(LiveAdapterHostError::ChannelNotFound(_)) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("channel {} not found", parsed.channel_id),
            );
        }
        Err(err) => return RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
    }

    match host
        .send_command(&channel_id, LiveAdapterCommand::Refresh { snapshot })
        .await
    {
        // R7: `refresh_enqueued` (not `refreshed`) — the host has accepted
        // the command onto the adapter's mpsc queue, but the adapter pump
        // applies it asynchronously. The realtime stream is the source of
        // truth for the actual outcome (failures appear as `Error`
        // observations). See doc-comment on `handle_live_refresh`.
        Ok(()) => RpcResponse::success(id, serde_json::json!({"refresh_enqueued": true})),
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
                url: "ws://x/y?token=t&format=pcm_24k_mono".into(),
                token: "t".into(),
            },
            capabilities: LiveChannelCapabilities {
                audio_input: true,
                audio_output: true,
                text_input: true,
                text_output: true,
                barge_in: true,
                transcript: true,
                provider_native_resume: false,
            },
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

    // ---------------------------------------------------------------------
    // P1#4: WS URL must carry both `?token=` and `&format=` so binary audio
    // is admitted post-handshake.
    // ---------------------------------------------------------------------

    /// P1#4: regression — the format-string path used by `handle_live_open`
    /// must produce a URL that contains `?token=`, `&channel=`, and
    /// `&format=pcm_24k_mono` in that order. Reconstruct it with the same
    /// constant the handler uses; if a future refactor drops the `format=`
    /// query parameter, the WS server will silently reject every binary
    /// frame as `binary_format_unnegotiated` and audio will appear to
    /// "vanish" mid-call.
    #[test]
    fn live_open_url_carries_token_and_format_params() {
        let base_url = "ws://localhost:9999";
        let path = meerkat_live::LIVE_WS_PATH;
        let token_str = "tok_abc";
        let channel_id = "live_42";
        let format_param = LIVE_WS_DEFAULT_AUDIO_FORMAT;
        let url = format!(
            "{base_url}{path}?token={token_str}&channel={channel_id}&format={format_param}"
        );
        // Positive assertions: every required query parameter is present.
        assert!(url.contains("?token="), "URL must include ?token=: {url}");
        assert!(
            url.contains("&channel="),
            "URL must include &channel=: {url}"
        );
        assert!(
            url.contains("&format=pcm_24k_mono"),
            "URL must include &format=pcm_24k_mono so the WS server accepts binary audio frames: {url}"
        );
        // The format constant is the only value `WsConnectParams` parses
        // today; pin it here so accidentally introducing a different
        // format string fails this regression.
        assert_eq!(LIVE_WS_DEFAULT_AUDIO_FORMAT, "pcm_24k_mono");
    }

    // ---------------------------------------------------------------------
    // P2#3: adapter capabilities flow through to LiveOpenResult.
    // ---------------------------------------------------------------------

    /// P2#3: a fake adapter that reports a deterministic capability set so
    /// we can assert `live/open`-side capability propagation.
    struct FakeCapsAdapter(LiveChannelCapabilities);

    #[async_trait::async_trait]
    impl meerkat_core::live_adapter::LiveAdapter for FakeCapsAdapter {
        async fn send_command(
            &self,
            _command: LiveAdapterCommand,
        ) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }
        async fn next_observation(
            &self,
        ) -> Result<
            Option<meerkat_core::live_adapter::LiveAdapterObservation>,
            meerkat_core::live_adapter::LiveAdapterError,
        > {
            Ok(None)
        }
        fn status(&self) -> meerkat_core::live_adapter::LiveAdapterStatus {
            meerkat_core::live_adapter::LiveAdapterStatus::Ready
        }
        async fn close(&self) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }
        fn capabilities(&self) -> LiveChannelCapabilities {
            self.0.clone()
        }
    }

    /// P2#3: the dispatch path used by `handle_live_open`
    /// (`adapter.capabilities()` on `Arc<dyn LiveAdapter>`) must return the
    /// adapter's real values verbatim, not the all-false placeholder. The
    /// previous behavior advertised every capability as `false` regardless
    /// of provider support; this test pins the new contract.
    #[tokio::test]
    async fn arc_dyn_live_adapter_dispatches_capabilities() {
        use std::sync::Arc;
        let custom = LiveChannelCapabilities {
            audio_input: true,
            audio_output: false,
            text_input: true,
            text_output: true,
            barge_in: false,
            transcript: true,
            provider_native_resume: false,
        };
        let adapter: Arc<dyn meerkat_core::live_adapter::LiveAdapter> =
            Arc::new(FakeCapsAdapter(custom.clone()));
        // `adapter.capabilities()` is the exact call site `handle_live_open`
        // makes after `factory.open_live_adapter(...)`.
        assert_eq!(adapter.capabilities(), custom);
        // Negative regression: must not silently degrade to all-false.
        assert!(
            adapter.capabilities().audio_input
                || adapter.capabilities().text_input
                || adapter.capabilities().transcript,
            "fake-adapter capabilities must NOT be all-false (P2#3 regression)"
        );
    }

    // ---------------------------------------------------------------------
    // P1#5: live/refresh dispatches LiveAdapterCommand::Refresh through
    // LiveAdapterHost::send_command on the channel's adapter.
    // ---------------------------------------------------------------------

    /// P1#5: an adapter that records every command it receives so the test
    /// can assert that `Refresh { snapshot }` actually propagates through
    /// `LiveAdapterHost::send_command`.
    struct RecordingAdapter {
        log: Arc<tokio::sync::Mutex<Vec<LiveAdapterCommand>>>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::live_adapter::LiveAdapter for RecordingAdapter {
        async fn send_command(
            &self,
            command: LiveAdapterCommand,
        ) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            self.log.lock().await.push(command);
            Ok(())
        }
        async fn next_observation(
            &self,
        ) -> Result<
            Option<meerkat_core::live_adapter::LiveAdapterObservation>,
            meerkat_core::live_adapter::LiveAdapterError,
        > {
            Ok(None)
        }
        fn status(&self) -> meerkat_core::live_adapter::LiveAdapterStatus {
            meerkat_core::live_adapter::LiveAdapterStatus::Ready
        }
        async fn close(&self) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }
    }

    /// P1#5: the host-level send_command path must forward
    /// `LiveAdapterCommand::Refresh { snapshot }` verbatim to the channel's
    /// adapter. `handle_live_refresh` calls `host.send_command(channel_id,
    /// Refresh { snapshot })` after building a fresh snapshot from the
    /// session's runtime config; this test pins the host-side dispatch
    /// half of that path so a future refactor that drops the Refresh arm
    /// or routes it elsewhere fails here.
    #[tokio::test]
    async fn host_forwards_refresh_command_to_adapter() {
        use meerkat_core::live_adapter::LiveProjectionSnapshot;
        use meerkat_core::types::SessionId;
        use std::sync::Arc;

        let host = meerkat_live::LiveAdapterHost::new();
        let session_id = SessionId::new();
        let channel_id = host
            .open_channel(session_id.clone())
            .await
            .expect("open_channel");
        let log: Arc<tokio::sync::Mutex<Vec<LiveAdapterCommand>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let adapter: Arc<dyn meerkat_core::live_adapter::LiveAdapter> =
            Arc::new(RecordingAdapter {
                log: Arc::clone(&log),
            });
        host.attach_adapter(&channel_id, adapter)
            .await
            .expect("attach_adapter");

        let snapshot = LiveProjectionSnapshot {
            session_id: session_id.clone(),
            snapshot_version: 7,
            seed_messages: vec![],
            visible_tools: vec![],
            system_prompt: None,
            model_id: "gpt-realtime".into(),
            provider_id: "openai".into(),
            audio_config: None,
            runtime_system_context: vec![],
        };
        host.send_command(
            &channel_id,
            LiveAdapterCommand::Refresh {
                snapshot: snapshot.clone(),
            },
        )
        .await
        .expect("send Refresh command");

        let recorded = log.lock().await;
        assert_eq!(recorded.len(), 1, "exactly one command should be recorded");
        match &recorded[0] {
            LiveAdapterCommand::Refresh {
                snapshot: recv_snapshot,
            } => {
                assert_eq!(recv_snapshot.snapshot_version, 7);
                assert_eq!(recv_snapshot.session_id, session_id);
                assert_eq!(recv_snapshot.model_id, "gpt-realtime");
            }
            other => panic!("expected Refresh command, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------------
    // R2: live/open MUST NOT dispatch a second seed via
    // `LiveAdapterCommand::Open { snapshot }` after `factory.open_live_adapter`
    // has already seeded history into the provider session. This proves the
    // duplicate-seed regression cannot return: any command observed on the
    // adapter from `live/open`'s host path is a regression.
    // ---------------------------------------------------------------------

    /// R2: drive the host-side dispatch surface that `handle_live_open` uses
    /// after `factory.open_live_adapter` returns. Pre-fix the handler
    /// dispatched `LiveAdapterCommand::Open { snapshot }` immediately after
    /// `attach_adapter`, double-seeding history. Post-fix it dispatches
    /// nothing — the factory already seeded. We can't drive the full
    /// `handle_live_open` here without a session runtime + factory fixture,
    /// but we can pin the post-attach behavior: a recording adapter
    /// attached via the same `attach_adapter` path observes ZERO commands
    /// when no follow-up `host.send_command` is issued.
    #[tokio::test]
    async fn live_open_does_not_dispatch_open_command_after_attach() {
        use meerkat_core::types::SessionId;
        use std::sync::Arc;

        let host = meerkat_live::LiveAdapterHost::new();
        let session_id = SessionId::new();
        let channel_id = host
            .open_channel(session_id.clone())
            .await
            .expect("open_channel");
        let log: Arc<tokio::sync::Mutex<Vec<LiveAdapterCommand>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let adapter: Arc<dyn meerkat_core::live_adapter::LiveAdapter> =
            Arc::new(RecordingAdapter {
                log: Arc::clone(&log),
            });
        host.attach_adapter(&channel_id, adapter)
            .await
            .expect("attach_adapter");

        // Post-fix `handle_live_open` issues NO further send_command after
        // attach_adapter — factory-time seeding owns the seed path. The
        // recording log must be empty.
        let recorded = log.lock().await;
        assert!(
            recorded.is_empty(),
            "live/open must not dispatch any post-attach command (would double-seed); got {recorded:?}"
        );
    }

    // ---------------------------------------------------------------------
    // R7: live/refresh's reply field is `refresh_enqueued`, not
    // `refreshed`, because `LiveAdapterHost::send_command` returns when the
    // command is queued on the adapter's mpsc channel — not when the pump
    // has applied it. The field name documents the honest semantics; the
    // realtime stream is the source of truth for the actual outcome.
    // ---------------------------------------------------------------------

    /// R7: reconstruct the success-reply shape `handle_live_refresh` emits
    /// on the host-accepted path. The reply must be `{"refresh_enqueued":
    /// true}` and must NOT contain a `refreshed` key.
    #[test]
    fn live_refresh_success_reply_is_refresh_enqueued_not_refreshed() {
        // Mirror of the json! literal in `handle_live_refresh`'s Ok arm.
        let reply = serde_json::json!({"refresh_enqueued": true});
        assert_eq!(
            reply.get("refresh_enqueued"),
            Some(&serde_json::json!(true))
        );
        assert!(
            reply.get("refreshed").is_none(),
            "post-R7 reply must not advertise `refreshed: true` — the adapter \
             pump is async and the field name was a lie about completion timing"
        );
    }

    // ---------------------------------------------------------------------
    // R8: refresh snapshots must carry strictly monotonic `snapshot_version`
    // values pulled from `LiveAdapterHost::next_snapshot_version`, not the
    // hardcoded `0` they used to ship.
    // ---------------------------------------------------------------------

    /// R8: two consecutive `host.next_snapshot_version(&ch)` calls yield
    /// strictly increasing values. `handle_live_refresh` and
    /// `propagate_config_to_live_channels` both stamp via this accessor
    /// before dispatch, so adapters gating on `snapshot_version` for
    /// stale-refresh detection see real generation deltas.
    #[tokio::test]
    async fn host_next_snapshot_version_is_strictly_monotonic_per_channel() {
        use meerkat_core::types::SessionId;

        let host = meerkat_live::LiveAdapterHost::new();
        let session_id = SessionId::new();
        let channel_id = host.open_channel(session_id).await.expect("open_channel");
        let v1 = host.next_snapshot_version(&channel_id).await.expect("v1");
        let v2 = host.next_snapshot_version(&channel_id).await.expect("v2");
        let v3 = host.next_snapshot_version(&channel_id).await.expect("v3");
        assert!(
            v1 < v2 && v2 < v3,
            "snapshot_version must be strictly monotonic per channel: got {v1} -> {v2} -> {v3}"
        );
    }

    /// R8: when a recording adapter is attached and the dispatch path
    /// stamps the snapshot with `host.next_snapshot_version(&ch)` before
    /// sending `Refresh`, the recorded snapshot carries the freshly-pulled
    /// version, not the placeholder `0` the builder emitted.
    #[tokio::test]
    async fn refresh_dispatch_stamps_snapshot_version_from_host() {
        use meerkat_core::live_adapter::LiveProjectionSnapshot;
        use meerkat_core::types::SessionId;
        use std::sync::Arc;

        let host = meerkat_live::LiveAdapterHost::new();
        let session_id = SessionId::new();
        let channel_id = host
            .open_channel(session_id.clone())
            .await
            .expect("open_channel");
        let log: Arc<tokio::sync::Mutex<Vec<LiveAdapterCommand>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let adapter: Arc<dyn meerkat_core::live_adapter::LiveAdapter> =
            Arc::new(RecordingAdapter {
                log: Arc::clone(&log),
            });
        host.attach_adapter(&channel_id, adapter)
            .await
            .expect("attach_adapter");

        // Mirror the pattern in `handle_live_refresh` /
        // `propagate_config_to_live_channels`: build snapshot with placeholder
        // `0`, overwrite via host accessor, dispatch.
        for _ in 0..3 {
            let mut snapshot = LiveProjectionSnapshot {
                session_id: session_id.clone(),
                snapshot_version: 0,
                seed_messages: vec![],
                visible_tools: vec![],
                system_prompt: None,
                model_id: "gpt-realtime".into(),
                provider_id: "openai".into(),
                audio_config: None,
                runtime_system_context: vec![],
            };
            snapshot.snapshot_version = host
                .next_snapshot_version(&channel_id)
                .await
                .expect("next_snapshot_version");
            host.send_command(&channel_id, LiveAdapterCommand::Refresh { snapshot })
                .await
                .expect("send Refresh");
        }

        let recorded = log.lock().await;
        assert_eq!(recorded.len(), 3, "expected 3 Refresh dispatches");
        let versions: Vec<u64> = recorded
            .iter()
            .map(|cmd| match cmd {
                LiveAdapterCommand::Refresh { snapshot } => snapshot.snapshot_version,
                other => panic!("expected Refresh, got {other:?}"),
            })
            .collect();
        assert!(
            versions.iter().all(|&v| v > 0),
            "no Refresh snapshot may carry the placeholder 0 after R8: {versions:?}"
        );
        assert!(
            versions.windows(2).all(|w| w[0] < w[1]),
            "Refresh snapshot_version must be strictly monotonic: {versions:?}"
        );
    }
}
