//! Handlers for `live/*` RPC methods.
//!
//! Phase 6b (DL4): thin shims over the ONE extracted live pipeline
//! (`meerkat::session_runtime::live_orchestration::LiveOrchestrator`),
//! shared with the member-host bridge responder. Handlers own params
//! parsing, `LiveTransportContext` construction, and the exhaustive
//! typed-error -> `(code, message)` projections with the exact frozen
//! strings; open/close/control sequencing, machine admission, token mint,
//! bootstrap build, and the fail-closed open-failure cleanup live in the
//! facade pipeline. `live/webrtc/answer` (signaling) stays RPC-only by
//! design (DL9) and keeps its machine-reaching helpers here.

use std::sync::Arc;
#[cfg(feature = "live-webrtc")]
use std::time::{SystemTime, UNIX_EPOCH};

use meerkat::session_runtime::errors::{LiveChannelVerbError, LiveIngressError, LiveOpenError};
use meerkat::session_runtime::live_orchestration::{
    LiveSeedProjectionError, LiveSeedWindow, LiveTransportContext,
    RealtimeSessionOpenProjectionError,
};
use meerkat_client::realtime_session::RealtimeSessionFactory;
#[cfg(test)]
use meerkat_contracts::LiveInputChunkWire;
use meerkat_contracts::{
    LiveChannelParams, LiveCommitInputParams, LiveOpenParams, LiveSendInputErrorData,
    LiveSendInputParams, LiveStatusResult, LiveTruncateParams, WireLiveAdapterErrorCode,
};
#[cfg(feature = "live-webrtc")]
use meerkat_contracts::{LiveWebrtcAnswerParams, LiveWebrtcAnswerResult};
use meerkat_core::live_adapter::{LiveAdapterError, LiveAdapterErrorCode};
use meerkat_core::types::SessionId;
#[cfg(feature = "live-webrtc")]
use meerkat_live::LiveWebrtcState;
use meerkat_live::{
    LiveAdapterHost, LiveAdapterHostError, LiveChannelId, LiveWsState,
    live_input_chunk_decode_rejection, live_input_chunk_from_wire,
};

use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

/// Public live channel identifiers are UUID-shaped today. Keep a modest
/// forward-compatible bound so lookup failures cannot reflect an attacker-
/// sized identifier into a response buffer.
const MAX_LIVE_CHANNEL_ID_BYTES: usize = 128;

fn live_seed_window_from_params(
    id: Option<RpcId>,
    seed_max_chars: Option<usize>,
) -> Result<Option<LiveSeedWindow>, Box<RpcResponse>> {
    match seed_max_chars {
        Some(max_chars) => LiveSeedWindow::new(max_chars)
            .map(Some)
            .map_err(|projection_error| {
                Box::new(RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    projection_error.to_string(),
                ))
            }),
        None => Ok(None),
    }
}

fn live_open_projection_error_code(error: &RealtimeSessionOpenProjectionError) -> i32 {
    match error {
        RealtimeSessionOpenProjectionError::Seed(
            LiveSeedProjectionError::ZeroWindow | LiveSeedProjectionError::RootExceedsWindow { .. },
        ) => crate::error::INVALID_PARAMS,
        RealtimeSessionOpenProjectionError::Session(_)
        | RealtimeSessionOpenProjectionError::Seed(
            LiveSeedProjectionError::Session(_)
            | LiveSeedProjectionError::Serialization(_)
            | LiveSeedProjectionError::SizeOverflow,
        ) => crate::error::INTERNAL_ERROR,
    }
}

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

#[cfg(feature = "live-webrtc")]
fn live_webrtc_now_ms() -> Result<u64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system time is before Unix epoch: {err}"))?;
    u64::try_from(elapsed.as_millis())
        .map_err(|_| "system time milliseconds overflow u64".to_string())
}

#[cfg(feature = "live-webrtc")]
async fn live_session_id_from_machine_authority(
    runtime: &Arc<SessionRuntime>,
    channel_id: &LiveChannelId,
) -> Option<SessionId> {
    runtime
        .runtime_adapter()
        .live_session_for_active_channel(channel_id)
        .await
}

/// Exhaustive typed-error -> `(code, message)` projection for the open
/// pipeline. Every string is the pre-extraction handler literal (behavior
/// freeze: `live_rpc_regression` / `live_smoke_rpc` pass unmodified); the
/// message text is owned by the typed error's `Display` where one exists.
/// NO wildcard arm: a new pipeline variant forces this surface to decide.
fn live_open_error_response(id: Option<RpcId>, error: LiveOpenError) -> RpcResponse {
    use meerkat::session_runtime::errors::LiveOpenPrecheckError;

    match error {
        // B17 not-found stays INVALID_PARAMS ("session {id} not found").
        LiveOpenError::SessionNotFound { .. } => {
            RpcResponse::error(id, error::INVALID_PARAMS, error.to_string())
        }
        // Row #98: a store fault keeps its canonical session-error wire
        // mapping (never collapsed into not-found).
        LiveOpenError::SessionStateFault(source) => {
            let rpc = crate::session_runtime::session_error_to_rpc(source);
            RpcResponse::error(id, rpc.code, rpc.message)
        }
        LiveOpenError::OpenConfig(source) => RpcResponse::error(
            id,
            live_open_projection_error_code(&source),
            format!("failed to build session config: {source}"),
        ),
        LiveOpenError::RealtimeFactoryMissing
        | LiveOpenError::AdmissionAuthority(_)
        | LiveOpenError::AdmissionRejectedChannelCollision { .. }
        | LiveOpenError::AdmissionRejectedNoReason
        | LiveOpenError::MissingHostHandoff
        | LiveOpenError::HostOpenSessionAlreadyBound { .. }
        | LiveOpenError::HostOpen(_)
        | LiveOpenError::ProviderUnsupportedByFactory { .. }
        | LiveOpenError::AdapterOpen(_)
        | LiveOpenError::AdapterAttach(_)
        | LiveOpenError::NoTransportConfigured
        | LiveOpenError::TokenMint(_)
        | LiveOpenError::AudioPolicyMissing
        | LiveOpenError::AudioFormatUnmappable { .. }
        | LiveOpenError::WebrtcClock(_)
        | LiveOpenError::WebrtcTokenMint(_) => {
            RpcResponse::error(id, error::INTERNAL_ERROR, error.to_string())
        }
        LiveOpenError::AdmissionRejectedAlreadyBound { .. }
        | LiveOpenError::AdmissionRejectedLifecycleClosed
        | LiveOpenError::WebsocketNotConfigured
        | LiveOpenError::WebrtcNotConfigured
        | LiveOpenError::WebrtcNotCompiled
        | LiveOpenError::UnsupportedTransport => {
            RpcResponse::error(id, error::INVALID_PARAMS, error.to_string())
        }
        LiveOpenError::Precheck(precheck) => {
            let code = match &precheck {
                LiveOpenPrecheckError::ModelNotRealtime { .. } => error::INVALID_PARAMS,
                // SessionLookup at this point is unexpected -- B17 already
                // rejected missing sessions. Surface as INTERNAL_ERROR.
                LiveOpenPrecheckError::ProviderHasNoLiveAdapter { .. }
                | LiveOpenPrecheckError::SessionLookup { .. } => error::INTERNAL_ERROR,
            };
            RpcResponse::error(id, code, precheck.to_string())
        }
        LiveOpenError::Ingress(ingress) => match ingress {
            LiveIngressError::InvalidSession => {
                RpcResponse::error(id, error::INVALID_PARAMS, ingress.to_string())
            }
            LiveIngressError::Internal(message) => {
                RpcResponse::error(id, error::INTERNAL_ERROR, message)
            }
        },
    }
}

/// Exhaustive typed-error -> `(code, message)` projection for the channel
/// verbs. Rejection-class responses rebuild through the same generated
/// authority projections as before extraction (the authorities travel
/// inside the typed error; no second machine reach).
fn live_verb_error_response(id: Option<RpcId>, error: LiveChannelVerbError) -> RpcResponse {
    match error {
        LiveChannelVerbError::UnboundCommand {
            channel_id,
            authority,
            expected,
        } => {
            let channel = LiveChannelId::new(&channel_id);
            let detail = LiveAdapterHostError::ChannelNotFound(channel.clone()).to_string();
            live_command_rejection_response_from_machine_authority(
                id, &authority, expected, &channel, &detail, None,
            )
        }
        LiveChannelVerbError::UnboundRequest {
            channel_id,
            authority,
            expected,
            detail,
        } => live_channel_request_rejection_response_from_machine_authority(
            id,
            &authority,
            expected,
            &LiveChannelId::new(&channel_id),
            detail,
        ),
        LiveChannelVerbError::CommandRejected {
            channel_id,
            authority,
            expected,
            detail,
            host_error,
        } => live_command_rejection_response_from_machine_authority(
            id,
            &authority,
            expected,
            &LiveChannelId::new(&channel_id),
            &detail,
            Some(host_error.as_ref()),
        ),
        LiveChannelVerbError::RequestRejected {
            channel_id,
            authority,
            expected,
            detail,
        } => live_channel_request_rejection_response_from_machine_authority(
            id,
            &authority,
            expected,
            &LiveChannelId::new(&channel_id),
            detail,
        ),
        // DEC-P6B-L6: RPC passes no session pin, so this arm is structurally
        // unreachable on this surface; project defensively as the unbound
        // shape a channel-addressed caller would observe.
        LiveChannelVerbError::SessionPinMismatch { channel_id } => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("channel {channel_id} not found"),
        ),
        LiveChannelVerbError::RejectionAuthorityFailed { message }
        | LiveChannelVerbError::ResultAuthority { message }
        | LiveChannelVerbError::ResultProjection { message } => {
            RpcResponse::error(id, error::INTERNAL_ERROR, message)
        }
        error @ (LiveChannelVerbError::CommitOmitted
        | LiveChannelVerbError::HostCommit { .. }
        | LiveChannelVerbError::RefreshConfig(_)) => {
            RpcResponse::error(id, error::INTERNAL_ERROR, error.to_string())
        }
    }
}

fn live_command_rejection_response_from_machine_authority(
    id: Option<RpcId>,
    authority: &meerkat_runtime::meerkat_machine::LiveCommandRejectionAuthority,
    expected: meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind,
    channel_id: &LiveChannelId,
    detail: &str,
    host_error: Option<&LiveAdapterHostError>,
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
        | LiveCommandRejectionReason::InternalHostError => detail.to_string(),
    };
    if expected == meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind::SendInput
        && let Some(data) = host_error.and_then(live_send_input_error_data)
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

#[cfg(feature = "live-webrtc")]
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

    let transport_ctx = LiveTransportContext::new(live_ws, live_ws_base_url);
    #[cfg(feature = "live-webrtc")]
    let transport_ctx = transport_ctx.with_webrtc(live_webrtc);
    // Validate caller-controlled seed policy before the shared pipeline can
    // mint a candidate channel or ask machine authority to admit the open.
    let seed_window = match live_seed_window_from_params(id.clone(), parsed.seed_max_chars) {
        Ok(seed_window) => seed_window,
        Err(response) => return *response,
    };
    match runtime
        .open_live_channel(
            host,
            transport_ctx,
            session_factory,
            &session_id,
            parsed.turning_mode,
            seed_window,
            parsed.transport,
        )
        .await
    {
        // N75: fixed-shape struct of `Serialize`-clean fields; on the
        // unexpected path return a typed INTERNAL_ERROR, never `null`.
        Ok(result) => match serde_json::to_value(result) {
            Ok(value) => RpcResponse::success(id, value),
            Err(err) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("failed to serialize LiveOpenResult: {err}"),
            ),
        },
        Err(open_error) => live_open_error_response(id, open_error),
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

    // Peer creation is transport materialization just like live/open's
    // provider adapter creation. Retain the same session lifecycle lease
    // through admission, peer installation, and generated answer commit so a
    // lifecycle close cannot prove the WebRTC registry empty and then race a
    // late answer peer into existence before its terminal marker.
    let _live_lifecycle_lease = match runtime
        .runtime_adapter()
        .acquire_live_open_lifecycle_lease(&session_id)
        .await
    {
        Ok(lease) => lease,
        Err(error) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("live WebRTC answer lifecycle authority unavailable: {error}"),
            );
        }
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

    match runtime.live_channel_status(host, &channel_id).await {
        Ok(status) => {
            let result = LiveStatusResult {
                channel_id: parsed.channel_id,
                status,
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
        Err(verb_error) => live_verb_error_response(id, verb_error),
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

    match runtime.close_live_channel(host, &channel_id).await {
        Ok(result) => match serde_json::to_value(result) {
            Ok(body) => RpcResponse::success(id, body),
            Err(error) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("live close authority projection failed: {error}"),
            ),
        },
        Err(verb_error) => live_verb_error_response(id, verb_error),
    }
}

/// P1#5: `live/refresh` — enqueue a mutable live-config update against the
/// active live session. Config-only by design (R1+R9: never replays
/// canonical history); identity changes require close + reopen; the reply
/// is `status: queued` (R7 — the host queues, the adapter pump applies
/// asynchronously). The rebuild/stamp/enqueue sequencing lives in the
/// shared pipeline (`LiveOrchestrator::refresh_live_channel`).
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

    match runtime.refresh_live_channel(host, &channel_id).await {
        Ok(result) => match serde_json::to_value(result) {
            Ok(body) => RpcResponse::success(id, body),
            Err(error) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("live refresh queued authority projection failed: {error}"),
            ),
        },
        Err(verb_error) => live_verb_error_response(id, verb_error),
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

    match runtime.send_live_input(host, &channel_id, chunk).await {
        Ok(result) => match serde_json::to_value(result) {
            Ok(value) => RpcResponse::success(id, value),
            Err(err) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("failed to serialize LiveSendInputResult: {err}"),
            ),
        },
        Err(verb_error) => live_verb_error_response(id, verb_error),
    }
}

/// I50: `live/commit_input` — flush any buffered uncommitted input on the
/// channel. G9 (P2): the optional `response_modality` param requests a
/// text-only response on an audio-first channel without flipping the
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
    // the client doesn't yet understand; reject rather than coerce.
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

    match runtime
        .commit_live_input(host, &channel_id, response_modality)
        .await
    {
        Ok(result) => match serde_json::to_value(result) {
            Ok(value) => RpcResponse::success(id, value),
            Err(err) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("failed to serialize LiveCommitInputResult: {err}"),
            ),
        },
        Err(verb_error) => live_verb_error_response(id, verb_error),
    }
}

// ---------------------------------------------------------------------------
// A7 — public interrupt / truncate surface
// ---------------------------------------------------------------------------
//
// Barge-in is advertised on `LiveChannelCapabilities`; `live/interrupt` and
// `live/truncate` give the caller an explicit handle. There is no
// `live/playback_cursor` read API: playback is a *client* fact — clients
// track the cursor locally and pass `audio_played_ms` into `live/truncate`.

/// A7: `live/interrupt` — signal the adapter to interrupt the in-progress
/// assistant turn. Maps to `LiveAdapterCommand::Interrupt` inside the
/// shared pipeline (media-plane barge-in, never hard-interrupt authority).
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
    let transport_ctx = LiveTransportContext::new(None, None);
    #[cfg(feature = "live-webrtc")]
    let transport_ctx = transport_ctx.with_webrtc(webrtc_state);

    match runtime
        .interrupt_live_channel(host, transport_ctx, &channel_id)
        .await
    {
        Ok(result) => match serde_json::to_value(result) {
            Ok(value) => RpcResponse::success(id, value),
            Err(err) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("failed to serialize LiveInterruptResult: {err}"),
            ),
        },
        Err(verb_error) => live_verb_error_response(id, verb_error),
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
    let transport_ctx = LiveTransportContext::new(None, None);
    #[cfg(feature = "live-webrtc")]
    let transport_ctx = transport_ctx.with_webrtc(webrtc_state);

    match runtime
        .truncate_live_output(
            host,
            transport_ctx,
            &channel_id,
            parsed.item_id.clone(),
            parsed.content_index,
            parsed.audio_played_ms,
        )
        .await
    {
        Ok(result) => match serde_json::to_value(result) {
            Ok(value) => RpcResponse::success(id, value),
            Err(err) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("failed to serialize LiveTruncateResult: {err}"),
            ),
        },
        Err(verb_error) => live_verb_error_response(id, verb_error),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    //! H46 round-trip tests + H48 nested `chunk` shape tests for the
    //! `live/*` wire types. (D24 base64-decode rejection is owned by
    //! `meerkat_live::wire_input` and tested there.)

    use super::*;
    use meerkat::session_runtime::live_orchestration::{
        LiveSeedProjectionStatus, build_live_projection_snapshot, continuity_from_snapshot,
        live_audio_config_from_capabilities, live_close_result_from_machine_authority,
        live_refresh_result_from_machine_authority, live_ws_audio_format_param,
        wire_live_status_from_machine_authority,
    };
    use meerkat_client::realtime_session::RealtimeSessionOpenConfig;
    use meerkat_contracts::{
        LiveInputChunkWire, LiveOpenParams, LiveOpenResult, LiveOpenTransport,
        RealtimeCapabilities, RealtimeTurningMode, WireLiveAdapterStatus,
    };
    use meerkat_core::SessionLlmIdentity;
    use meerkat_core::live_adapter::{
        LiveAdapterCommand, LiveAdapterStatus, LiveAudioConfig, LiveChannelCapabilities,
        LiveContinuityMode, LiveProjectionSnapshot,
    };
    use serde::Serialize;

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
            seed_max_chars: None,
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
            seed_max_chars: Some(24_000),
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
    fn continuity_from_snapshot_tracks_seed_completeness_without_provider_resume() {
        // T12: `ProviderNativeResume { provider_session_id }` is reserved
        // for a future provider that surfaces a resume id. A complete seed
        // is `Fresh` when empty and `TranscriptOnly` when non-empty. A
        // windowed seed has known canonical-history gaps and must be
        // `Degraded`, even when the retained provider seed is empty.
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
            canonical_user_image_decoded_bytes: None,
            transcript_rewrite_generation: 0,
        };
        assert_eq!(
            continuity_from_snapshot(&empty, LiveSeedProjectionStatus::Complete),
            LiveContinuityMode::Fresh
        );

        let seeded = LiveProjectionSnapshot {
            seed_messages: vec![Message::User(UserMessage::text("hi".to_string()))],
            ..empty.clone()
        };
        assert_eq!(
            continuity_from_snapshot(&seeded, LiveSeedProjectionStatus::Complete),
            LiveContinuityMode::TranscriptOnly
        );

        let windowed = LiveSeedProjectionStatus::Windowed {
            dropped_messages: 3,
            included_compaction_summary: false,
        };
        assert_eq!(
            continuity_from_snapshot(&empty, windowed),
            LiveContinuityMode::Degraded,
            "known gaps must stay degraded even when no replay messages fit"
        );
        assert_eq!(
            continuity_from_snapshot(&seeded, windowed),
            LiveContinuityMode::Degraded,
            "a retained suffix must not conceal omitted canonical history"
        );
    }

    #[test]
    fn zero_live_seed_window_is_invalid_params_before_channel_creation() {
        let response = *super::live_seed_window_from_params(Some(RpcId::Num(30)), Some(0))
            .expect_err("zero is not a valid live seed window");
        let rpc_error = response.error.expect("typed JSON-RPC error");
        assert_eq!(rpc_error.code, error::INVALID_PARAMS);
        assert_eq!(
            rpc_error.message,
            "live seed window must be greater than zero"
        );
    }

    #[test]
    fn typed_live_seed_projection_errors_map_without_string_reclassification() {
        let too_small =
            RealtimeSessionOpenProjectionError::Seed(LiveSeedProjectionError::RootExceedsWindow {
                required_chars: 11,
                max_chars: 10,
            });
        assert_eq!(
            super::live_open_projection_error_code(&too_small),
            error::INVALID_PARAMS
        );

        let internal =
            RealtimeSessionOpenProjectionError::Seed(LiveSeedProjectionError::SizeOverflow);
        assert_eq!(
            super::live_open_projection_error_code(&internal),
            error::INTERNAL_ERROR
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

        let status = wire_live_status_from_machine_authority(&authority)
            .expect("generated status authority should project to wire");
        let reply = serde_json::to_value(LiveStatusResult {
            channel_id: "live_1".to_string(),
            status,
        })
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
            canonical_user_image_decoded_bytes: None,
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
                canonical_user_image_decoded_bytes: None,
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
