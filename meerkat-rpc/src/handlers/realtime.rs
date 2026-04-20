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

/// W3-H: resolve a RealtimeChannelTarget to a concrete bridge session id.
/// On mob-enabled builds this routes through the MobMcpState resolver
/// (SessionTarget parses directly; MobMember reads the MobMachine's
/// canonical binding map). On mob-disabled builds MobMember targets are
/// rejected with INVALID_PARAMS, and SessionTarget parses directly.
#[cfg(feature = "mob")]
async fn resolve_realtime_target_session(
    target: &meerkat_contracts::RealtimeChannelTarget,
    mob_state: &std::sync::Arc<meerkat_mob_mcp::MobMcpState>,
) -> Result<meerkat_core::types::SessionId, String> {
    mob_state
        .resolve_realtime_target_session(target)
        .await
        .map_err(|err| err.to_string())
}

#[cfg(not(feature = "mob"))]
async fn resolve_realtime_target_session(
    target: &meerkat_contracts::RealtimeChannelTarget,
) -> Result<meerkat_core::types::SessionId, String> {
    match target {
        meerkat_contracts::RealtimeChannelTarget::SessionTarget { session_id } => {
            meerkat_core::types::SessionId::parse(session_id)
                .map_err(|err| format!("invalid session target: {err}"))
        }
        meerkat_contracts::RealtimeChannelTarget::MobMember { .. } => Err(
            "mob-member realtime targets require this host to be built with the `mob` feature"
                .to_string(),
        ),
    }
}

pub async fn handle_realtime_status(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
    #[cfg(feature = "mob")] mob_state: &std::sync::Arc<meerkat_mob_mcp::MobMcpState>,
) -> RpcResponse {
    let params: RealtimeStatusParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };

    let session_id = match resolve_realtime_target_session(
        &params.target,
        #[cfg(feature = "mob")]
        mob_state,
    )
    .await
    {
        Ok(sid) => sid,
        Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err),
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
    #[cfg(feature = "mob")] mob_state: &std::sync::Arc<meerkat_mob_mcp::MobMcpState>,
) -> RpcResponse {
    let params: RealtimeCapabilitiesParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };

    let session_id = match resolve_realtime_target_session(
        &params.target,
        #[cfg(feature = "mob")]
        mob_state,
    )
    .await
    {
        Ok(sid) => sid,
        Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err),
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
    #[cfg(feature = "mob")] mob_state: &std::sync::Arc<meerkat_mob_mcp::MobMcpState>,
) -> RpcResponse {
    let params: RealtimeOpenRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };

    let session_id = match resolve_realtime_target_session(
        &params.target,
        #[cfg(feature = "mob")]
        mob_state,
    )
    .await
    {
        Ok(sid) => sid,
        Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err),
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
