//! `comms/*` handlers — canonical comms command dispatch and peer discovery.

pub const COMMS_STREAM_CONTRACT_VERSION: &str = "0.3.0";

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

use super::{parse_params, parse_session_id_for_runtime};

fn is_transport_internal(message: &str) -> bool {
    message.starts_with("Transport error:") || message.starts_with("IO error:")
}

fn normalize_send_error(
    peer_name: Option<&str>,
    error: &meerkat_core::comms::SendError,
) -> serde_json::Value {
    match error {
        meerkat_core::comms::SendError::PeerNotFound(peer) => serde_json::json!({
            "code": "peer_not_found_or_not_trusted",
            "peer": peer,
            "message": format!("peer '{peer}' is not found or not trusted"),
        }),
        meerkat_core::comms::SendError::PeerOffline => {
            let peer = peer_name.unwrap_or("<unknown>");
            serde_json::json!({
                "code": "peer_unreachable",
                "peer": peer,
                "reason": "offline_or_no_ack",
                "message": format!("peer '{peer}' is unreachable: offline_or_no_ack"),
            })
        }
        meerkat_core::comms::SendError::Internal(message)
            if peer_name.is_some() && is_transport_internal(message) =>
        {
            let peer = peer_name.unwrap_or("<unknown>");
            serde_json::json!({
                "code": "peer_unreachable",
                "peer": peer,
                "reason": "transport_error",
                "message": format!("peer '{peer}' is unreachable: transport_error"),
                "details": message,
            })
        }
        other => serde_json::json!({
            "code": "send_failed",
            "message": other.to_string(),
        }),
    }
}

pub(crate) fn send_receipt_json(receipt: meerkat_core::comms::SendReceipt) -> serde_json::Value {
    match receipt {
        meerkat_core::comms::SendReceipt::InputAccepted {
            interaction_id,
            stream_reserved,
        } => {
            serde_json::json!({
                "kind": "input_accepted",
                "interaction_id": interaction_id.0.to_string(),
                "stream_reserved": stream_reserved,
            })
        }
        meerkat_core::comms::SendReceipt::PeerMessageSent { envelope_id, acked } => {
            serde_json::json!({
                "kind": "peer_message_sent",
                "envelope_id": envelope_id.to_string(),
                "acked": acked,
            })
        }
        meerkat_core::comms::SendReceipt::PeerLifecycleSent { envelope_id } => {
            serde_json::json!({
                "kind": "peer_lifecycle_sent",
                "envelope_id": envelope_id.to_string(),
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
            "request_id": envelope_id.to_string(),
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
    }
}

/// Parameters for `comms/send`.
///
/// `command` carries the typed [`CommsCommandRequest`] enum (serde-tagged on
/// `kind`). The wire shape is flat: `{"session_id": "...", "kind": "...", ...}`.
#[derive(Debug, Deserialize)]
pub struct CommsSendParams {
    pub session_id: String,
    #[serde(flatten)]
    pub command: meerkat_core::comms::CommsCommandRequest,
}

impl CommsSendParams {
    /// Recipient peer name for error normalization, if the command targets one.
    pub fn peer_name(&self) -> Option<&str> {
        use meerkat_core::comms::CommsCommandRequest;
        match &self.command {
            CommsCommandRequest::Input { .. } => None,
            CommsCommandRequest::PeerMessage { to, .. }
            | CommsCommandRequest::PeerLifecycle { to, .. }
            | CommsCommandRequest::PeerRequest { to, .. }
            | CommsCommandRequest::PeerResponse { to, .. } => Some(to.as_str()),
        }
    }
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

    let peer_name = params.peer_name().map(str::to_string);
    let cmd = match params.command.into_command(&session_id) {
        Ok(cmd) => cmd,
        Err(err) => {
            return RpcResponse::error_with_data(
                id,
                error::INVALID_PARAMS,
                "Command validation failed",
                serde_json::json!({
                    "code": "invalid_command",
                    "message": err.to_string(),
                }),
            );
        }
    };

    match comms.send(cmd).await {
        Ok(receipt) => RpcResponse::success(id, send_receipt_json(receipt)),
        Err(e) => {
            let normalized = normalize_send_error(peer_name.as_deref(), &e);
            let message = normalized
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Comms send failed")
                .to_string();
            RpcResponse::error_with_data(id, error::INTERNAL_ERROR, message, normalized)
        }
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
                "reachability": p.reachability,
                "last_unreachable_reason": p.last_unreachable_reason,
                "meta": p.meta,
            })
        })
        .collect();

    RpcResponse::success(id, serde_json::json!({ "peers": entries }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::comms::CommsCommandRequest;

    #[test]
    fn deserialize_input_command() {
        let json = r#"{"session_id":"sid_1","kind":"input","body":"hello"}"#;
        let params: CommsSendParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.session_id, "sid_1");
        assert!(matches!(params.command, CommsCommandRequest::Input { .. }));
    }

    #[test]
    fn deserialize_unknown_kind_fails_at_serde_boundary() {
        let json = r#"{"session_id":"sid_1","kind":"foobar"}"#;
        let err = serde_json::from_str::<CommsSendParams>(json).unwrap_err();
        assert!(
            err.to_string().contains("foobar") || err.to_string().contains("variant"),
            "expected unknown-variant serde error, got: {err}"
        );
    }

    #[test]
    fn deserialize_input_missing_body_fails_at_serde_boundary() {
        let json = r#"{"session_id":"sid_1","kind":"input"}"#;
        let err = serde_json::from_str::<CommsSendParams>(json).unwrap_err();
        assert!(
            err.to_string().contains("body"),
            "expected missing-body serde error, got: {err}"
        );
    }

    #[test]
    fn deserialize_input_invalid_source_fails_at_serde_boundary() {
        let json = r#"{"session_id":"sid_1","kind":"input","body":"hi","source":"webhookd"}"#;
        let err = serde_json::from_str::<CommsSendParams>(json).unwrap_err();
        assert!(
            err.to_string().contains("source") || err.to_string().contains("webhookd"),
            "expected invalid-source serde error, got: {err}"
        );
    }

    #[test]
    fn deserialize_peer_response_invalid_status_fails_at_serde_boundary() {
        let json = r#"{"session_id":"sid_1","kind":"peer_response","to":"alice","in_reply_to":"00000000-0000-0000-0000-000000000000","status":"almost-done"}"#;
        let err = serde_json::from_str::<CommsSendParams>(json).unwrap_err();
        assert!(
            err.to_string().contains("status") || err.to_string().contains("almost-done"),
            "expected invalid-status serde error, got: {err}"
        );
    }

    #[test]
    fn handler_send_receipt_json_peer_request_uses_envelope_id_as_request_id() {
        let envelope_id = uuid::Uuid::new_v4();
        let interaction_id = meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4());

        let payload = send_receipt_json(meerkat_core::comms::SendReceipt::PeerRequestSent {
            envelope_id,
            interaction_id,
            stream_reserved: true,
        });

        assert_eq!(
            payload["request_id"],
            serde_json::json!(envelope_id.to_string())
        );
        assert_eq!(
            payload["interaction_id"],
            serde_json::json!(interaction_id.0.to_string())
        );
    }
}
