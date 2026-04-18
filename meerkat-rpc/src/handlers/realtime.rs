//! Handlers for `realtime/*` bootstrap and control methods.

use serde_json::value::RawValue;

use meerkat_contracts::{
    ErrorCode, RealtimeCapabilities, RealtimeCapabilitiesParams, RealtimeCapabilitiesResult,
    RealtimeChannelState, RealtimeChannelStatus, RealtimeChannelTarget, RealtimeOpenRequest,
    RealtimeStatusParams, RealtimeStatusResult,
};
use meerkat_core::SessionId;
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

pub(crate) fn realtime_status_from_mob_serialized(
    status: &serde_json::Value,
) -> Result<RealtimeChannelStatus, String> {
    let Some(status) = status.as_str() else {
        return Err("mob member live attachment status should serialize as a string".to_string());
    };
    let projected = match status {
        "unattached" => channel_status(
            RealtimeChannelState::Closed,
            Some("no realtime channel is open for this target"),
            0,
        ),
        "intent_present_unbound" | "binding_not_ready" => channel_status(
            RealtimeChannelState::Opening,
            Some("realtime attachment is pending"),
            0,
        ),
        "binding_ready" => channel_status(RealtimeChannelState::Ready, None, 0),
        "replacement_pending" => channel_status(
            RealtimeChannelState::Reconnecting,
            Some("realtime attachment replacement is pending"),
            1,
        ),
        "reattach_required" => channel_status(
            RealtimeChannelState::Reconnecting,
            Some("realtime attachment requires reattach"),
            1,
        ),
        other => {
            return Err(format!("unsupported mob live attachment status '{other}'"));
        }
    };
    Ok(projected)
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait RealtimeMobInspector: Send + Sync {
    async fn member_channel_status(
        &self,
        mob_id: &str,
        agent_identity: &str,
    ) -> Result<RealtimeChannelStatus, String>;

    async fn ensure_member_target(&self, mob_id: &str, agent_identity: &str) -> Result<(), String>;
}

#[cfg(feature = "mob")]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl RealtimeMobInspector for meerkat_mob_mcp::MobMcpState {
    async fn member_channel_status(
        &self,
        mob_id: &str,
        agent_identity: &str,
    ) -> Result<RealtimeChannelStatus, String> {
        let mob_id = meerkat_mob::MobId::from(mob_id);
        let snapshot = self
            .mob_member_status(&mob_id, &meerkat_mob::AgentIdentity::from(agent_identity))
            .await
            .map_err(|err| err.to_string())?;
        let serialized = serde_json::to_value(snapshot.realtime_attachment_status)
            .map_err(|err| err.to_string())?;
        realtime_status_from_mob_serialized(&serialized)
    }

    async fn ensure_member_target(&self, mob_id: &str, agent_identity: &str) -> Result<(), String> {
        let mob_id = meerkat_mob::MobId::from(mob_id);
        self.mob_member_status(&mob_id, &meerkat_mob::AgentIdentity::from(agent_identity))
            .await
            .map(|_| ())
            .map_err(|err| err.to_string())
    }
}

fn parse_session_target(raw: &str) -> Result<SessionId, String> {
    SessionId::parse(raw).map_err(|err| err.to_string())
}

fn parse_member_target(mob_id: &str, agent_identity: &str) -> Result<(), String> {
    if mob_id.trim().is_empty() {
        return Err("mob_id must not be empty".to_string());
    }
    if agent_identity.trim().is_empty() {
        return Err("agent_identity must not be empty".to_string());
    }
    Ok(())
}

pub async fn handle_realtime_status(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
    mob_inspector: Option<&dyn RealtimeMobInspector>,
) -> RpcResponse {
    let params: RealtimeStatusParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };

    let status = match params.target {
        RealtimeChannelTarget::SessionTarget { session_id } => {
            let session_id = match parse_session_target(&session_id) {
                Ok(session_id) => session_id,
                Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err),
            };
            match adapter.realtime_attachment_status(&session_id).await {
                Ok(status) => realtime_status_from_runtime(status),
                Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string()),
            }
        }
        RealtimeChannelTarget::MobMemberTarget {
            mob_id,
            agent_identity,
        } => {
            if let Err(err) = parse_member_target(&mob_id, &agent_identity) {
                return RpcResponse::error(id, error::INVALID_PARAMS, err);
            }
            let Some(inspector) = mob_inspector else {
                return RpcResponse::error(
                    id,
                    ErrorCode::CapabilityUnavailable.jsonrpc_code(),
                    "mob-backed realtime targets are unavailable on this surface build",
                );
            };
            match inspector
                .member_channel_status(&mob_id, &agent_identity)
                .await
            {
                Ok(status) => status,
                Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err),
            }
        }
    };

    RpcResponse::success(id, RealtimeStatusResult { status })
}

pub async fn handle_realtime_capabilities(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
    mob_inspector: Option<&dyn RealtimeMobInspector>,
    realtime_ws_host: Option<&crate::realtime_ws::RealtimeWsHost>,
) -> RpcResponse {
    let params: RealtimeCapabilitiesParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };

    match &params.target {
        RealtimeChannelTarget::SessionTarget { session_id } => {
            let session_id = match parse_session_target(session_id) {
                Ok(session_id) => session_id,
                Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err),
            };
            if let Err(err) = adapter.realtime_attachment_status(&session_id).await {
                return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string());
            }
        }
        RealtimeChannelTarget::MobMemberTarget {
            mob_id,
            agent_identity,
        } => {
            if let Err(err) = parse_member_target(mob_id, agent_identity) {
                return RpcResponse::error(id, error::INVALID_PARAMS, err);
            }
            let Some(inspector) = mob_inspector else {
                return RpcResponse::error(
                    id,
                    ErrorCode::CapabilityUnavailable.jsonrpc_code(),
                    "mob-backed realtime targets are unavailable on this surface build",
                );
            };
            if let Err(err) = inspector.ensure_member_target(mob_id, agent_identity).await {
                return RpcResponse::error(id, error::INVALID_PARAMS, err);
            }
        }
    }

    let capabilities = match params.target {
        RealtimeChannelTarget::SessionTarget { .. } => realtime_ws_host
            .and_then(crate::realtime_ws::RealtimeWsHost::session_factory_capabilities)
            .unwrap_or_else(conservative_phase_one_capabilities),
        RealtimeChannelTarget::MobMemberTarget { .. } => conservative_phase_one_capabilities(),
    };

    RpcResponse::success(id, RealtimeCapabilitiesResult { capabilities })
}

pub async fn handle_realtime_open_info(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
    mob_inspector: Option<&dyn RealtimeMobInspector>,
    realtime_ws_host: Option<&crate::realtime_ws::RealtimeWsHost>,
    realm_id: Option<&str>,
) -> RpcResponse {
    let params: RealtimeOpenRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };

    match &params.target {
        RealtimeChannelTarget::SessionTarget { session_id } => {
            let session_id = match parse_session_target(session_id) {
                Ok(session_id) => session_id,
                Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err),
            };
            if let Err(err) = adapter.realtime_attachment_status(&session_id).await {
                return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string());
            }
        }
        RealtimeChannelTarget::MobMemberTarget {
            mob_id,
            agent_identity,
        } => {
            if let Err(err) = parse_member_target(mob_id, agent_identity) {
                return RpcResponse::error(id, error::INVALID_PARAMS, err);
            }
            let Some(inspector) = mob_inspector else {
                return RpcResponse::error(
                    id,
                    ErrorCode::CapabilityUnavailable.jsonrpc_code(),
                    "mob-backed realtime targets are unavailable on this surface build",
                );
            };
            if let Err(err) = inspector.ensure_member_target(mob_id, agent_identity).await {
                return RpcResponse::error(id, error::INVALID_PARAMS, err);
            }
        }
    }

    let capabilities = match &params.target {
        RealtimeChannelTarget::SessionTarget { .. } => realtime_ws_host
            .and_then(crate::realtime_ws::RealtimeWsHost::session_factory_capabilities)
            .unwrap_or_else(conservative_phase_one_capabilities),
        RealtimeChannelTarget::MobMemberTarget { .. } => conservative_phase_one_capabilities(),
    };
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
