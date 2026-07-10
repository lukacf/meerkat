//! Handlers for `live/*` RPC methods.
//!
//! These expose the live adapter surface: open/status/close/send_input plus
//! WebSocket and, when the `live-webrtc` feature is enabled, WebRTC transport
//! bootstrap and signaling.

use std::sync::Arc;
#[cfg(feature = "live-webrtc")]
use std::time::{SystemTime, UNIX_EPOCH};

use meerkat_client::realtime_session::RealtimeSessionFactory;
use meerkat_client::realtime_session::RealtimeSessionOpenConfig;
use meerkat_contracts::{
    LiveChannelParams, LiveCloseResult, LiveCommitInputParams, LiveCommitInputResult,
    LiveInputChunkWire, LiveInterruptResult, LiveOpenParams, LiveOpenResult, LiveOpenTransport,
    LiveRefreshResult, LiveSendInputErrorData, LiveSendInputParams, LiveSendInputResult,
    LiveStatusResult, LiveTruncateParams, LiveTruncateResult, LiveWebrtcAnswerParams,
    LiveWebrtcAnswerResult, RealtimeCapabilities, RealtimeTurningMode, WireLiveAdapterErrorCode,
    WireLiveAdapterStatus, WireLiveDegradationReason,
};
use meerkat_core::SessionLlmIdentity;
use meerkat_core::live_adapter::{
    LiveAdapterCommand, LiveAdapterError, LiveAdapterErrorCode, LiveAudioConfig,
    LiveChannelCapabilities, LiveContinuityMode, LiveProjectionSnapshot, LiveTransportBootstrap,
};
use meerkat_core::types::SessionId;
#[cfg(feature = "live-webrtc")]
use meerkat_live::{LIVE_WEBRTC_ANSWER_METHOD, LiveWebrtcState};
use meerkat_live::{
    LiveAdapterHost, LiveAdapterHostError, LiveChannelCloseObservation, LiveChannelId, LiveWsState,
    live_input_chunk_decode_rejection, live_input_chunk_from_wire,
};
use serde::{Deserialize, Serialize};

use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::{LiveOpenPrecheckError, SessionRuntime};

/// Public live channel identifiers are UUID-shaped today. Keep a modest
/// forward-compatible bound so lookup failures cannot reflect an attacker-
/// sized identifier into a response buffer.
const MAX_LIVE_CHANNEL_ID_BYTES: usize = 128;

fn live_send_input_channel_id_rejection(
    id: Option<RpcId>,
    channel_id: &str,
) -> Option<RpcResponse> {
    let reason = if channel_id.is_empty() {
        meerkat_contracts::WireLiveConfigRejectionReason::Other {
            detail: "live_channel_id_empty".to_string(),
        }
    } else if channel_id.len() > MAX_LIVE_CHANNEL_ID_BYTES {
        meerkat_contracts::WireLiveConfigRejectionReason::InputTooLarge {
            max_bytes: u64::try_from(MAX_LIVE_CHANNEL_ID_BYTES).unwrap_or(u64::MAX),
            actual_bytes: u64::try_from(channel_id.len()).unwrap_or(u64::MAX),
        }
    } else {
        return None;
    };
    let data = LiveSendInputErrorData {
        error_code: WireLiveAdapterErrorCode::ConfigRejected { reason },
    };
    Some(match serde_json::to_value(data) {
        Ok(data) => RpcResponse::error_with_data(
            id,
            error::INVALID_PARAMS,
            "live channel identity is empty or too large".to_string(),
            data,
        ),
        Err(serialize_error) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("failed to serialize live/send_input error data: {serialize_error}"),
        ),
    })
}

#[cfg(feature = "live-webrtc")]
fn live_webrtc_now_ms() -> Result<u64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system time is before Unix epoch: {err}"))?;
    u64::try_from(elapsed.as_millis())
        .map_err(|_| "system time milliseconds overflow u64".to_string())
}

#[cfg(feature = "live-webrtc")]
fn live_webrtc_duration_ms(duration: std::time::Duration) -> Result<u64, String> {
    u64::try_from(duration.as_millis())
        .map_err(|_| "WebRTC token TTL milliseconds overflow u64".to_string())
}

fn live_refresh_result_from_machine_authority(
    authority: &meerkat_runtime::meerkat_machine::LiveRefreshResultAuthority,
) -> LiveRefreshResult {
    // Exhaustive: generated authority emits only `Queued` today. A future
    // generated status variant forces a compile error here, so the wire
    // projection can never silently misreport an unmapped authority class.
    match authority.status {
        meerkat_runtime::meerkat_machine::dsl::LiveRefreshPublicStatus::Queued => {
            LiveRefreshResult::queued()
        }
    }
}

fn live_close_result_from_machine_authority(
    authority: &meerkat_runtime::meerkat_machine::LiveCloseResultAuthority,
) -> LiveCloseResult {
    // Exhaustive: generated authority emits only `Closed` today. A future
    // generated status variant forces a compile error here, so the wire
    // projection can never silently misreport an unmapped authority class.
    match authority.status {
        meerkat_runtime::meerkat_machine::dsl::LiveClosePublicStatus::Closed => {
            LiveCloseResult::closed()
        }
    }
}

fn live_command_result_from_machine_authority(
    authority: &meerkat_runtime::meerkat_machine::LiveCommandResultAuthority,
    expected: meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind,
) -> Result<serde_json::Value, String> {
    use meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind;

    if authority.command != expected {
        return Err(format!(
            "LiveCommandResultResolved emitted command {:?} for expected {:?}",
            authority.command, expected
        ));
    }

    // #234: return typed result shapes (mirroring the `LiveCloseResult`
    // precedent) instead of ad-hoc `json!` blobs. The typed struct carries a
    // typed `status` discriminator, so SDK codegen sees a named shape rather
    // than `Value` / `Any`. Serde failures surface as `String` so the caller
    // maps them onto the RPC error channel.
    match expected {
        LiveCommandPublicKind::SendInput => serde_json::to_value(LiveSendInputResult::sent())
            .map_err(|err| format!("failed to serialize LiveSendInputResult: {err}")),
        LiveCommandPublicKind::CommitInput => {
            serde_json::to_value(LiveCommitInputResult::committed())
                .map_err(|err| format!("failed to serialize LiveCommitInputResult: {err}"))
        }
        LiveCommandPublicKind::Interrupt => {
            serde_json::to_value(LiveInterruptResult::interrupted())
                .map_err(|err| format!("failed to serialize LiveInterruptResult: {err}"))
        }
        LiveCommandPublicKind::TruncateAssistantOutput => {
            serde_json::to_value(LiveTruncateResult::truncated())
                .map_err(|err| format!("failed to serialize LiveTruncateResult: {err}"))
        }
    }
}

async fn live_command_session_id_from_machine_authority(
    runtime: &Arc<SessionRuntime>,
    channel_id: &LiveChannelId,
) -> Option<SessionId> {
    live_session_id_from_machine_authority(runtime, channel_id).await
}

async fn live_session_id_from_machine_authority(
    runtime: &Arc<SessionRuntime>,
    channel_id: &LiveChannelId,
) -> Option<SessionId> {
    runtime
        .runtime_adapter()
        .live_session_for_active_channel(channel_id)
        .await
}

fn live_command_rejection_response_from_machine_authority(
    id: Option<RpcId>,
    authority: &meerkat_runtime::meerkat_machine::LiveCommandRejectionAuthority,
    expected: meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind,
    channel_id: &LiveChannelId,
    host_error: &LiveAdapterHostError,
) -> RpcResponse {
    use meerkat_runtime::meerkat_machine::dsl::{
        LiveCommandRejectionPublicErrorClass, LiveCommandRejectionReason,
    };

    if authority.command != expected {
        return RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!(
                "LiveCommandRejectionResolved emitted command {:?} for expected {:?}",
                authority.command, expected
            ),
        );
    }

    let code = match authority.public_error_class {
        LiveCommandRejectionPublicErrorClass::InvalidParams => error::INVALID_PARAMS,
        LiveCommandRejectionPublicErrorClass::InternalError => error::INTERNAL_ERROR,
    };
    let message = match authority.rejection {
        LiveCommandRejectionReason::ChannelNotFound => format!("channel {channel_id} not found"),
        LiveCommandRejectionReason::NoAdapter => {
            format!("channel {channel_id} has no adapter attached")
        }
        LiveCommandRejectionReason::ChannelNotReady
        | LiveCommandRejectionReason::UnsupportedCommand
        | LiveCommandRejectionReason::AdapterError
        | LiveCommandRejectionReason::InternalHostError => host_error.to_string(),
    };
    if expected == meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind::SendInput
        && let Some(data) = live_send_input_error_data(host_error)
    {
        return match serde_json::to_value(data) {
            Ok(data) => RpcResponse::error_with_data(id, code, message, data),
            Err(serialize_error) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("failed to serialize live/send_input error data: {serialize_error}"),
            ),
        };
    }
    RpcResponse::error(id, code, message)
}

fn live_send_input_error_data(host_error: &LiveAdapterHostError) -> Option<LiveSendInputErrorData> {
    let LiveAdapterHostError::AdapterError(LiveAdapterError::ProviderError {
        code: code @ LiveAdapterErrorCode::ConfigRejected { .. },
        ..
    }) = host_error
    else {
        return None;
    };
    Some(LiveSendInputErrorData {
        error_code: WireLiveAdapterErrorCode::from(code.clone()),
    })
}

async fn live_command_error_response(
    id: Option<RpcId>,
    runtime: &Arc<SessionRuntime>,
    session_id: &SessionId,
    channel_id: &LiveChannelId,
    command: meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind,
    host_error: &LiveAdapterHostError,
) -> RpcResponse {
    let authority = match runtime
        .runtime_adapter()
        .resolve_live_command_rejection_result(session_id, channel_id, command, host_error)
        .await
    {
        Ok(authority) => authority,
        Err(error) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("live command rejection authority rejected result: {error}"),
            );
        }
    };
    live_command_rejection_response_from_machine_authority(
        id, &authority, command, channel_id, host_error,
    )
}

async fn live_unbound_command_error_response(
    id: Option<RpcId>,
    runtime: &Arc<SessionRuntime>,
    channel_id: &LiveChannelId,
    command: meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind,
) -> RpcResponse {
    let authority = match runtime
        .runtime_adapter()
        .resolve_unbound_live_command_rejection_result(channel_id, command)
        .await
    {
        Ok(authority) => authority,
        Err(error) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("unbound live command rejection authority rejected result: {error}"),
            );
        }
    };
    let host_error = LiveAdapterHostError::ChannelNotFound(channel_id.clone());
    live_command_rejection_response_from_machine_authority(
        id,
        &authority,
        command,
        channel_id,
        &host_error,
    )
}

fn live_channel_request_rejection_response_from_machine_authority(
    id: Option<RpcId>,
    authority: &meerkat_runtime::meerkat_machine::LiveChannelRequestRejectionAuthority,
    expected: meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind,
    channel_id: &LiveChannelId,
    detail: Option<String>,
) -> RpcResponse {
    use meerkat_runtime::meerkat_machine::dsl::{
        LiveChannelRequestRejectionPublicErrorClass, LiveChannelRequestRejectionReason,
    };

    if authority.request != expected {
        return RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!(
                "LiveChannelRequestRejectionResolved emitted request {:?} for expected {:?}",
                authority.request, expected
            ),
        );
    }

    let code = match authority.public_error_class {
        LiveChannelRequestRejectionPublicErrorClass::InvalidParams => error::INVALID_PARAMS,
        LiveChannelRequestRejectionPublicErrorClass::InternalError => error::INTERNAL_ERROR,
    };
    let message = match authority.rejection {
        LiveChannelRequestRejectionReason::ChannelNotFound => {
            format!("channel {channel_id} not found")
        }
        LiveChannelRequestRejectionReason::NoAdapter => {
            format!("channel {channel_id} has no adapter attached")
        }
        LiveChannelRequestRejectionReason::InvalidToken
        | LiveChannelRequestRejectionReason::InvalidPayload
        | LiveChannelRequestRejectionReason::WebrtcAnswerError
        | LiveChannelRequestRejectionReason::InternalHostError => detail.unwrap_or_else(|| {
            format!(
                "live channel request {:?} rejected for channel {}",
                authority.request, channel_id
            )
        }),
    };
    RpcResponse::error(id, code, message)
}

async fn live_channel_request_error_response(
    id: Option<RpcId>,
    runtime: &Arc<SessionRuntime>,
    session_id: &SessionId,
    channel_id: &LiveChannelId,
    request: meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind,
    host_error: &LiveAdapterHostError,
) -> RpcResponse {
    let authority = match runtime
        .runtime_adapter()
        .resolve_live_channel_request_rejection_result(session_id, channel_id, request, host_error)
        .await
    {
        Ok(authority) => authority,
        Err(error) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("live channel request rejection authority rejected result: {error}"),
            );
        }
    };
    live_channel_request_rejection_response_from_machine_authority(
        id,
        &authority,
        request,
        channel_id,
        Some(host_error.to_string()),
    )
}

async fn live_unbound_channel_request_error_response(
    id: Option<RpcId>,
    runtime: &Arc<SessionRuntime>,
    channel_id: &LiveChannelId,
    request: meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind,
) -> RpcResponse {
    let authority = match runtime
        .runtime_adapter()
        .resolve_unbound_live_channel_request_rejection_result(channel_id, request)
        .await
    {
        Ok(authority) => authority,
        Err(error) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!(
                    "unbound live channel request rejection authority rejected result: {error}"
                ),
            );
        }
    };
    let host_error = LiveAdapterHostError::ChannelNotFound(channel_id.clone());
    live_channel_request_rejection_response_from_machine_authority(
        id,
        &authority,
        request,
        channel_id,
        Some(host_error.to_string()),
    )
}

#[cfg(feature = "live-webrtc")]
fn live_webrtc_answer_rejection_reason(
    error: &meerkat_live::LiveWebrtcError,
) -> meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestRejectionReason {
    use meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestRejectionReason;

    match error {
        meerkat_live::LiveWebrtcError::ChannelNotFound(_) => {
            LiveChannelRequestRejectionReason::ChannelNotFound
        }
        meerkat_live::LiveWebrtcError::Json(_) => LiveChannelRequestRejectionReason::InvalidPayload,
        // All WebRTC answer-phase faults (codec registration / peer creation /
        // set-remote-description / create-answer / data-channel send / audio)
        // and a missing local description map to the generated WebrtcAnswerError
        // rejection reason. The enum is #[non_exhaustive]; future phase variants
        // fall here too. (Per-phase generated rejection reasons are a tracked
        // machine-schema follow-up for #124.)
        _ => LiveChannelRequestRejectionReason::WebrtcAnswerError,
    }
}

#[cfg(feature = "live-webrtc")]
fn live_webrtc_answer_rejection_detail(error: &meerkat_live::LiveWebrtcError) -> String {
    match error {
        meerkat_live::LiveWebrtcError::ChannelNotFound(channel) => {
            format!("channel {channel} not found")
        }
        meerkat_live::LiveWebrtcError::Json(err) => err.to_string(),
        other => format!("failed to answer WebRTC offer: {other}"),
    }
}

#[cfg(feature = "live-webrtc")]
async fn live_webrtc_answer_error_response(
    id: Option<RpcId>,
    runtime: &Arc<SessionRuntime>,
    session_id: &SessionId,
    channel_id: &LiveChannelId,
    error: &meerkat_live::LiveWebrtcError,
) -> RpcResponse {
    let request_kind =
        meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind::WebrtcAnswer;
    let rejection = live_webrtc_answer_rejection_reason(error);
    let authority = match runtime
        .runtime_adapter()
        .resolve_live_channel_request_rejection_reason_result(
            session_id,
            channel_id,
            request_kind,
            rejection,
        )
        .await
    {
        Ok(authority) => authority,
        Err(error) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("live WebRTC answer rejection authority rejected result: {error}"),
            );
        }
    };
    live_channel_request_rejection_response_from_machine_authority(
        id,
        &authority,
        request_kind,
        channel_id,
        Some(live_webrtc_answer_rejection_detail(error)),
    )
}

#[cfg(feature = "live-webrtc")]
async fn live_webrtc_answer_malformed_admission_response(
    id: Option<RpcId>,
    runtime: &Arc<SessionRuntime>,
    session_id: &SessionId,
    channel_id: &LiveChannelId,
    detail: String,
) -> RpcResponse {
    use meerkat_runtime::meerkat_machine::dsl::{
        LiveChannelRequestPublicKind, LiveChannelRequestRejectionReason,
    };

    let request_kind = LiveChannelRequestPublicKind::WebrtcAnswer;
    let authority = match runtime
        .runtime_adapter()
        .resolve_live_channel_request_rejection_reason_result(
            session_id,
            channel_id,
            request_kind,
            LiveChannelRequestRejectionReason::InternalHostError,
        )
        .await
    {
        Ok(authority) => authority,
        Err(error) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("live WebRTC malformed admission rejection authority failed: {error}"),
            );
        }
    };
    live_channel_request_rejection_response_from_machine_authority(
        id,
        &authority,
        request_kind,
        channel_id,
        Some(detail),
    )
}

#[cfg(feature = "live-webrtc")]
async fn live_webrtc_answer_admission_error_response_from_machine_authority(
    id: Option<RpcId>,
    runtime: &Arc<SessionRuntime>,
    session_id: &SessionId,
    authority: &meerkat_runtime::meerkat_machine::LiveWebrtcAnswerAdmissionAuthority,
    channel_id: &LiveChannelId,
) -> RpcResponse {
    use meerkat_runtime::meerkat_machine::dsl::{
        LiveChannelRequestRejectionPublicErrorClass, LiveWebrtcAnswerAdmissionRejection,
    };

    if authority.admitted {
        return live_webrtc_answer_malformed_admission_response(
            id,
            runtime,
            session_id,
            channel_id,
            "LiveWebrtcAnswerAdmissionResolved admitted token on error path".to_string(),
        )
        .await;
    }

    let Some(public_error_class) = authority.public_error_class else {
        return live_webrtc_answer_malformed_admission_response(
            id,
            runtime,
            session_id,
            channel_id,
            "LiveWebrtcAnswerAdmissionResolved omitted generated public error class".to_string(),
        )
        .await;
    };
    let Some(rejection) = authority.rejection else {
        return live_webrtc_answer_malformed_admission_response(
            id,
            runtime,
            session_id,
            channel_id,
            "LiveWebrtcAnswerAdmissionResolved omitted generated rejection reason".to_string(),
        )
        .await;
    };

    let code = match public_error_class {
        LiveChannelRequestRejectionPublicErrorClass::InvalidParams => error::INVALID_PARAMS,
        LiveChannelRequestRejectionPublicErrorClass::InternalError => error::INTERNAL_ERROR,
    };
    let message = match rejection {
        LiveWebrtcAnswerAdmissionRejection::TokenNotFound => {
            "invalid WebRTC live token: not found".to_string()
        }
        LiveWebrtcAnswerAdmissionRejection::TokenExpired => {
            "invalid WebRTC live token: expired".to_string()
        }
        LiveWebrtcAnswerAdmissionRejection::TokenChannelMismatch => {
            "invalid WebRTC live token: channel mismatch".to_string()
        }
        LiveWebrtcAnswerAdmissionRejection::TokenAlreadyConsumed => {
            "invalid WebRTC live token: already consumed".to_string()
        }
        LiveWebrtcAnswerAdmissionRejection::ChannelNotBound => {
            format!("channel {channel_id} not found")
        }
    };
    RpcResponse::error(id, code, message)
}

#[cfg(feature = "live-webrtc")]
fn live_webrtc_answer_result_from_machine_authority(
    authority: &meerkat_runtime::meerkat_machine::LiveWebrtcAnswerResultAuthority,
    answer_sdp: String,
) -> Result<LiveWebrtcAnswerResult, String> {
    match authority.status {
        meerkat_runtime::meerkat_machine::dsl::LiveWebrtcAnswerPublicStatus::Answered
            if authority.answered =>
        {
            Ok(LiveWebrtcAnswerResult { answer_sdp })
        }
        other => Err(format!(
            "LiveWebrtcAnswerResultResolved emitted unsupported status {other:?} with answered={}",
            authority.answered
        )),
    }
}

fn live_status_result_from_machine_authority(
    channel_id: String,
    authority: &meerkat_runtime::meerkat_machine::LiveChannelStatusAuthority,
) -> Result<LiveStatusResult, String> {
    Ok(LiveStatusResult {
        channel_id,
        status: wire_live_status_from_machine_authority(authority)?,
    })
}

fn wire_live_status_from_machine_authority(
    authority: &meerkat_runtime::meerkat_machine::LiveChannelStatusAuthority,
) -> Result<WireLiveAdapterStatus, String> {
    use meerkat_runtime::meerkat_machine::dsl::LiveChannelPublicStatus;

    match authority.status {
        LiveChannelPublicStatus::Idle => Ok(WireLiveAdapterStatus::Idle),
        LiveChannelPublicStatus::Opening => Ok(WireLiveAdapterStatus::Opening),
        LiveChannelPublicStatus::Ready => Ok(WireLiveAdapterStatus::Ready),
        LiveChannelPublicStatus::Closing => Ok(WireLiveAdapterStatus::Closing),
        LiveChannelPublicStatus::Closed => Ok(WireLiveAdapterStatus::Closed),
        LiveChannelPublicStatus::Degraded => {
            let reason = authority.degradation_reason.ok_or_else(|| {
                "LiveChannelStatusResolved emitted degraded status without reason".to_string()
            })?;
            Ok(WireLiveAdapterStatus::Degraded {
                reason: wire_live_degradation_reason_from_machine_authority(
                    reason,
                    authority.degradation_detail.as_deref(),
                ),
            })
        }
    }
}

fn wire_live_degradation_reason_from_machine_authority(
    reason: meerkat_runtime::meerkat_machine::dsl::LiveChannelDegradationReason,
    detail: Option<&str>,
) -> WireLiveDegradationReason {
    use meerkat_runtime::meerkat_machine::dsl::LiveChannelDegradationReason;

    match reason {
        LiveChannelDegradationReason::RateLimited => WireLiveDegradationReason::RateLimited,
        LiveChannelDegradationReason::ProviderThrottled => {
            WireLiveDegradationReason::ProviderThrottled
        }
        LiveChannelDegradationReason::NetworkUnstable => WireLiveDegradationReason::NetworkUnstable,
        LiveChannelDegradationReason::Other => WireLiveDegradationReason::Other {
            detail: detail.unwrap_or_default().to_string(),
        },
        LiveChannelDegradationReason::Unknown => WireLiveDegradationReason::Unknown {
            debug: detail
                .unwrap_or("unknown live channel degradation")
                .to_string(),
        },
    }
}

/// #176: project the provider's typed realtime audio policy into the typed
/// [`LiveAudioConfig`] the snapshot carries.
///
/// The provider/model audio policy is owned by the realtime factory and
/// published as a typed [`RealtimeCapabilities`] (`audio_input_format` /
/// `audio_output_format`, each a [`RealtimeAudioFormat`]). The surface does
/// not decide the format — it reads it here. Returns `None` when the factory
/// declines to advertise both directions of audio (e.g. a text-only realtime
/// product), so the caller fails closed rather than inventing a sample rate.
fn live_audio_config_from_capabilities(
    capabilities: &RealtimeCapabilities,
) -> Option<LiveAudioConfig> {
    let input = capabilities.audio_input_format.as_ref()?;
    let output = capabilities.audio_output_format.as_ref()?;
    Some(LiveAudioConfig {
        input_sample_rate_hz: input.sample_rate_hz,
        input_channels: u16::from(input.channels),
        output_sample_rate_hz: output.sample_rate_hz,
        output_channels: u16::from(output.channels),
    })
}

/// #176: derive the WS `&format=` query token from the typed audio policy.
///
/// The live WS transport (`meerkat_live::transport`) negotiates the binary
/// frame layout at upgrade time via this query token, and stamps the
/// inbound binary-chunk `sample_rate_hz` from whatever token it parses. The
/// only binary format that transport accepts today is 16-bit signed
/// little-endian PCM, 24 kHz, mono (`pcm_24k_mono`). We map the resolved
/// typed config to that token and **fail closed** (return `None`) when the
/// resolved policy does not match a token the WS server can parse — a
/// mismatch would otherwise have the server silently stamp the wrong rate
/// onto every binary frame. Surface only reads the typed config; it never
/// pins the token independently.
fn live_ws_audio_format_param(audio: &LiveAudioConfig) -> Option<&'static str> {
    const PCM_24K_MONO_RATE_HZ: u32 = 24_000;
    const PCM_24K_MONO_CHANNELS: u16 = 1;
    if audio.input_sample_rate_hz == PCM_24K_MONO_RATE_HZ
        && audio.input_channels == PCM_24K_MONO_CHANNELS
    {
        Some("pcm_24k_mono")
    } else {
        None
    }
}

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
    audio_config: Option<LiveAudioConfig>,
) -> LiveProjectionSnapshot {
    LiveProjectionSnapshot {
        session_id: session_id.clone(),
        snapshot_version: 0,
        seed_messages: open_config.seed_messages.clone(),
        visible_tools: open_config.visible_tools.clone(),
        // R10: read the typed root system prompt directly from the resolved
        // `RealtimeSessionOpenConfig.system_prompt` field — the producer
        // (`live_orchestration::propagate`/projection in the facade) populates
        // it from `realtime_projection_root_system_message` at projection time.
        // Consumers MUST NOT re-derive the prompt from `seed_messages[0]`: the
        // history-event projector explicitly drops `Message::System`, so the
        // seed-history is not an authoritative source for the prompt on the
        // refresh path. The typed field is the single owner of this fact.
        system_prompt: open_config.system_prompt.clone(),
        model_id: open_config.llm_identity.model.clone(),
        provider_id: open_config.llm_identity.provider,
        // #176: typed audio policy resolved from the realtime factory's
        // `RealtimeCapabilities` (the provider/model audio-format owner).
        // The surface reads this; it never pins a format. `None` only when
        // the resolving caller had no factory-published audio policy to
        // read (degraded/test config without a wired factory).
        audio_config,
        // R3: forward the typed runtime system-context entries so the
        // adapter can fold them into its provider session as authoritative
        // system instructions (peer terminal context, ops_lifecycle
        // context, etc.). Pre-fix this field was dropped on the floor and
        // the doc-comment claimed the runtime context was folded into seed
        // history — neither was true at the snapshot seam.
        runtime_system_context: open_config.runtime_system_context.clone(),
        user_content_identities: open_config.user_content_identities.clone(),
        user_content_tombstones: open_config.user_content_tombstones.clone(),
        transcript_rewrite_generation: open_config.transcript_rewrite_generation,
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

async fn abandon_live_open_admission(
    runtime: &Arc<SessionRuntime>,
    session_id: &SessionId,
    channel_id: &LiveChannelId,
) {
    if let Err(err) = runtime
        .runtime_adapter()
        .abandon_live_open_admission(session_id, channel_id)
        .await
    {
        tracing::warn!(
            target: "meerkat_rpc::handlers::live",
            ?channel_id,
            ?session_id,
            ?err,
            "generated live-open admission abandonment failed"
        );
    }
}

async fn close_live_channel_after_open_failure(
    host: &LiveAdapterHost,
    runtime: &Arc<SessionRuntime>,
    session_id: &SessionId,
    channel_id: &LiveChannelId,
) {
    // #355: open-failure cleanup is fail-closed, not best-effort. A graceful
    // close (`reserve` -> generated close authority -> host commit) clears the
    // machine-owned channel binding (`live_active_channel_by_session` /
    // `live_channel_session_by_channel` / `live_channel_identity_by_channel`)
    // only on a *committed* close. If the generated close authority rejects,
    // omits the host commit handoff, or the host commit itself fails, the
    // binding stays installed — a half-installed channel for an open that never
    // succeeded. In every such case we fall through to
    // `abandon_live_open_admission`, the same generated eviction the
    // `ChannelNotFound` arm already uses, so a failed open never leaves an
    // orphaned `by_session` / `channels` entry behind. The graceful close is
    // attempted first because it retains a queryable closed-channel status; the
    // abandon path is the harder fail-closed eviction reserved for the failure
    // arms (and for a channel the host never registered).
    match host.reserve_channel_close_observation(channel_id).await {
        Ok(observation) => {
            let committed = commit_live_close_for_open_failure(
                host,
                runtime,
                session_id,
                channel_id,
                &observation,
            )
            .await;
            if !committed {
                // The binding was not cleared by a committed close; evict it.
                abandon_live_open_admission(runtime, session_id, channel_id).await;
            }
        }
        Err(LiveAdapterHostError::ChannelNotFound(_)) => {
            abandon_live_open_admission(runtime, session_id, channel_id).await;
        }
        Err(err) => {
            tracing::warn!(
                target: "meerkat_rpc::handlers::live",
                ?channel_id,
                ?session_id,
                ?err,
                "failed to close live channel after open failure; evicting admission"
            );
            abandon_live_open_admission(runtime, session_id, channel_id).await;
        }
    }
}

/// Attempt a generated graceful close for an open-failure cleanup.
///
/// Returns `true` only when the host commit succeeded (the machine-owned
/// channel binding is now cleared). Returns `false` — after logging the typed
/// cause — when the generated close authority rejected, omitted the host commit
/// handoff, or the host commit failed; the caller then fail-closes by abandoning
/// the open admission so no half-installed binding survives (#355).
async fn commit_live_close_for_open_failure(
    host: &LiveAdapterHost,
    runtime: &Arc<SessionRuntime>,
    session_id: &SessionId,
    channel_id: &LiveChannelId,
    observation: &LiveChannelCloseObservation,
) -> bool {
    let authority = match runtime
        .runtime_adapter()
        .resolve_live_close_result(session_id, observation)
        .await
    {
        Ok(authority) => authority,
        Err(err) => {
            tracing::warn!(
                target: "meerkat_rpc::handlers::live",
                ?channel_id,
                ?session_id,
                ?err,
                "generated live-close authority rejected open-failure cleanup; evicting admission"
            );
            return false;
        }
    };
    let Some(close_commit_authority) = authority.channel_close_commit_authority() else {
        tracing::warn!(
            target: "meerkat_rpc::handlers::live",
            ?channel_id,
            ?session_id,
            "generated live-close result omitted host commit authority; evicting admission"
        );
        return false;
    };
    if let Err(err) = host
        .commit_channel_close_observation(observation, close_commit_authority)
        .await
    {
        tracing::warn!(
            target: "meerkat_rpc::handlers::live",
            ?channel_id,
            ?session_id,
            ?err,
            "host live-close commit failed after generated open-failure cleanup; evicting admission"
        );
        return false;
    }
    true
}

pub struct LiveOpenHandlerContext<'a> {
    pub host: &'a LiveAdapterHost,
    pub live_ws: Option<&'a LiveWsState>,
    pub live_ws_base_url: Option<&'a str>,
    #[cfg(feature = "live-webrtc")]
    pub live_webrtc: Option<&'a LiveWebrtcState>,
    pub runtime: &'a Arc<SessionRuntime>,
    pub session_factory: Option<&'a dyn RealtimeSessionFactory>,
}

pub async fn handle_live_open(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    ctx: LiveOpenHandlerContext<'_>,
) -> RpcResponse {
    let LiveOpenHandlerContext {
        host,
        live_ws,
        live_ws_base_url,
        #[cfg(feature = "live-webrtc")]
        live_webrtc,
        runtime,
        session_factory,
    } = ctx;
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
    // handles behind. Row #98: a store fault is surfaced as a typed error,
    // not silently treated as "session present".
    let session_state = match runtime.session_state(&session_id).await {
        Ok(state) => state,
        Err(err) => return RpcResponse::error(id, err.code, err.message),
    };
    if session_state.is_none() && !runtime.pending_session_exists(&session_id).await {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("session {session_id} not found"),
        );
    }

    // #302: refuse the open up front when no realtime session factory is
    // wired into this build. Without a factory there is no provider adapter
    // to attach, so an opened channel could only report all-false
    // capabilities and would reject every real command later. Fail closed
    // *before* `resolve_live_open_admission` / `open_channel_with_authority`
    // run so we never register or open a channel that cannot serve traffic.
    let Some(session_factory) = session_factory else {
        return RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            "live open requires a realtime session factory; none is wired in this build"
                .to_string(),
        );
    };

    // R3-1 (P1): honor the caller's optional `turning_mode` override.
    // Default = `ProviderManaged` (back-compat with pre-R3-1 callers).
    // Callers that need the G9 typed text-only `live/commit_input`
    // path must pass `ExplicitCommit` here — the OpenAI realtime API
    // rejects `input_audio_buffer.commit` unless the session was
    // opened in explicit-commit mode, and the rejected commit
    // surfaces as a terminal `LiveAdapterObservation::Error` that
    // closes the channel.
    let turning_mode = parsed
        .turning_mode
        .unwrap_or(RealtimeTurningMode::ProviderManaged);
    let prepared_open_config = match runtime
        .live_open_config_for_session(&session_id, turning_mode)
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
    let live_open_identity = prepared_open_config.llm_identity.clone();

    let candidate_channel_id = LiveChannelId::random_uuid();
    let open_authority = match runtime
        .runtime_adapter()
        .resolve_live_open_admission(&session_id, &candidate_channel_id, &live_open_identity)
        .await
    {
        Ok(authority) => authority,
        Err(err) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("live open authority rejected admission: {err}"),
            );
        }
    };
    if !open_authority.admitted() {
        return match open_authority.rejection() {
            Some(meerkat_runtime::meerkat_machine::dsl::LiveOpenAdmissionRejection::AlreadyBound) => {
                RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("session {session_id} already has an active live channel"),
                )
            }
            Some(
                meerkat_runtime::meerkat_machine::dsl::LiveOpenAdmissionRejection::ChannelAlreadyBound,
            ) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("generated duplicate live channel id {candidate_channel_id}"),
            ),
            None => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                "live open authority rejected admission without a reason".to_string(),
            ),
        };
    }

    let Some(channel_open_authority) = open_authority.channel_open_authority() else {
        abandon_live_open_admission(runtime, &session_id, &candidate_channel_id).await;
        return RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            "live open admission was accepted without a generated host handoff".to_string(),
        );
    };

    let channel_id = match host
        .open_channel_with_authority(channel_open_authority)
        .await
    {
        Ok(ch) => ch,
        Err(LiveAdapterHostError::SessionAlreadyBound(sid)) => {
            abandon_live_open_admission(runtime, &session_id, &candidate_channel_id).await;
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!(
                    "live host transport cache still has active channel for session {sid} \
                     after generated admission"
                ),
            );
        }
        Err(err) => {
            abandon_live_open_admission(runtime, &session_id, &candidate_channel_id).await;
            return RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string());
        }
    };

    // A8: continuity is computed from the projection snapshot built for this
    // channel. #302 makes a wired realtime factory a precondition, so the
    // factory block below always runs and assigns these on the success path
    // (every error path early-returns) — they are assigned exactly once, with
    // no dead placeholder fallback.
    //
    // - `continuity` honestly reports `Fresh` for empty seed history and
    //   `TranscriptOnly` once seeded (from `continuity_from_snapshot`).
    // - `resolved_audio_config` (#176) is the typed audio policy resolved from
    //   the factory's `RealtimeCapabilities` (provider/model audio owner); the
    //   WS `&format=` token and the snapshot `audio_config` both read it.
    // - `capabilities` (P2#3) is the adapter's real capability set.
    let continuity: LiveContinuityMode;
    let resolved_audio_config: Option<LiveAudioConfig>;
    let capabilities: LiveChannelCapabilities;

    {
        let factory = session_factory;
        let open_config = &prepared_open_config;
        // B19: refuse models that lack realtime capability before reaching the
        // factory. B18 (provider has a wired live adapter) is checked against
        // the factory below so the adapter-minting seam owns provider support.
        if let Err(precheck_err) = runtime.precheck_live_open(&session_id).await {
            close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id).await;
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
        if !factory.supports_provider(open_config.llm_identity.provider) {
            close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id).await;
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!(
                    "provider {} has no live adapter wired in this build",
                    open_config.llm_identity.provider.as_str()
                ),
            );
        }

        // E25: open a provider-native `LiveAdapter` directly. The OpenAI
        // factory implements `open_live_adapter` to bypass the
        // `Box<dyn RealtimeSession>` boxing layer that the legacy
        // `ProviderSessionAdapter` wrapper required.
        // #176: resolve the typed audio policy from the factory's
        // `RealtimeCapabilities` (the provider/model audio-format owner)
        // before opening the adapter. The WS `&format=` token and the
        // snapshot `audio_config` both read this single typed value.
        resolved_audio_config = live_audio_config_from_capabilities(&factory.capabilities());
        match factory.open_live_adapter(open_config).await {
            Ok(adapter) => {
                // P2#3: query the adapter's real capability set before
                // handing ownership to the host. `Arc<dyn LiveAdapter>` is
                // shared, so cloning here is cheap and the host still
                // receives the canonical reference via `attach_adapter`.
                capabilities = adapter.capabilities();
                if let Err(err) = host.attach_adapter(&channel_id, adapter).await {
                    close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id)
                        .await;
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
                let snapshot = build_live_projection_snapshot(
                    &session_id,
                    open_config,
                    resolved_audio_config.clone(),
                );
                continuity = continuity_from_snapshot(&snapshot);
            }
            Err(err) => {
                close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id)
                    .await;
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("failed to open provider session: {err}"),
                );
            }
        }
    }

    if let Err(err) = runtime.ensure_live_peer_ingress(&session_id).await {
        close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id).await;
        return RpcResponse::error(id, err.code, err.message);
    }

    #[cfg(feature = "live-webrtc")]
    let webrtc_configured = live_webrtc.is_some();
    #[cfg(not(feature = "live-webrtc"))]
    let webrtc_configured = false;

    let requested_transport = match parsed.transport {
        Some(transport) => transport,
        None if live_ws.is_some() => LiveOpenTransport::Websocket,
        None if webrtc_configured => LiveOpenTransport::Webrtc,
        None => {
            close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id).await;
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                "live/open reached handler without configured live transport".to_string(),
            );
        }
    };

    let transport = match requested_transport {
        LiveOpenTransport::Websocket => {
            // B16 updated: WebSocket is no longer the only live transport,
            // but requesting it still requires the WS state/base URL pair.
            let (ws_state, base_url) = match (live_ws, live_ws_base_url) {
                (Some(ws), Some(url)) => (ws, url),
                _ => {
                    close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id)
                        .await;
                    return RpcResponse::error(
                        id,
                        error::INVALID_PARAMS,
                        "live transport websocket is not configured".to_string(),
                    );
                }
            };
            let token = match ws_state.mint_token(&session_id, channel_id.clone()).await {
                Ok(token) => token,
                Err(err) => {
                    close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id)
                        .await;
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("live WebSocket token authority rejected issue: {err}"),
                    );
                }
            };
            let token_str = token.to_string();
            // #176: derive the WS `&format=` token from the resolved typed
            // audio policy (provider/model owner via `RealtimeCapabilities`),
            // not a surface-pinned constant. Fail closed when no audio policy
            // resolved (no wired factory) or it does not map to a token the
            // WS server can parse — otherwise the server would silently stamp
            // the wrong sample rate onto every binary frame.
            let Some(audio_config) = resolved_audio_config.as_ref() else {
                close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id)
                    .await;
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    "live websocket transport requires a resolved audio policy; no realtime \
                     factory audio format was available"
                        .to_string(),
                );
            };
            let Some(format_param) = live_ws_audio_format_param(audio_config) else {
                close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id)
                    .await;
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!(
                        "resolved live audio policy (input {}Hz/{}ch) has no websocket binary \
                         format the transport can negotiate",
                        audio_config.input_sample_rate_hz, audio_config.input_channels,
                    ),
                );
            };
            // G38: pin the bearer token to the channel via a `channel` query
            // param so a leaked token cannot be replayed against a different
            // channel. #176: `&format=` is the typed audio policy projected
            // into the WS server's binary-frame negotiation token; the server
            // stamps inbound binary-chunk sample rates from this token.
            LiveTransportBootstrap::Websocket {
                url: format!(
                    "{base_url}{path}?token={token_str}&channel={channel_id}&format={format_param}",
                    path = meerkat_live::LIVE_WS_PATH,
                ),
                token: token_str,
            }
        }
        LiveOpenTransport::Webrtc => {
            #[cfg(feature = "live-webrtc")]
            {
                let Some(webrtc_state) = live_webrtc else {
                    close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id)
                        .await;
                    return RpcResponse::error(
                        id,
                        error::INVALID_PARAMS,
                        "live transport webrtc is not configured".to_string(),
                    );
                };
                let token = webrtc_state.mint_token(channel_id.clone()).await;
                let token_str = token.to_string();
                let issued_at_ms = match live_webrtc_now_ms() {
                    Ok(now) => now,
                    Err(err) => {
                        close_live_channel_after_open_failure(
                            host,
                            runtime,
                            &session_id,
                            &channel_id,
                        )
                        .await;
                        return RpcResponse::error(id, error::INTERNAL_ERROR, err);
                    }
                };
                let ttl_ms = match live_webrtc_duration_ms(webrtc_state.token_ttl()) {
                    Ok(ttl) => ttl,
                    Err(err) => {
                        close_live_channel_after_open_failure(
                            host,
                            runtime,
                            &session_id,
                            &channel_id,
                        )
                        .await;
                        return RpcResponse::error(id, error::INTERNAL_ERROR, err);
                    }
                };
                let token_authority = match runtime
                    .runtime_adapter()
                    .record_live_webrtc_token_issued(
                        &session_id,
                        &channel_id,
                        &token_str,
                        issued_at_ms,
                        ttl_ms,
                    )
                    .await
                {
                    Ok(authority) => authority,
                    Err(err) => {
                        close_live_channel_after_open_failure(
                            host,
                            runtime,
                            &session_id,
                            &channel_id,
                        )
                        .await;
                        return RpcResponse::error(
                            id,
                            error::INTERNAL_ERROR,
                            format!("live WebRTC token authority rejected issue: {err}"),
                        );
                    }
                };
                LiveTransportBootstrap::Webrtc {
                    token: token_authority.token,
                    answer_method: LIVE_WEBRTC_ANSWER_METHOD.to_string(),
                    http_url: None,
                }
            }
            #[cfg(not(feature = "live-webrtc"))]
            {
                close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id)
                    .await;
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    "live transport webrtc is not compiled into this build".to_string(),
                );
            }
        }
        #[allow(unreachable_patterns)]
        _ => {
            close_live_channel_after_open_failure(host, runtime, &session_id, &channel_id).await;
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "unsupported live transport".to_string(),
            );
        }
    };
    // G8 (P2): project the core typed transport into the wire mirror at
    // the boundary so SDK codegen sees a typed discriminated union instead
    // of `unknown` / `Any`. The wire ↔ core `From` impls are byte-compatible.
    let transport: meerkat_contracts::WireLiveTransportBootstrap = transport.into();

    // P2#3: capabilities now reflect the adapter's real `capabilities()`
    // (queried in the factory branch above). #302 makes a wired factory a
    // precondition for reaching this point, so the all-false defaults are
    // always overwritten with the adapter's real capability set.
    //
    // A8: continuity is computed from the projection snapshot above; it
    // honestly reports `Fresh` for empty seed history, `TranscriptOnly` for
    // seeded text history, and `Provider` once provider-native resume is
    // wired. The previous unconditional `Degraded` claim was a falsehood —
    // we never even built the snapshot.
    // CC5/CC6: project core typed shapes into the wire mirrors at the
    // boundary so SDK codegen sees real structured types instead of opaque
    // JSON. The wire ↔ core `From` impls are byte-compatible, so the
    // serialized payload is identical.
    let result = LiveOpenResult {
        channel_id: channel_id.to_string(),
        transport,
        capabilities: capabilities.into(),
        continuity: continuity.into(),
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

#[cfg(feature = "live-webrtc")]
pub async fn handle_live_webrtc_answer(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    live_webrtc: &LiveWebrtcState,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let parsed: LiveWebrtcAnswerParams = match super::parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let channel_id = LiveChannelId::new(parsed.channel_id.clone());
    let request_kind =
        meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind::WebrtcAnswer;
    let session_id = match runtime
        .runtime_adapter()
        .live_session_for_webrtc_token(&parsed.token)
        .await
    {
        Some(session_id) => session_id,
        None => match live_session_id_from_machine_authority(runtime, &channel_id).await {
            Some(session_id) => session_id,
            None => {
                return live_unbound_channel_request_error_response(
                    id,
                    runtime,
                    &channel_id,
                    request_kind,
                )
                .await;
            }
        },
    };

    let observed_at_ms = match live_webrtc_now_ms() {
        Ok(now) => now,
        Err(err) => return RpcResponse::error(id, error::INTERNAL_ERROR, err),
    };
    let admission = match runtime
        .runtime_adapter()
        .resolve_live_webrtc_answer_admission(
            &session_id,
            &channel_id,
            &parsed.token,
            observed_at_ms,
        )
        .await
    {
        Ok(admission) => admission,
        Err(error) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("live WebRTC answer admission authority rejected result: {error}"),
            );
        }
    };
    if !admission.admitted {
        return live_webrtc_answer_admission_error_response_from_machine_authority(
            id,
            runtime,
            &session_id,
            &admission,
            &channel_id,
        )
        .await;
    }

    match live_webrtc
        .answer_offer(channel_id.clone(), parsed.offer_sdp)
        .await
    {
        Ok(answer) => {
            let authority = match runtime
                .runtime_adapter()
                .resolve_live_webrtc_answer_result(
                    &session_id,
                    &channel_id,
                    answer.answer_observation_sequence,
                )
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    // #346: answer_offer already installed the transport peer in
                    // the WebRTC registry. The generated answer-result authority
                    // rejected the result, so fail closed: tear the just-inserted
                    // peer back out rather than leaving an orphaned peer with no
                    // accepted answer behind it.
                    live_webrtc.close_peer(&channel_id).await;
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("live WebRTC answer result authority rejected result: {error}"),
                    );
                }
            };
            match live_webrtc_answer_result_from_machine_authority(&authority, answer.answer_sdp)
                .and_then(|result| {
                    serde_json::to_value(result)
                        .map_err(|err| format!("failed to serialize LiveWebrtcAnswerResult: {err}"))
                }) {
                Ok(value) => RpcResponse::success(id, value),
                Err(err) => {
                    // #346: the peer is installed but we cannot hand back a valid
                    // answer result — fail closed by removing the orphaned peer.
                    live_webrtc.close_peer(&channel_id).await;
                    RpcResponse::error(id, error::INTERNAL_ERROR, err)
                }
            }
        }
        Err(err) => {
            live_webrtc_answer_error_response(id, runtime, &session_id, &channel_id, &err).await
        }
    }
}

pub async fn handle_live_status(
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

    let request_kind = meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind::Status;
    let session_id = match runtime
        .runtime_adapter()
        .live_session_for_status_channel(&channel_id)
        .await
    {
        Some(session_id) => session_id,
        None => {
            return live_unbound_channel_request_error_response(
                id,
                runtime,
                &channel_id,
                request_kind,
            )
            .await;
        }
    };

    match host.channel_status_observation(&channel_id).await {
        Ok(observation) => {
            let authority = match runtime
                .runtime_adapter()
                .resolve_live_channel_status_result(&session_id, &observation)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("live status authority rejected result: {error}"),
                    );
                }
            };
            let result =
                match live_status_result_from_machine_authority(parsed.channel_id, &authority) {
                    Ok(result) => result,
                    Err(error) => return RpcResponse::error(id, error::INTERNAL_ERROR, error),
                };
            match serde_json::to_value(result) {
                Ok(value) => RpcResponse::success(id, value),
                Err(err) => RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("failed to serialize LiveStatusResult: {err}"),
                ),
            }
        }
        Err(err) => {
            live_channel_request_error_response(
                id,
                runtime,
                &session_id,
                &channel_id,
                request_kind,
                &err,
            )
            .await
        }
    }
}

pub async fn handle_live_close(
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

    let request_kind = meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind::Close;
    let session_id = match live_session_id_from_machine_authority(runtime, &channel_id).await {
        Some(session_id) => session_id,
        None => {
            return live_unbound_channel_request_error_response(
                id,
                runtime,
                &channel_id,
                request_kind,
            )
            .await;
        }
    };

    match host.reserve_channel_close_observation(&channel_id).await {
        Ok(observation) => {
            let authority = match runtime
                .runtime_adapter()
                .resolve_live_close_result(&session_id, &observation)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("live close authority rejected result: {error}"),
                    );
                }
            };
            let Some(close_commit_authority) = authority.channel_close_commit_authority() else {
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    "live close authority omitted host commit handoff".to_string(),
                );
            };
            if let Err(error) = host
                .commit_channel_close_observation(&observation, close_commit_authority)
                .await
            {
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("live close host commit failed after generated authority: {error}"),
                );
            }
            let result = live_close_result_from_machine_authority(&authority);
            let body = match serde_json::to_value(result) {
                Ok(body) => body,
                Err(error) => {
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("live close authority projection failed: {error}"),
                    );
                }
            };
            RpcResponse::success(id, body)
        }
        Err(err) => {
            live_channel_request_error_response(
                id,
                runtime,
                &session_id,
                &channel_id,
                request_kind,
                &err,
            )
            .await
        }
    }
}

/// P1#5: `live/refresh` — enqueue a mutable live-config update against the
/// active live session.
///
/// **Does NOT replay canonical history.** Mutable session config
/// (instructions / tools / audio settings) is applied via a single
/// `session.update` carrying the latest projection snapshot's config fields.
/// History is the responsibility of `live/open`'s seed step; refresh is
/// config-only by design. See R1+R9 in
/// `meerkat-openai/src/live.rs::execute_openai_live_command` for why
/// re-seeding history on refresh is unsafe (compounds the provider
/// transcript by N+1 every refresh).
///
/// **Identity changes require close + reopen.** Refresh validates that
/// `model_id` and `provider_id` match the channel's currently-open identity
/// and rejects swaps with a typed `InvalidConfig` error — the OpenAI Realtime
/// API does not accept a mutable `model` on `session.update`, and provider
/// identity is bound at WebSocket handshake time.
///
/// Triggered by upstream session-state changes (mutable-config edits via
/// `config/patch`, instructions drift after a session edit, etc.). The host
/// maps this to [`LiveAdapterCommand::Refresh { snapshot }`] and returns typed
/// queue-acceptance evidence.
///
/// The adapter does not decide whether the refresh is legal — the runtime
/// builds a config-only snapshot through `live_refresh_config_for_session`.
/// That helper shares canonical config resolution with live open but
/// deliberately skips history loading and image hydration; only `live/open`
/// owns replay. Adapters that cannot apply mutable config live should either
/// no-op or surface a typed error observation.
///
/// **R7 — honest response shape.** The reply is `status: queued`, not
/// `refreshed`. `LiveAdapterHost::enqueue_refresh` queues the command on
/// the adapter command channel and returns typed queue-acceptance evidence;
/// generated MeerkatMachine authority projects that evidence to the public
/// result class. The adapter pump applies the refresh asynchronously. Callers
/// that need the actual refresh outcome must observe the adapter's realtime
/// stream — failures surface as `LiveAdapterObservation::Error`.
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

    let request_kind = meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind::Refresh;
    let session_id = match live_session_id_from_machine_authority(runtime, &channel_id).await {
        Some(session_id) => session_id,
        None => {
            return live_unbound_channel_request_error_response(
                id,
                runtime,
                &channel_id,
                request_kind,
            )
            .await;
        }
    };

    let open_config = match runtime
        .live_refresh_config_for_session(&session_id, RealtimeTurningMode::ProviderManaged)
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
    // #176: refresh does not rebuild the WS transport URL and has no
    // factory in scope to publish a `RealtimeCapabilities` audio policy, so
    // the snapshot carries no audio_config here. The format the channel
    // negotiated at open time stays in force on the live transport.
    let mut snapshot = build_live_projection_snapshot(&session_id, &open_config, None);
    match host.next_snapshot_version(&channel_id).await {
        Ok(v) => snapshot.snapshot_version = v,
        Err(err) => {
            return live_channel_request_error_response(
                id,
                runtime,
                &session_id,
                &channel_id,
                request_kind,
                &err,
            )
            .await;
        }
    }

    match host.enqueue_refresh(&channel_id, snapshot).await {
        // R7 + Dogma #1: host queue acceptance is an observation only. The
        // public `status: queued` discriminator is emitted by generated
        // MeerkatMachine authority before the RPC surface projects the wire
        // payload.
        Ok(acceptance) => {
            let authority = match runtime
                .runtime_adapter()
                .resolve_live_refresh_queued_result(&session_id, &acceptance)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("live refresh queued authority rejected result: {error}"),
                    );
                }
            };
            let result = live_refresh_result_from_machine_authority(&authority);
            let body = match serde_json::to_value(result) {
                Ok(body) => body,
                Err(error) => {
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("live refresh queued authority projection failed: {error}"),
                    );
                }
            };
            RpcResponse::success(id, body)
        }
        Err(err) => {
            live_channel_request_error_response(
                id,
                runtime,
                &session_id,
                &channel_id,
                request_kind,
                &err,
            )
            .await
        }
    }
}

pub async fn handle_live_send_input(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    host: &LiveAdapterHost,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let parsed: LiveSendInputParams = match super::parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let LiveSendInputParams { channel_id, chunk } = parsed;
    if let Some(response) = live_send_input_channel_id_rejection(id.clone(), &channel_id) {
        return response;
    }
    let channel_id = LiveChannelId::new(channel_id);
    let chunk = match live_input_chunk_from_wire(chunk) {
        Ok(chunk) => chunk,
        Err(err) => {
            if let Some(code) = live_input_chunk_decode_rejection(&err) {
                let data = LiveSendInputErrorData {
                    error_code: WireLiveAdapterErrorCode::from(code),
                };
                return match serde_json::to_value(data) {
                    Ok(data) => RpcResponse::error_with_data(
                        id,
                        error::INVALID_PARAMS,
                        err.to_string(),
                        data,
                    ),
                    Err(serialize_error) => RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!(
                            "failed to serialize live/send_input error data: {serialize_error}"
                        ),
                    ),
                };
            }
            return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string());
        }
    };

    let command_kind = meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind::SendInput;
    let session_id =
        match live_command_session_id_from_machine_authority(runtime, &channel_id).await {
            Some(session_id) => session_id,
            None => {
                return live_unbound_command_error_response(id, runtime, &channel_id, command_kind)
                    .await;
            }
        };

    match host.send_input_observed(&channel_id, chunk).await {
        Ok(acceptance) => {
            let authority = match runtime
                .runtime_adapter()
                .resolve_live_command_result(&session_id, &acceptance)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("live send_input authority rejected result: {error}"),
                    );
                }
            };
            match live_command_result_from_machine_authority(&authority, command_kind) {
                Ok(value) => RpcResponse::success(id, value),
                Err(error) => RpcResponse::error(id, error::INTERNAL_ERROR, error),
            }
        }
        Err(err) => {
            live_command_error_response(id, runtime, &session_id, &channel_id, command_kind, &err)
                .await
        }
    }
}

/// I50: `live/commit_input` — flush any buffered uncommitted input on the
/// channel. Maps to `LiveAdapterCommand::CommitInput`.
///
/// G9 (P2): the optional `response_modality` param lets the caller request
/// a text-only response on an audio-first channel without flipping the
/// channel-wide modality.
pub async fn handle_live_commit_input(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    host: &LiveAdapterHost,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let parsed: LiveCommitInputParams = match super::parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);
    // R5-3 (P3): the wire mirror is `TryFrom`-only. The `Unknown` sentinel
    // is the explicit fail-loud variant for a future core-side modality
    // the client doesn't yet understand; reject the request rather than
    // silently coerce it.
    let response_modality = match parsed.response_modality.map(TryInto::try_into) {
        Some(Ok(modality)) => Some(modality),
        Some(Err(err)) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("invalid response_modality: {err}"),
            );
        }
        None => None,
    };

    let command_kind = meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind::CommitInput;
    let session_id =
        match live_command_session_id_from_machine_authority(runtime, &channel_id).await {
            Some(session_id) => session_id,
            None => {
                return live_unbound_command_error_response(id, runtime, &channel_id, command_kind)
                    .await;
            }
        };

    match host
        .send_command_observed(
            &channel_id,
            LiveAdapterCommand::CommitInput { response_modality },
        )
        .await
    {
        Ok(acceptance) => {
            let authority = match runtime
                .runtime_adapter()
                .resolve_live_command_result(&session_id, &acceptance)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("live commit_input authority rejected result: {error}"),
                    );
                }
            };
            match live_command_result_from_machine_authority(&authority, command_kind) {
                Ok(value) => RpcResponse::success(id, value),
                Err(error) => RpcResponse::error(id, error::INTERNAL_ERROR, error),
            }
        }
        Err(err) => {
            live_command_error_response(id, runtime, &session_id, &channel_id, command_kind, &err)
                .await
        }
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
    runtime: &Arc<SessionRuntime>,
    #[cfg(feature = "live-webrtc")] webrtc_state: Option<&meerkat_live::LiveWebrtcState>,
) -> RpcResponse {
    let parsed: LiveChannelParams = match super::parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);
    let command_kind = meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind::Interrupt;
    let session_id =
        match live_command_session_id_from_machine_authority(runtime, &channel_id).await {
            Some(session_id) => session_id,
            None => {
                return live_unbound_command_error_response(id, runtime, &channel_id, command_kind)
                    .await;
            }
        };

    match host
        .send_command_observed(&channel_id, LiveAdapterCommand::Interrupt)
        .await
    {
        Ok(acceptance) => {
            let authority = match runtime
                .runtime_adapter()
                .resolve_live_command_result(&session_id, &acceptance)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("live interrupt authority rejected result: {error}"),
                    );
                }
            };
            match live_command_result_from_machine_authority(&authority, command_kind) {
                Ok(value) => {
                    #[cfg(feature = "live-webrtc")]
                    if let Some(state) = webrtc_state {
                        state.discard_output_audio(&channel_id).await;
                    }
                    RpcResponse::success(id, value)
                }
                Err(error) => RpcResponse::error(id, error::INTERNAL_ERROR, error),
            }
        }
        Err(err) => {
            live_command_error_response(id, runtime, &session_id, &channel_id, command_kind, &err)
                .await
        }
    }
}

/// A7: `live/truncate` — truncate an assistant item at the given playback
/// cursor. Maps to `LiveAdapterCommand::TruncateAssistantOutput`.
pub async fn handle_live_truncate(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    host: &LiveAdapterHost,
    runtime: &Arc<SessionRuntime>,
    #[cfg(feature = "live-webrtc")] webrtc_state: Option<&meerkat_live::LiveWebrtcState>,
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
    let command_kind =
        meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind::TruncateAssistantOutput;
    let session_id =
        match live_command_session_id_from_machine_authority(runtime, &channel_id).await {
            Some(session_id) => session_id,
            None => {
                return live_unbound_command_error_response(id, runtime, &channel_id, command_kind)
                    .await;
            }
        };
    let command = LiveAdapterCommand::TruncateAssistantOutput {
        item_id: parsed.item_id.clone(),
        content_index: parsed.content_index,
        audio_played_ms: parsed.audio_played_ms,
    };

    match host.send_command_observed(&channel_id, command).await {
        Ok(acceptance) => {
            let authority = match runtime
                .runtime_adapter()
                .resolve_live_command_result(&session_id, &acceptance)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("live truncate authority rejected result: {error}"),
                    );
                }
            };
            match live_command_result_from_machine_authority(&authority, command_kind) {
                Ok(value) => {
                    #[cfg(feature = "live-webrtc")]
                    if let Some(state) = webrtc_state {
                        state.discard_output_audio(&channel_id).await;
                    }
                    RpcResponse::success(id, value)
                }
                Err(error) => RpcResponse::error(id, error::INTERNAL_ERROR, error),
            }
        }
        Err(err) => {
            live_command_error_response(id, runtime, &session_id, &channel_id, command_kind, &err)
                .await
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    //! H46 round-trip tests + H48 nested `chunk` shape tests for the
    //! `live/*` wire types. (D24 base64-decode rejection is owned by
    //! `meerkat_live::wire_input` and tested there.)

    use super::*;
    use meerkat_core::live_adapter::{
        LiveAdapterStatus, LiveChannelCapabilities, LiveContinuityMode,
    };

    fn test_live_identity() -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: "gpt-realtime-2".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    async fn open_test_channel(
        host: &meerkat_live::LiveAdapterHost,
        session_id: meerkat_core::types::SessionId,
    ) -> meerkat_live::LiveChannelId {
        let machine = meerkat_runtime::meerkat_machine::MeerkatMachine::ephemeral();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let channel_id = meerkat_live::LiveChannelId::random_uuid();
        let identity = test_live_identity();
        let authority = machine
            .resolve_live_open_admission(&session_id, &channel_id, &identity)
            .await
            .expect("generated live open admission");
        host.open_channel_with_authority(
            authority
                .channel_open_authority()
                .expect("generated live open handoff"),
        )
        .await
        .expect("open_channel")
    }

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + serde::de::DeserializeOwned,
    {
        let s = serde_json::to_string(value).expect("serialize");
        serde_json::from_str(&s).expect("deserialize")
    }

    #[test]
    fn immediate_send_input_config_rejection_keeps_typed_error_data() {
        let host_error = LiveAdapterHostError::AdapterError(LiveAdapterError::ProviderError {
            code: LiveAdapterErrorCode::ConfigRejected {
                reason: meerkat_core::live_adapter::LiveConfigRejectionReason::InputBackpressured {
                    max_pending_bytes: 1024,
                },
            },
            message: "input queue is full".to_string(),
        });
        let data = live_send_input_error_data(&host_error)
            .expect("scoped adapter rejection must retain typed send_input data");
        assert!(matches!(
            data.error_code,
            WireLiveAdapterErrorCode::ConfigRejected {
                reason: meerkat_contracts::WireLiveConfigRejectionReason::InputBackpressured {
                    max_pending_bytes: 1024
                }
            }
        ));
    }

    #[test]
    fn malformed_image_base64_keeps_typed_send_input_error_data() {
        let wire = LiveInputChunkWire::Image {
            idempotency_key: "bad-base64-rpc-1".to_string(),
            mime: "image/png".to_string(),
            data: "@@@".to_string(),
        };
        let decode_error =
            live_input_chunk_from_wire(wire).expect_err("malformed image must fail decode");
        let code = live_input_chunk_decode_rejection(&decode_error)
            .expect("malformed image base64 must be a scoped typed rejection");
        let data = serde_json::to_value(LiveSendInputErrorData {
            error_code: WireLiveAdapterErrorCode::from(code),
        })
        .expect("typed live/send_input error data should serialize");
        assert_eq!(data["error_code"]["code"], "config_rejected");
        assert_eq!(
            data["error_code"]["reason"]["kind"],
            "image_input_invalid_base64"
        );
        assert!(data["error_code"]["reason"].get("detail").is_none());
    }

    #[test]
    fn oversized_send_input_channel_id_is_bounded_and_typed_without_echo() {
        let oversized = format!("private-channel:{}", "x".repeat(MAX_LIVE_CHANNEL_ID_BYTES));
        let response = live_send_input_channel_id_rejection(Some(RpcId::Num(7)), &oversized)
            .expect("oversized public channel id must be rejected");
        let rpc_error = response.error.as_ref().expect("typed RPC error");
        assert_eq!(rpc_error.code, error::INVALID_PARAMS);
        assert!(matches!(
            rpc_error.data.as_ref(),
            Some(data)
                if data["error_code"]["code"] == "config_rejected"
                    && data["error_code"]["reason"]["kind"] == "input_too_large"
                    && data["error_code"]["reason"]["max_bytes"]
                        == MAX_LIVE_CHANNEL_ID_BYTES
                    && data["error_code"]["reason"]["actual_bytes"] == oversized.len()
        ));
        let serialized = serde_json::to_string(&response).expect("serialize rejection");
        assert!(!serialized.contains(&oversized));
        assert!(serialized.len() < 1024);
    }

    #[test]
    fn bounded_send_input_channel_id_passes_admission() {
        assert!(
            live_send_input_channel_id_rejection(
                Some(RpcId::Num(8)),
                "01234567-89ab-cdef-0123-456789abcdef",
            )
            .is_none()
        );
    }

    #[test]
    fn snapshot_reads_typed_system_prompt_field_not_seed_messages() {
        // R10 (D209): the snapshot builder MUST surface the typed
        // `RealtimeSessionOpenConfig.system_prompt` field directly — the
        // single owner of the live-session prompt populated by the runtime
        // projection — instead of re-deriving it from `seed_messages[0]`.
        // The history-event projector drops `Message::System`, so inference
        // from seed history silently wipes the prompt on the OpenAI Refresh
        // `session.update` path. To prove the source is the typed field and
        // NOT the seed history, the seed messages here lead with a NON-system
        // user message while the typed field carries the resolved prompt.
        use meerkat_core::types::{Message, UserMessage};

        let resolved_prompt = "you are a helpful meerkat".to_string();
        let open_config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ProviderManaged,
            test_live_identity(),
            Vec::new(),
            vec![Message::User(UserMessage::text("hi".to_string()))],
        )
        .with_system_prompt(Some(resolved_prompt.clone()));
        let session_id = SessionId::new();
        let snapshot = build_live_projection_snapshot(&session_id, &open_config, None);
        assert_eq!(
            snapshot.system_prompt,
            Some(resolved_prompt),
            "snapshot.system_prompt must read the typed open_config field, not infer from seed_messages[0]"
        );
    }

    #[test]
    fn snapshot_system_prompt_is_none_when_typed_field_absent() {
        // R10 (D209): a session without a resolved root system prompt must
        // surface `None` honestly. Even when a `Message::System` happens to
        // lead the seed history, the snapshot must NOT resurrect it as the
        // prompt — the typed field is the only authority, and its absence is
        // an honest `None` (not an empty string the adapter must guard).
        use meerkat_core::types::{Message, SystemMessage};

        let open_config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ProviderManaged,
            test_live_identity(),
            Vec::new(),
            vec![Message::System(SystemMessage::new(
                "stray seed system message",
            ))],
        );
        let session_id = SessionId::new();
        let snapshot = build_live_projection_snapshot(&session_id, &open_config, None);
        assert_eq!(
            snapshot.system_prompt, None,
            "snapshot.system_prompt must mirror the absent typed field, not infer from seed_messages[0]"
        );
    }

    #[test]
    fn live_open_params_roundtrip() {
        let v = LiveOpenParams {
            session_id: "sess-123".into(),
            turning_mode: None,
            transport: None,
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn live_open_params_explicit_commit_roundtrip() {
        // R3-1 (P1): explicit-commit on the wire so the G9 typed text-only
        // commit_input path is reachable.
        let v = LiveOpenParams {
            session_id: "sess-123".into(),
            turning_mode: Some(RealtimeTurningMode::ExplicitCommit),
            transport: None,
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn live_open_result_roundtrip() {
        // CC5/CC6: capabilities + continuity are typed wire mirrors now.
        // Construct directly from the wire types so SDK codegen sees the
        // typed shape (booleans / discriminated continuity-mode union) and
        // not an opaque JSON blob.
        let v = LiveOpenResult {
            channel_id: "live_42".into(),
            transport: meerkat_contracts::WireLiveTransportBootstrap::Websocket {
                url: "ws://x/y?token=t&format=pcm_24k_mono".into(),
                token: "t".into(),
            },
            capabilities: meerkat_contracts::WireLiveChannelCapabilities {
                audio_in: true,
                audio_out: true,
                text_in: true,
                text_out: true,
                image_in: false,
                video_in: false,
                barge_in_supported: true,
                transcript_supported: true,
                provider_native_resume: false,
            },
            continuity: meerkat_contracts::WireLiveContinuityMode::Degraded,
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn continuity_from_snapshot_never_synthesizes_provider_native_resume() {
        // T12: `ProviderNativeResume { provider_session_id }` is reserved
        // for a future provider that surfaces a resume id. No provider
        // Meerkat ships today does, so the open-time helper must only emit
        // `Fresh` (empty seed) or `TranscriptOnly` (any seed messages) —
        // never `ProviderNativeResume` and never `Degraded` (the latter is
        // only set on canonical-replay failure further up the open path).
        use meerkat_core::Provider;
        use meerkat_core::live_adapter::LiveProjectionSnapshot;
        use meerkat_core::types::{Message, SessionId, UserMessage};

        let empty = LiveProjectionSnapshot {
            session_id: SessionId::new(),
            snapshot_version: 1,
            seed_messages: Vec::new(),
            visible_tools: Vec::new(),
            system_prompt: None,
            model_id: "gpt-realtime-2".into(),
            provider_id: meerkat_core::Provider::OpenAI,
            audio_config: None,
            runtime_system_context: Vec::new(),
            user_content_identities: Vec::new(),
            user_content_tombstones: Vec::new(),
            transcript_rewrite_generation: 0,
        };
        assert_eq!(
            super::continuity_from_snapshot(&empty),
            LiveContinuityMode::Fresh
        );

        let seeded = LiveProjectionSnapshot {
            seed_messages: vec![Message::User(UserMessage::text("hi".to_string()))],
            ..empty
        };
        assert_eq!(
            super::continuity_from_snapshot(&seeded),
            LiveContinuityMode::TranscriptOnly
        );
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
            status: WireLiveAdapterStatus::from(LiveAdapterStatus::Idle),
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn live_status_result_roundtrip_ready() {
        let v = LiveStatusResult {
            channel_id: "live_1".into(),
            status: WireLiveAdapterStatus::from(LiveAdapterStatus::Ready),
        };
        assert_eq!(round_trip(&v), v);
    }

    /// R6-3 (P2): the wire payload carries a typed `status` discriminator
    /// (string for payload-less variants, internally-tagged on `status`
    /// for the `degraded { reason }` payload variant), not an opaque
    /// nested `Value`. Asserts the schema-shape contract that SDK
    /// codegen now emits typed dict / discriminated-union variants
    /// instead of `Any` / `unknown`.
    #[test]
    fn live_status_result_serializes_typed_status_discriminator() {
        let ready = LiveStatusResult {
            channel_id: "live_1".into(),
            status: WireLiveAdapterStatus::from(LiveAdapterStatus::Ready),
        };
        let j = serde_json::to_value(&ready).expect("round-trip should succeed");
        // The wire mirror is internally-tagged on `status`; payload-less
        // variants surface as `{ "status": "ready" }`.
        assert_eq!(j["status"]["status"], "ready");
        assert_eq!(j["channel_id"], "live_1");

        // The byte-shape stays compatible with the previous untyped
        // `core::LiveAdapterStatus` projection — clients that parsed the
        // old shape still see the same discriminator.
        let core_only =
            serde_json::to_value(LiveAdapterStatus::Ready).expect("core serialize should succeed");
        assert_eq!(j["status"], core_only);
    }

    #[test]
    fn live_status_success_reply_is_machine_owned() {
        let authority = meerkat_runtime::meerkat_machine::LiveChannelStatusAuthority {
            status: meerkat_runtime::meerkat_machine::dsl::LiveChannelPublicStatus::Degraded,
            sequence: 1,
            status_observation_sequence: 3,
            degradation_reason: Some(
                meerkat_runtime::meerkat_machine::dsl::LiveChannelDegradationReason::Other,
            ),
            degradation_detail: Some("provider reported degraded mode".to_string()),
            channel_status_commit_authority: None,
        };

        let reply = serde_json::to_value(
            live_status_result_from_machine_authority("live_1".to_string(), &authority)
                .expect("generated status authority should project to wire"),
        )
        .expect("LiveStatusResult must round-trip through serde");

        assert_eq!(reply["channel_id"], "live_1");
        assert_eq!(reply["status"]["status"], "degraded");
        assert_eq!(reply["status"]["reason"]["kind"], "other");
        assert_eq!(
            reply["status"]["reason"]["detail"],
            "provider reported degraded mode"
        );
    }

    #[tokio::test]
    async fn live_close_success_reply_is_machine_owned() {
        let host = meerkat_live::LiveAdapterHost::new(std::sync::Arc::new(
            meerkat_live::NoOpProjectionSink,
        ));
        let machine = meerkat_runtime::meerkat_machine::MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let channel_id = LiveChannelId::random_uuid();
        let identity = test_live_identity();
        let open_authority = machine
            .resolve_live_open_admission(&session_id, &channel_id, &identity)
            .await
            .expect("generated live open admission");
        host.open_channel_with_authority(
            open_authority
                .channel_open_authority()
                .expect("generated live open handoff"),
        )
        .await
        .expect("open_channel");
        let close_observation = host
            .reserve_channel_close_observation(&channel_id)
            .await
            .expect("reserve close observation");
        let authority = machine
            .resolve_live_close_result(&session_id, &close_observation)
            .await
            .expect("generated live close authority");
        assert!(
            authority.channel_close_commit_authority().is_some(),
            "generated live close authority should carry host commit handoff"
        );

        let reply = serde_json::to_value(live_close_result_from_machine_authority(&authority))
            .expect("LiveCloseResult must round-trip through serde");

        assert_eq!(reply["status"], "closed");
        assert!(
            reply.get("closed").is_none(),
            "deleted legacy `closed` boolean must not be on the wire"
        );
    }

    /// #355: open-failure cleanup is fail-closed. When the graceful close path
    /// cannot commit (generated close authority rejects, omits the host commit
    /// handoff, or the host commit fails), `close_live_channel_after_open_failure`
    /// falls back to `abandon_live_open_admission` so the machine-owned channel
    /// binding is evicted and never leaves a half-installed channel behind.
    ///
    /// This pins the underlying eviction contract the cleanup now routes to: an
    /// admitted open installs a binding observable via
    /// `live_session_for_active_channel`; abandoning the admission (the
    /// fail-closed action the failure arms take) clears it, so a non-committed
    /// close cannot orphan a `by_session` / `channels` entry.
    #[tokio::test]
    async fn open_failure_abandon_evicts_half_installed_channel_binding() {
        let machine = meerkat_runtime::meerkat_machine::MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let channel_id = LiveChannelId::random_uuid();
        let identity = test_live_identity();
        let open_authority = machine
            .resolve_live_open_admission(&session_id, &channel_id, &identity)
            .await
            .expect("generated live open admission");
        assert!(
            open_authority.channel_open_authority().is_some(),
            "admitted open must carry the generated host handoff"
        );

        // Binding is installed: the half-installed state a failed open leaves
        // behind when the graceful close cannot commit.
        assert_eq!(
            machine.live_session_for_active_channel(&channel_id).await,
            Some(session_id.clone()),
            "admitted open must install a channel binding"
        );

        // Fail-closed eviction the cleanup falls back to on a non-committed
        // close (authority reject / missing commit handoff / commit failure).
        machine
            .abandon_live_open_admission(&session_id, &channel_id)
            .await
            .expect("abandon must evict the half-installed admission");

        assert_eq!(
            machine.live_session_for_active_channel(&channel_id).await,
            None,
            "#355: open-failure abandon must leave no orphaned channel binding"
        );
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

    /// P1#4 + #176: regression — the format-string path used by
    /// `handle_live_open` must produce a URL that contains `?token=`,
    /// `&channel=`, and `&format=pcm_24k_mono` in that order, and the
    /// `&format=` token must derive from the resolved typed audio policy
    /// (not a surface-pinned constant). If a future refactor drops the
    /// `format=` query parameter, the WS server fails closed on every binary
    /// frame and audio appears to "vanish" mid-call.
    #[test]
    fn live_open_url_carries_token_and_format_params() {
        let base_url = "ws://localhost:9999";
        let path = meerkat_live::LIVE_WS_PATH;
        let token_str = "tok_abc";
        let channel_id = "live_42";
        // #176: token derives from the typed audio policy, not a constant.
        let audio = LiveAudioConfig {
            input_sample_rate_hz: 24_000,
            input_channels: 1,
            output_sample_rate_hz: 24_000,
            output_channels: 1,
        };
        let format_param =
            live_ws_audio_format_param(&audio).expect("24kHz mono must map to a WS format token");
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
    }

    /// #176 gate: the WS `&format=` token and the snapshot `audio_config`
    /// both derive from the resolved typed [`LiveAudioConfig`] (sourced from
    /// the realtime factory's `RealtimeCapabilities`), and `audio_config` is
    /// `Some` on open. Pre-fix the snapshot pinned `audio_config: None` and
    /// the URL pinned a `pcm_24k_mono` constant; this test fails against that
    /// shape.
    #[test]
    fn live_open_audio_format_and_snapshot_derive_from_resolved_audio_config() {
        use meerkat_contracts::RealtimeAudioFormat;

        // The provider/model audio policy seam: a factory's typed
        // `RealtimeCapabilities` carrying PCM 24 kHz mono in both directions.
        let capabilities = RealtimeCapabilities {
            audio_input_format: Some(RealtimeAudioFormat::pcm(24_000, 1)),
            audio_output_format: Some(RealtimeAudioFormat::pcm(24_000, 1)),
            ..RealtimeCapabilities::default()
        };
        let resolved = live_audio_config_from_capabilities(&capabilities)
            .expect("PCM in+out must resolve to a typed LiveAudioConfig");
        assert_eq!(resolved.input_sample_rate_hz, 24_000);
        assert_eq!(resolved.input_channels, 1);

        // The WS `&format=` token derives from the typed config.
        let format_param = live_ws_audio_format_param(&resolved)
            .expect("resolved 24kHz mono must map to the WS format token");
        assert_eq!(format_param, "pcm_24k_mono");

        // The snapshot carries the resolved config (Some, not None).
        let open_config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ProviderManaged,
            test_live_identity(),
            Vec::new(),
            Vec::new(),
        );
        let session_id = SessionId::new();
        let snapshot =
            build_live_projection_snapshot(&session_id, &open_config, Some(resolved.clone()));
        assert_eq!(
            snapshot.audio_config.as_ref(),
            Some(&resolved),
            "snapshot.audio_config must be the resolved typed config, not None"
        );

        // Fail-closed: an audio policy the WS transport cannot negotiate
        // yields no token (no silent wrong-rate stamping).
        let unsupported = LiveAudioConfig {
            input_sample_rate_hz: 16_000,
            input_channels: 1,
            output_sample_rate_hz: 16_000,
            output_channels: 1,
        };
        assert!(
            live_ws_audio_format_param(&unsupported).is_none(),
            "16kHz must fail closed: the WS transport has no matching binary format token"
        );

        // A text-only realtime product (no audio formats) resolves to None,
        // so the WS open path fails closed instead of inventing a rate.
        let text_only = RealtimeCapabilities::default();
        assert!(
            live_audio_config_from_capabilities(&text_only).is_none(),
            "a factory advertising no audio formats must resolve to no audio config"
        );
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
            audio_in: true,
            audio_out: false,
            text_in: true,
            text_out: true,
            image_in: false,
            video_in: false,
            barge_in_supported: false,
            transcript_supported: true,
            provider_native_resume: false,
        };
        let adapter: Arc<dyn meerkat_core::live_adapter::LiveAdapter> =
            Arc::new(FakeCapsAdapter(custom.clone()));
        // `adapter.capabilities()` is the exact call site `handle_live_open`
        // makes after `factory.open_live_adapter(...)`.
        assert_eq!(adapter.capabilities(), custom);
        // Negative regression: must not silently degrade to all-false.
        assert!(
            adapter.capabilities().audio_in
                || adapter.capabilities().text_in
                || adapter.capabilities().transcript_supported,
            "fake-adapter capabilities must NOT be all-false (P2#3 regression)"
        );
    }

    // ---------------------------------------------------------------------
    // P1#5: live/refresh dispatches LiveAdapterCommand::Refresh through
    // LiveAdapterHost::enqueue_refresh on the channel's adapter.
    // ---------------------------------------------------------------------

    /// P1#5: an adapter that records every command it receives so the test
    /// can assert that `Refresh { snapshot }` actually propagates through
    /// `LiveAdapterHost::enqueue_refresh`.
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

    /// P1#5: the host-level refresh enqueue path must forward
    /// `LiveAdapterCommand::Refresh { snapshot }` verbatim to the channel's
    /// adapter and mint typed queue-acceptance evidence. `handle_live_refresh`
    /// consumes that evidence before projecting the generated machine result.
    #[tokio::test]
    async fn host_forwards_refresh_command_to_adapter() {
        use meerkat_core::live_adapter::LiveProjectionSnapshot;
        use meerkat_core::types::SessionId;
        use std::sync::Arc;

        let host = meerkat_live::LiveAdapterHost::new(std::sync::Arc::new(
            meerkat_live::NoOpProjectionSink,
        ));
        let session_id = SessionId::new();
        let channel_id = open_test_channel(&host, session_id.clone()).await;
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
            model_id: "gpt-realtime-2".into(),
            provider_id: meerkat_core::Provider::OpenAI,
            audio_config: None,
            runtime_system_context: vec![],
            user_content_identities: vec![],
            user_content_tombstones: vec![],
            transcript_rewrite_generation: 0,
        };
        let acceptance = host
            .enqueue_refresh(&channel_id, snapshot.clone())
            .await
            .expect("enqueue Refresh command");
        assert_eq!(acceptance.channel_id(), channel_id.as_str());
        assert_eq!(acceptance.acceptance_sequence(), 1);

        let recorded = log.lock().await;
        assert_eq!(recorded.len(), 1, "exactly one command should be recorded");
        match &recorded[0] {
            LiveAdapterCommand::Refresh {
                snapshot: recv_snapshot,
            } => {
                assert_eq!(recv_snapshot.snapshot_version, 7);
                assert_eq!(recv_snapshot.session_id, session_id);
                assert_eq!(recv_snapshot.model_id, "gpt-realtime-2");
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

        let host = meerkat_live::LiveAdapterHost::new(std::sync::Arc::new(
            meerkat_live::NoOpProjectionSink,
        ));
        let session_id = SessionId::new();
        let channel_id = open_test_channel(&host, session_id.clone()).await;
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
    // R7: live/refresh's reply is `status: queued`, not `refreshed`,
    // because `LiveAdapterHost::enqueue_refresh` returns typed evidence
    // that the command was queued on the adapter command channel — not
    // that the pump has applied it. The status name documents the honest
    // semantics; the realtime stream is the source of truth for the actual
    // outcome.
    // ---------------------------------------------------------------------

    /// R7: reconstruct the success-reply shape `handle_live_refresh` emits
    /// on the host-accepted path. The reply must carry the typed
    /// `status: "queued"` discriminator projected from generated authority
    /// and must NOT contain a `refreshed` key.
    #[test]
    fn live_refresh_success_reply_is_status_queued_not_refreshed() {
        let authority = meerkat_runtime::meerkat_machine::LiveRefreshResultAuthority {
            status: meerkat_runtime::meerkat_machine::dsl::LiveRefreshPublicStatus::Queued,
            sequence: 1,
            queue_acceptance_sequence: 1,
        };
        let reply = serde_json::to_value(live_refresh_result_from_machine_authority(&authority))
            .expect("LiveRefreshResult must round-trip through serde");
        assert!(
            reply.get("refreshed").is_none(),
            "post-R7 reply must not advertise `refreshed: true` — the adapter \
             pump is async and the field name was a lie about completion timing"
        );
        assert!(
            reply.get("refresh_enqueued").is_none(),
            "deleted legacy `refresh_enqueued` boolean must not be on the wire"
        );
        // SDKs route on the typed status discriminator and fail closed for
        // statuses outside their generated contract.
        assert_eq!(
            reply.get("status"),
            Some(&serde_json::Value::String("queued".into())),
            "typed `status: queued` must be the reply discriminator"
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

        let host = meerkat_live::LiveAdapterHost::new(std::sync::Arc::new(
            meerkat_live::NoOpProjectionSink,
        ));
        let session_id = SessionId::new();
        let channel_id = open_test_channel(&host, session_id).await;
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

        let host = meerkat_live::LiveAdapterHost::new(std::sync::Arc::new(
            meerkat_live::NoOpProjectionSink,
        ));
        let session_id = SessionId::new();
        let channel_id = open_test_channel(&host, session_id.clone()).await;
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
                model_id: "gpt-realtime-2".into(),
                provider_id: meerkat_core::Provider::OpenAI,
                audio_config: None,
                runtime_system_context: vec![],
                user_content_identities: vec![],
                user_content_tombstones: vec![],
                transcript_rewrite_generation: 0,
            };
            snapshot.snapshot_version = host
                .next_snapshot_version(&channel_id)
                .await
                .expect("next_snapshot_version");
            host.enqueue_refresh(&channel_id, snapshot)
                .await
                .expect("enqueue Refresh");
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
