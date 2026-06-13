//! `comms/*` handlers — canonical comms command dispatch and peer discovery.

pub const COMMS_STREAM_CONTRACT_VERSION: &str = "0.3.0";

use serde_json::value::RawValue;

use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

use super::{parse_params, parse_session_id_for_runtime};

use meerkat_contracts::{
    CommsPeerUnreachableReason, CommsPeersResult, CommsSendErrorData, CommsSendResult,
};
pub use meerkat_contracts::{CommsPeersParams, CommsSendParams};

/// Project a typed comms `SendError` into the generated wire error-data
/// contract. The error taxonomy is owned by `meerkat-contracts`
/// (`CommsSendErrorData`), not hand-shaped JSON (K17).
pub(crate) fn normalize_send_error(
    peer_name: Option<&str>,
    error: &meerkat_core::comms::SendError,
) -> CommsSendErrorData {
    match error {
        meerkat_core::comms::SendError::PeerNotFound(peer) => {
            CommsSendErrorData::PeerNotFoundOrNotTrusted {
                peer: peer.clone(),
                message: format!("peer '{peer}' is not found or not trusted"),
            }
        }
        meerkat_core::comms::SendError::PeerOffline => {
            let peer = peer_name.unwrap_or("<unknown>");
            CommsSendErrorData::PeerUnreachable {
                peer: peer.to_string(),
                reason: CommsPeerUnreachableReason::OfflineOrNoAck,
                message: format!("peer '{peer}' is unreachable: offline_or_no_ack"),
                details: None,
            }
        }
        meerkat_core::comms::SendError::Transport(details) if peer_name.is_some() => {
            let peer = peer_name.unwrap_or("<unknown>");
            CommsSendErrorData::PeerUnreachable {
                peer: peer.to_string(),
                reason: CommsPeerUnreachableReason::TransportError,
                message: format!("peer '{peer}' is unreachable: transport_error"),
                details: Some(details.clone()),
            }
        }
        meerkat_core::comms::SendError::AdmissionDropped { reason } => {
            let peer = peer_name.unwrap_or("<unknown>");
            CommsSendErrorData::PeerAdmissionDropped {
                peer: peer.to_string(),
                reason: *reason,
                message: format!(
                    "peer '{peer}' rejected envelope at ingress: {}",
                    reason.as_code()
                ),
            }
        }
        other => CommsSendErrorData::SendFailed {
            message: other.to_string(),
        },
    }
}

pub(crate) fn send_receipt_json(
    receipt: meerkat_core::comms::SendReceipt,
) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(CommsSendResult::from(receipt))
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

    let session_id = match parse_session_id_for_runtime(id.clone(), params.session_id(), runtime) {
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

    let peer_name = params.peer_label();
    let cmd = match params.into_command().into_command(&session_id) {
        Ok(cmd) => cmd,
        Err(err) => {
            let data = CommsSendErrorData::InvalidCommand {
                message: err.to_string(),
            };
            return match serde_json::to_value(&data) {
                Ok(data) => RpcResponse::error_with_data(
                    id,
                    error::INVALID_PARAMS,
                    "Command validation failed",
                    data,
                ),
                Err(serialize_error) => RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("failed to serialize comms error data: {serialize_error}"),
                ),
            };
        }
    };

    match comms.send(cmd).await {
        Ok(receipt) => match send_receipt_json(receipt) {
            Ok(value) => RpcResponse::success(id, value),
            Err(serialize_error) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("failed to serialize comms send receipt: {serialize_error}"),
            ),
        },
        Err(e) => {
            let normalized = normalize_send_error(peer_name.as_deref(), &e);
            let message = normalized.message().to_string();
            match serde_json::to_value(&normalized) {
                Ok(data) => RpcResponse::error_with_data(id, error::INTERNAL_ERROR, message, data),
                Err(serialize_error) => RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("failed to serialize comms error data: {serialize_error}"),
                ),
            }
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
    let result = CommsPeersResult::from_entries(&peers);

    match serde_json::to_value(result) {
        Ok(value) => RpcResponse::success(id, value),
        Err(serialize_error) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("Serialize error: {serialize_error}"),
        ),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_contracts::CommsCommandRequest;
    use meerkat_core::comms::{
        PeerAddress, PeerCapabilitySet, PeerDirectoryEntry, PeerDirectorySource, PeerId, PeerName,
        PeerSendability, PeerTransport,
    };

    #[test]
    fn deserialize_input_command() {
        let json = r#"{"session_id":"sid_1","kind":"input","body":"hello"}"#;
        let params: CommsSendParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.session_id(), "sid_1");
        assert!(matches!(
            params.into_command(),
            CommsCommandRequest::Input { .. }
        ));
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
    fn deserialize_peer_request_unknown_intent_fails_at_serde_boundary() {
        let json = format!(
            r#"{{"session_id":"sid_1","kind":"peer_request","to":"{}","intent":"not.generated","params":{{}}}}"#,
            uuid::Uuid::new_v4()
        );
        let err = serde_json::from_str::<CommsSendParams>(&json).unwrap_err();
        assert!(
            err.to_string().contains("not.generated") || err.to_string().contains("variant"),
            "expected unknown-intent serde error, got: {err}"
        );
    }

    #[test]
    fn deserialize_peer_response_malformed_result_fails_at_serde_boundary() {
        let json = format!(
            r#"{{"session_id":"sid_1","kind":"peer_response","to":"{}","in_reply_to":"{}","status":"completed","result":{{"result":"ack","ok":"yes"}}}}"#,
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4()
        );
        let err = serde_json::from_str::<CommsSendParams>(&json).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("ok")
                || message.contains("invalid type")
                || message.contains("did not match any variant"),
            "expected typed-result serde error, got: {message}"
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
    fn deserialize_input_unknown_field_fails_at_serde_boundary() {
        let json = r#"{"session_id":"sid_1","kind":"input","body":"hi","unexpected":true}"#;
        let err = serde_json::from_str::<CommsSendParams>(json).unwrap_err();
        assert!(
            err.to_string().contains("unexpected") || err.to_string().contains("unknown field"),
            "expected unknown-field serde error, got: {err}"
        );
    }

    #[test]
    fn deserialize_peer_response_invalid_status_fails_at_serde_boundary() {
        let json = format!(
            r#"{{"session_id":"sid_1","kind":"peer_response","to":"{}","in_reply_to":"{}","status":"almost-done"}}"#,
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4()
        );
        let err = serde_json::from_str::<CommsSendParams>(&json).unwrap_err();
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
        })
        .expect("well-formed receipt serializes");

        assert_eq!(
            payload["request_id"],
            serde_json::json!(envelope_id.to_string())
        );
        assert_eq!(
            payload["interaction_id"],
            serde_json::json!(interaction_id.0.to_string())
        );
    }

    #[test]
    fn normalize_send_error_preserves_admission_drop_reason() {
        let data = normalize_send_error(
            Some("peer-a"),
            &meerkat_core::comms::SendError::AdmissionDropped {
                reason: meerkat_core::comms::AdmissionDropReason::InboxFull,
            },
        );

        assert_eq!(
            data.message(),
            "peer 'peer-a' rejected envelope at ingress: inbox_full"
        );
        let json = serde_json::to_value(data).unwrap();
        assert_eq!(json["code"], "peer_admission_dropped");
        assert_eq!(json["peer"], "peer-a");
        assert_eq!(json["reason"], "inbox_full");
    }

    // Gate (#62): `send_receipt_json` returns a typed `Result` and no longer
    // launders a serialization failure into a fabricated empty `{}` object.
    // A well-formed receipt serializes to a real payload; the contract change
    // (Result, not a `{}` fallback) is what lets `handle_send` surface a typed
    // JSON-RPC error instead of a success-with-{} on serialize failure.
    #[test]
    fn gate_send_receipt_json_returns_result_not_empty_object_fallback() {
        let envelope_id = uuid::Uuid::new_v4();
        let interaction_id = meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4());

        let result = send_receipt_json(meerkat_core::comms::SendReceipt::PeerRequestSent {
            envelope_id,
            interaction_id,
            stream_reserved: true,
        });

        let payload = result.expect("well-formed receipt serializes");
        // The success payload must be the real receipt, never the old `{}`
        // fallback that the OLD `unwrap_or_else` would have produced.
        assert_ne!(
            payload,
            serde_json::Value::Object(serde_json::Map::new()),
            "send_receipt_json must not fall back to an empty object"
        );
        assert_eq!(
            payload["request_id"],
            serde_json::json!(envelope_id.to_string())
        );
    }

    #[test]
    fn rpc_peer_directory_response_uses_typed_core_wire_contract() {
        let result = CommsPeersResult::from_entries(&[sample_peer_directory_entry()]);
        let value = serde_json::to_value(result).unwrap();

        assert_peer_directory_wire(&value);
    }

    fn sample_peer_directory_entry() -> PeerDirectoryEntry {
        PeerDirectoryEntry {
            peer_id: PeerId::new(),
            name: PeerName::new("agent").unwrap(),
            address: PeerAddress::new(PeerTransport::Inproc, "agent"),
            source: PeerDirectorySource::Inproc,
            sendable_kinds: vec![PeerSendability::PeerMessage, PeerSendability::PeerRequest],
            capabilities: PeerCapabilitySet::default()
                .with_extension("vendor.echo", serde_json::json!({ "enabled": true })),
            meta: meerkat_core::PeerMeta::default(),
        }
    }

    fn assert_peer_directory_wire(result: &serde_json::Value) {
        let peer = &result["peers"][0];

        assert_eq!(peer["source"], "inproc");
        assert_eq!(
            peer["sendable_kinds"],
            serde_json::json!(["peer_message", "peer_request"])
        );
        assert_eq!(peer["capabilities"]["version"], 1);
        assert_eq!(
            peer["capabilities"]["extensions"]["vendor.echo"]["enabled"],
            true
        );
    }
}
