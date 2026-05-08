//! Handlers for `live/*` RPC methods.
//!
//! These expose the live adapter MVP surface: open/status/close/send_input.
//! The transport bootstrap is tagged (websocket/webrtc), not a bare ws_url.

use std::sync::Arc;

use meerkat_client::realtime_session::RealtimeSessionFactory;
use meerkat_contracts::RealtimeTurningMode;
use meerkat_core::live_adapter::{
    LiveAdapterStatus, LiveChannelCapabilities, LiveContinuityMode, LiveInputChunk,
    LiveTransportBootstrap,
};
use meerkat_core::types::SessionId;
use meerkat_live::LiveWsState;
use meerkat_runtime::live_adapter_host::{LiveAdapterHost, LiveAdapterHostError, LiveChannelId};
use serde::{Deserialize, Serialize};

use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

#[derive(Deserialize)]
pub struct LiveOpenParams {
    pub session_id: String,
}

#[derive(Serialize)]
pub struct LiveOpenResult {
    pub channel_id: String,
    pub transport: LiveTransportBootstrap,
    pub capabilities: LiveChannelCapabilities,
    pub continuity: LiveContinuityMode,
}

#[derive(Deserialize)]
pub struct LiveChannelParams {
    pub channel_id: String,
}

#[derive(Serialize)]
pub struct LiveStatusResult {
    pub channel_id: String,
    pub status: LiveAdapterStatus,
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
                let adapter = Box::new(meerkat_live::ProviderSessionAdapter::new(session));
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

    let transport = if let (Some(ws_state), Some(base_url)) = (live_ws, live_ws_base_url) {
        let token = ws_state.mint_token(channel_id.clone()).await;
        LiveTransportBootstrap::Websocket {
            url: format!(
                "{base_url}{path}?token={token}",
                path = meerkat_live::LIVE_WS_PATH
            ),
            token,
        }
    } else {
        LiveTransportBootstrap::Websocket {
            url: String::new(),
            token: String::new(),
        }
    };

    let result = LiveOpenResult {
        channel_id: channel_id.to_string(),
        transport,
        capabilities: LiveChannelCapabilities::default(),
        continuity: LiveContinuityMode::Fresh,
    };

    RpcResponse::success(id, serde_json::to_value(result).unwrap_or_default())
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
            RpcResponse::success(id, serde_json::to_value(result).unwrap_or_default())
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

#[derive(Deserialize)]
pub struct LiveSendInputParams {
    pub channel_id: String,
    #[serde(flatten)]
    pub chunk: LiveInputChunkWire,
}

#[derive(Deserialize)]
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

impl LiveInputChunkWire {
    fn into_chunk(self) -> LiveInputChunk {
        match self {
            Self::Audio {
                data,
                sample_rate_hz,
                channels,
            } => {
                use base64::Engine;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(&data)
                    .unwrap_or_default();
                LiveInputChunk::Audio {
                    data: decoded,
                    sample_rate_hz,
                    channels,
                }
            }
            Self::Text { text } => LiveInputChunk::Text { text },
        }
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
    let chunk = parsed.chunk.into_chunk();

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
