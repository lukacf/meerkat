//! Handlers for `live/*` RPC methods.
//!
//! These expose the live adapter MVP surface: open/status/close/send_input.
//! The transport bootstrap is tagged (websocket/webrtc), not a bare ws_url.

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

    let channel_id = match host.open_channel(session_id).await {
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::value::to_raw_value;

    fn raw(value: serde_json::Value) -> Box<serde_json::value::RawValue> {
        to_raw_value(&value).unwrap()
    }

    #[tokio::test]
    async fn live_open_returns_channel_id_and_tagged_transport() {
        let host = LiveAdapterHost::new();
        let params = raw(serde_json::json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef"
        }));
        let response = handle_live_open(
            Some(crate::protocol::RpcId::Num(1)),
            Some(&params),
            &host,
            None,
            None,
        )
        .await;
        assert!(response.error.is_none(), "expected success: {response:?}");
        let result: serde_json::Value =
            serde_json::from_str(response.result.unwrap().get()).unwrap();
        assert!(result["channel_id"].as_str().is_some());
        assert!(result["transport"]["transport"].as_str().is_some());
        assert!(result["capabilities"]["barge_in"].as_bool().is_some());
        assert_eq!(result["continuity"], "fresh");
    }

    #[tokio::test]
    async fn live_open_rejects_duplicate_session() {
        let host = LiveAdapterHost::new();
        let params = raw(serde_json::json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef"
        }));
        let _ = handle_live_open(
            Some(crate::protocol::RpcId::Num(1)),
            Some(&params),
            &host,
            None,
            None,
        )
        .await;
        let response = handle_live_open(
            Some(crate::protocol::RpcId::Num(2)),
            Some(&params),
            &host,
            None,
            None,
        )
        .await;
        assert!(response.error.is_some());
    }

    #[tokio::test]
    async fn live_status_returns_channel_status() {
        let host = LiveAdapterHost::new();
        let open_params = raw(serde_json::json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef"
        }));
        let open_resp = handle_live_open(
            Some(crate::protocol::RpcId::Num(1)),
            Some(&open_params),
            &host,
            None,
            None,
        )
        .await;
        let open_result: serde_json::Value =
            serde_json::from_str(open_resp.result.unwrap().get()).unwrap();
        let channel_id = open_result["channel_id"].as_str().unwrap();

        let status_params = raw(serde_json::json!({"channel_id": channel_id}));
        let response = handle_live_status(
            Some(crate::protocol::RpcId::Num(2)),
            Some(&status_params),
            &host,
        )
        .await;
        assert!(response.error.is_none(), "expected success: {response:?}");
        let result: serde_json::Value =
            serde_json::from_str(response.result.unwrap().get()).unwrap();
        assert_eq!(result["status"]["status"], "opening");
    }

    #[tokio::test]
    async fn live_close_removes_channel() {
        let host = LiveAdapterHost::new();
        let open_params = raw(serde_json::json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef"
        }));
        let open_resp = handle_live_open(
            Some(crate::protocol::RpcId::Num(1)),
            Some(&open_params),
            &host,
            None,
            None,
        )
        .await;
        let open_result: serde_json::Value =
            serde_json::from_str(open_resp.result.unwrap().get()).unwrap();
        let channel_id = open_result["channel_id"].as_str().unwrap();

        let close_params = raw(serde_json::json!({"channel_id": channel_id}));
        let response = handle_live_close(
            Some(crate::protocol::RpcId::Num(2)),
            Some(&close_params),
            &host,
        )
        .await;
        assert!(response.error.is_none());

        let status_resp = handle_live_status(
            Some(crate::protocol::RpcId::Num(3)),
            Some(&close_params),
            &host,
        )
        .await;
        assert!(
            status_resp.error.is_some(),
            "channel should not exist after close"
        );
    }

    #[tokio::test]
    async fn live_close_rejects_unknown_channel() {
        let host = LiveAdapterHost::new();
        let params = raw(serde_json::json!({"channel_id": "nonexistent"}));
        let response =
            handle_live_close(Some(crate::protocol::RpcId::Num(1)), Some(&params), &host).await;
        assert!(response.error.is_some());
    }
}
