//! `comms/*` handlers — canonical comms command dispatch and peer discovery.

pub const COMMS_STREAM_CONTRACT_VERSION: &str = "0.3.0";

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

use super::{parse_params, parse_session_id_for_runtime};

/// Parameters for `comms/send`.
#[derive(Deserialize)]
pub struct CommsSendParams {
    pub session_id: String,
    pub kind: String,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub allow_self_session: Option<bool>,
}

/// Parameters for `comms/peers`.
#[derive(Deserialize)]
pub struct CommsPeersParams {
    pub session_id: String,
}

/// Handle `comms/send` — dispatch a canonical comms command.
pub async fn handle_send(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let params: CommsSendParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };

    let comms = match runtime.comms_runtime(&session_id).await {
        Some(c) => c,
        None => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Session not found or comms not enabled: {session_id}"),
            );
        }
    };

    let cmd = match build_comms_command(&params, &session_id) {
        Ok(cmd) => cmd,
        Err(details) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                serde_json::json!({
                    "code": "invalid_command",
                    "message": "Command validation failed",
                    "details": meerkat_core::comms::CommsCommandRequest::validation_errors_to_json(
                        &details
                    ),
                })
                .to_string(),
            );
        }
    };

    match comms.send(cmd).await {
        Ok(receipt) => {
            let result = match receipt {
                meerkat_core::comms::SendReceipt::InputAccepted {
                    interaction_id,
                    stream_reserved,
                } => serde_json::json!({
                    "kind": "input_accepted",
                    "interaction_id": interaction_id.0.to_string(),
                    "stream_reserved": stream_reserved,
                }),
                meerkat_core::comms::SendReceipt::PeerMessageSent { envelope_id, acked } => {
                    serde_json::json!({
                        "kind": "peer_message_sent",
                        "envelope_id": envelope_id.to_string(),
                        "acked": acked,
                    })
                }
                meerkat_core::comms::SendReceipt::PeerRequestSent {
                    envelope_id,
                    interaction_id,
                    stream_reserved,
                } => serde_json::json!({
                    "kind": "peer_request_sent",
                    "envelope_id": envelope_id.to_string(),
                    "interaction_id": interaction_id.0.to_string(),
                    "stream_reserved": stream_reserved,
                }),
                meerkat_core::comms::SendReceipt::PeerResponseSent {
                    envelope_id,
                    in_reply_to,
                } => serde_json::json!({
                    "kind": "peer_response_sent",
                    "envelope_id": envelope_id.to_string(),
                    "in_reply_to": in_reply_to.0.to_string(),
                }),
            };
            RpcResponse::success(id, result)
        }
        Err(e) => RpcResponse::error(id, error::INTERNAL_ERROR, e.to_string()),
    }
}

/// Handle `comms/peers` — list peers visible to a session's runtime.
pub async fn handle_peers(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let params: CommsPeersParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };

    let comms = match runtime.comms_runtime(&session_id).await {
        Some(c) => c,
        None => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Session not found or comms not enabled: {session_id}"),
            );
        }
    };

    let peers = comms.peers().await;
    let entries: Vec<serde_json::Value> = peers
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name.to_string(),
                "peer_id": p.peer_id,
                "address": p.address,
                "source": format!("{:?}", p.source),
                "sendable_kinds": p.sendable_kinds,
                "capabilities": p.capabilities,
            })
        })
        .collect();

    RpcResponse::success(id, serde_json::json!({ "peers": entries }))
}

fn build_comms_command(
    params: &CommsSendParams,
    session_id: &meerkat_core::SessionId,
) -> Result<meerkat_core::comms::CommsCommand, Vec<meerkat_core::comms::CommsCommandValidationError>>
{
    let request = meerkat_core::comms::CommsCommandRequest {
        kind: params.kind.clone(),
        to: params.to.clone(),
        body: params.body.clone(),
        intent: params.intent.clone(),
        params: params.params.clone(),
        in_reply_to: params.in_reply_to.clone(),
        status: params.status.clone(),
        result: params.result.clone(),
        source: params.source.clone(),
        stream: params.stream.clone(),
        allow_self_session: params.allow_self_session,
    };
    request.parse(session_id)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_comms_send_params_deserialization() {
        let json = r#"{"session_id":"sid_1","kind":"input","body":"hello"}"#;
        let params: CommsSendParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.kind, "input");
        assert_eq!(params.body.as_deref(), Some("hello"));
    }

    #[test]
    fn test_comms_send_params_peer_message() {
        let json = r#"{"session_id":"sid_1","kind":"peer_message","to":"alice","body":"hi"}"#;
        let params: CommsSendParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.kind, "peer_message");
        assert_eq!(params.to.as_deref(), Some("alice"));
    }

    #[test]
    fn test_build_comms_command_unknown_kind() {
        let params = CommsSendParams {
            session_id: "sid_1".to_string(),
            kind: "foobar".to_string(),
            to: None,
            body: None,
            intent: None,
            params: None,
            in_reply_to: None,
            status: None,
            result: None,
            source: None,
            stream: None,
            allow_self_session: None,
        };
        let session_id = meerkat_core::SessionId::new();
        let result = build_comms_command(&params, &session_id);
        assert!(result.is_err());
        let errors = meerkat_core::comms::CommsCommandRequest::validation_errors_to_json(
            &result.unwrap_err(),
        );
        assert_eq!(errors[0]["issue"], "unknown_kind");
    }

    #[test]
    fn test_build_comms_command_input_missing_body() {
        let params = CommsSendParams {
            session_id: "sid_1".to_string(),
            kind: "input".to_string(),
            to: None,
            body: None,
            intent: None,
            params: None,
            in_reply_to: None,
            status: None,
            result: None,
            source: None,
            stream: None,
            allow_self_session: None,
        };
        let session_id = meerkat_core::SessionId::new();
        let result = build_comms_command(&params, &session_id);
        assert!(result.is_err());
        let errors = meerkat_core::comms::CommsCommandRequest::validation_errors_to_json(
            &result.unwrap_err(),
        );
        assert_eq!(errors[0]["field"], "body");
    }

    #[test]
    fn test_build_comms_command_peer_request_invalid_stream() {
        let params = CommsSendParams {
            session_id: "sid_1".to_string(),
            kind: "peer_request".to_string(),
            to: Some("alice".to_string()),
            body: None,
            intent: Some("ask".to_string()),
            params: None,
            in_reply_to: None,
            status: None,
            result: None,
            source: None,
            stream: Some("invalid".to_string()),
            allow_self_session: None,
        };
        let session_id = meerkat_core::SessionId::new();
        let result = build_comms_command(&params, &session_id);
        assert!(result.is_err());
        let errors = meerkat_core::comms::CommsCommandRequest::validation_errors_to_json(
            &result.unwrap_err(),
        );
        assert_eq!(errors[0]["field"], "stream");
        assert_eq!(errors[0]["issue"], "invalid_value");
    }

    #[test]
    fn test_build_comms_command_peer_response_invalid_status() {
        let params = CommsSendParams {
            session_id: "sid_1".to_string(),
            kind: "peer_response".to_string(),
            to: Some("alice".to_string()),
            body: None,
            intent: None,
            params: None,
            in_reply_to: Some(uuid::Uuid::new_v4().to_string()),
            status: Some("almost-done".to_string()),
            result: None,
            source: None,
            stream: None,
            allow_self_session: None,
        };
        let session_id = meerkat_core::SessionId::new();
        let result = build_comms_command(&params, &session_id);
        assert!(result.is_err());
        let errors = meerkat_core::comms::CommsCommandRequest::validation_errors_to_json(
            &result.unwrap_err(),
        );
        assert_eq!(errors[0]["field"], "status");
        assert_eq!(errors[0]["issue"], "invalid_value");
    }

    #[test]
    fn test_build_comms_command_input_invalid_source() {
        let params = CommsSendParams {
            session_id: "sid_1".to_string(),
            kind: "input".to_string(),
            to: None,
            body: Some("hi".to_string()),
            intent: None,
            params: None,
            in_reply_to: None,
            status: None,
            result: None,
            source: Some("webhookd".to_string()),
            stream: None,
            allow_self_session: None,
        };
        let session_id = meerkat_core::SessionId::new();
        let result = build_comms_command(&params, &session_id);
        assert!(result.is_err());
        let errors = meerkat_core::comms::CommsCommandRequest::validation_errors_to_json(
            &result.unwrap_err(),
        );
        assert_eq!(errors[0]["field"], "source");
        assert_eq!(errors[0]["issue"], "invalid_value");
    }
}
