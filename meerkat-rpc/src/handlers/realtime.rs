//! Handlers for `realtime/*` bootstrap and control methods.

use serde_json::value::RawValue;

use meerkat_contracts::{
    ErrorCode, RealtimeCapabilities, RealtimeCapabilitiesParams, RealtimeCapabilitiesResult,
    RealtimeChannelState, RealtimeChannelStatus, RealtimeOpenRequest, RealtimeStatusParams,
    RealtimeStatusResult,
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};

fn conservative_phase_one_capabilities() -> RealtimeCapabilities {
    RealtimeCapabilities {
        input_kinds: vec![
            meerkat_contracts::RealtimeInputKind::Text,
            meerkat_contracts::RealtimeInputKind::Audio,
        ],
        output_kinds: vec![
            meerkat_contracts::RealtimeOutputKind::Text,
            meerkat_contracts::RealtimeOutputKind::Audio,
        ],
        turning_modes: vec![meerkat_contracts::RealtimeTurningMode::ProviderManaged],
        interrupt_supported: true,
        transcript_supported: true,
        tool_lifecycle_events_supported: false,
        video_supported: false,
        audio_input_format: None,
        audio_output_format: None,
    }
}

fn channel_status(
    state: RealtimeChannelState,
    reason: Option<&str>,
    attempt_count: u32,
) -> RealtimeChannelStatus {
    RealtimeChannelStatus {
        state,
        attempt_count,
        next_retry_at: None,
        deadline_at: None,
        reason: reason.map(str::to_string),
    }
}

pub(crate) fn realtime_status_from_runtime(
    status: meerkat_runtime::RealtimeAttachmentStatus,
) -> RealtimeChannelStatus {
    match status {
        meerkat_runtime::RealtimeAttachmentStatus::Unattached => channel_status(
            RealtimeChannelState::Closed,
            Some("no realtime channel is open for this target"),
            0,
        ),
        meerkat_runtime::RealtimeAttachmentStatus::IntentPresentUnbound
        | meerkat_runtime::RealtimeAttachmentStatus::BindingNotReady => channel_status(
            RealtimeChannelState::Opening,
            Some("realtime attachment is pending"),
            0,
        ),
        meerkat_runtime::RealtimeAttachmentStatus::BindingReady => {
            channel_status(RealtimeChannelState::Ready, None, 0)
        }
        meerkat_runtime::RealtimeAttachmentStatus::ReplacementPending => channel_status(
            RealtimeChannelState::Reconnecting,
            Some("realtime attachment replacement is pending"),
            1,
        ),
        meerkat_runtime::RealtimeAttachmentStatus::ReattachRequired => channel_status(
            RealtimeChannelState::Reconnecting,
            Some("realtime attachment requires reattach"),
            1,
        ),
    }
}

pub async fn handle_realtime_status(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
    mob_state: &std::sync::Arc<meerkat_mob_mcp::MobMcpState>,
) -> RpcResponse {
    let params: RealtimeStatusParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };

    // W3-H: resolve the target to a concrete bridge session id via the
    // MobMcpState resolver. SessionTarget parses directly; MobMember
    // resolves through the MobMachine's canonical binding map.
    let session_id = match mob_state
        .resolve_realtime_target_session(&params.target)
        .await
    {
        Ok(sid) => sid,
        Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string()),
    };
    let status = match adapter.realtime_attachment_status(&session_id).await {
        Ok(status) => realtime_status_from_runtime(status),
        Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string()),
    };

    RpcResponse::success(id, RealtimeStatusResult { status })
}

pub async fn handle_realtime_capabilities(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
    realtime_ws_host: Option<&crate::realtime_ws::RealtimeWsHost>,
    mob_state: &std::sync::Arc<meerkat_mob_mcp::MobMcpState>,
) -> RpcResponse {
    let params: RealtimeCapabilitiesParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };

    // W3-H: target resolution via the MobMcpState resolver.
    let session_id = match mob_state
        .resolve_realtime_target_session(&params.target)
        .await
    {
        Ok(sid) => sid,
        Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string()),
    };
    if let Err(err) = adapter.realtime_attachment_status(&session_id).await {
        return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string());
    }

    let capabilities = realtime_ws_host
        .and_then(crate::realtime_ws::RealtimeWsHost::session_factory_capabilities)
        .unwrap_or_else(conservative_phase_one_capabilities);

    RpcResponse::success(id, RealtimeCapabilitiesResult { capabilities })
}

pub async fn handle_realtime_open_info(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
    realtime_ws_host: Option<&crate::realtime_ws::RealtimeWsHost>,
    realm_id: Option<&str>,
    mob_state: &std::sync::Arc<meerkat_mob_mcp::MobMcpState>,
) -> RpcResponse {
    let params: RealtimeOpenRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };

    // W3-H: resolve the target — both SessionTarget and MobMember flow
    // through the MobMcpState resolver. For MobMember, the returned
    // session id is the initial binding; the WS accept path re-resolves
    // via the canonical binding map and subscribes to rotation events on
    // first tick.
    let session_id = match mob_state
        .resolve_realtime_target_session(&params.target)
        .await
    {
        Ok(sid) => sid,
        Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string()),
    };
    if let Err(err) = adapter.realtime_attachment_status(&session_id).await {
        return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string());
    }

    let capabilities = realtime_ws_host
        .and_then(crate::realtime_ws::RealtimeWsHost::session_factory_capabilities)
        .unwrap_or_else(conservative_phase_one_capabilities);
    if !capabilities.turning_modes.contains(&params.turning_mode) {
        return RpcResponse::error(
            id,
            ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            format!(
                "turning mode '{}' is not supported for this realtime target",
                match params.turning_mode {
                    meerkat_contracts::RealtimeTurningMode::ProviderManaged => "provider_managed",
                    meerkat_contracts::RealtimeTurningMode::ExplicitCommit => "explicit_commit",
                }
            ),
        );
    }

    let Some(realtime_ws_host) = realtime_ws_host else {
        return RpcResponse::error(
            id,
            ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            "realtime/open_info is unavailable until the realtime websocket host ships",
        );
    };

    let open_info = realtime_ws_host
        .issue_open_info(params, capabilities, realm_id.map(str::to_string))
        .await;
    RpcResponse::success(id, open_info)
}
