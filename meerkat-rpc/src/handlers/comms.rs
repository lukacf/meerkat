//! `comms/*` handlers — canonical comms command dispatch and peer discovery.

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

use super::parse_params;

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

    let session_id = match meerkat_core::SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Invalid session ID: {}", params.session_id),
            );
        }
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
                    "details": details,
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
                meerkat_core::comms::SendReceipt::PeerMessageSent {
                    envelope_id,
                    acked,
                } => serde_json::json!({
                    "kind": "peer_message_sent",
                    "envelope_id": envelope_id.to_string(),
                    "acked": acked,
                }),
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

    let session_id = match meerkat_core::SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Invalid session ID: {}", params.session_id),
            );
        }
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
                "name": p.name.0,
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

/// Build a `CommsCommand` from the validated wire params.
fn build_comms_command(
    params: &CommsSendParams,
    session_id: &meerkat_core::SessionId,
) -> Result<meerkat_core::comms::CommsCommand, Vec<serde_json::Value>> {
    use meerkat_core::comms::*;

    let mut errors = Vec::new();

    match params.kind.as_str() {
        "input" => {
            let body = match &params.body {
                Some(b) => b.clone(),
                None => {
                    errors.push(serde_json::json!({
                        "field": "body", "issue": "required_field"
                    }));
                    String::new()
                }
            };
            if !errors.is_empty() {
                return Err(errors);
            }
            let source = match params.source.as_deref() {
                Some("tcp") => InputSource::Tcp,
                Some("uds") => InputSource::Uds,
                Some("stdin") => InputSource::Stdin,
                Some("webhook") => InputSource::Webhook,
                Some("rpc") | None => InputSource::Rpc,
                Some(other) => {
                    errors.push(serde_json::json!({
                        "field": "source", "issue": "invalid_value", "got": other
                    }));
                    return Err(errors);
                }
            };
            let stream = match params.stream.as_deref() {
                Some("reserve_interaction") => InputStreamMode::ReserveInteraction,
                Some("none") | None => InputStreamMode::None,
                Some(other) => {
                    errors.push(serde_json::json!({
                        "field": "stream", "issue": "invalid_value", "got": other
                    }));
                    return Err(errors);
                }
            };
            Ok(CommsCommand::Input {
                session_id: session_id.clone(),
                body,
                source,
                stream,
                allow_self_session: params.allow_self_session.unwrap_or(false),
            })
        }
        "peer_message" => {
            let to = match &params.to {
                Some(t) => PeerName(t.clone()),
                None => {
                    errors.push(serde_json::json!({
                        "field": "to", "issue": "required_field"
                    }));
                    return Err(errors);
                }
            };
            let body = params.body.clone().unwrap_or_default();
            Ok(CommsCommand::PeerMessage { to, body })
        }
        "peer_request" => {
            let to = match &params.to {
                Some(t) => PeerName(t.clone()),
                None => {
                    errors.push(serde_json::json!({
                        "field": "to", "issue": "required_field"
                    }));
                    return Err(errors);
                }
            };
            let intent = match &params.intent {
                Some(i) => i.clone(),
                None => {
                    errors.push(serde_json::json!({
                        "field": "intent", "issue": "required_field"
                    }));
                    return Err(errors);
                }
            };
            if !errors.is_empty() {
                return Err(errors);
            }
            let p = params.params.clone().unwrap_or(serde_json::json!({}));
            let stream = match params.stream.as_deref() {
                Some("reserve_interaction") => InputStreamMode::ReserveInteraction,
                Some("none") | None => InputStreamMode::None,
                Some(other) => {
                    errors.push(serde_json::json!({
                        "field": "stream", "issue": "invalid_value", "got": other
                    }));
                    return Err(errors);
                }
            };
            Ok(CommsCommand::PeerRequest {
                to,
                intent,
                params: p,
                stream,
            })
        }
        "peer_response" => {
            let to = match &params.to {
                Some(t) => PeerName(t.clone()),
                None => {
                    errors.push(serde_json::json!({
                        "field": "to", "issue": "required_field"
                    }));
                    return Err(errors);
                }
            };
            let in_reply_to = match &params.in_reply_to {
                Some(id_str) => match uuid::Uuid::parse_str(id_str) {
                    Ok(id) => meerkat_core::InteractionId(id),
                    Err(_) => {
                        errors.push(serde_json::json!({
                            "field": "in_reply_to", "issue": "invalid_uuid", "got": id_str
                        }));
                        return Err(errors);
                    }
                },
                None => {
                    errors.push(serde_json::json!({
                        "field": "in_reply_to", "issue": "required_field"
                    }));
                    return Err(errors);
                }
            };
            let status = match params.status.as_deref() {
                Some("accepted") => meerkat_core::ResponseStatus::Accepted,
                Some("completed") | None => meerkat_core::ResponseStatus::Completed,
                Some("failed") => meerkat_core::ResponseStatus::Failed,
                Some(other) => {
                    errors.push(serde_json::json!({
                        "field": "status", "issue": "invalid_value", "got": other
                    }));
                    return Err(errors);
                }
            };
            if !errors.is_empty() {
                return Err(errors);
            }
            let result = params.result.clone().unwrap_or(serde_json::json!(null));
            Ok(CommsCommand::PeerResponse {
                to,
                in_reply_to,
                status,
                result,
            })
        }
        other => {
            errors.push(serde_json::json!({
                "field": "kind", "issue": "unknown_kind", "got": other
            }));
            Err(errors)
        }
    }
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
        let errors = result.unwrap_err();
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
        let errors = result.unwrap_err();
        assert_eq!(errors[0]["field"], "body");
    }
}
