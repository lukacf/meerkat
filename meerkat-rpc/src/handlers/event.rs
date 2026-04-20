//! Canonical `session/external_event` handler for runtime-backed external events.

use serde::Deserialize;
use serde_json::value::RawValue;
use std::sync::Arc;

use crate::handlers::runtime::to_wire_accept_result;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;
use meerkat_contracts::{PeerResponseTerminalStatusWire, SessionExternalEventEnvelope};

use super::{parse_params, parse_session_id_for_runtime};

/// Parameters for `session/external_event`.
#[derive(Debug, Deserialize)]
pub struct ExternalEventParams {
    pub session_id: String,
    #[serde(flatten)]
    pub event: SessionExternalEventEnvelope,
}

/// Parameters for `session/peer_response_terminal`.
#[derive(Debug, Deserialize)]
pub struct PeerResponseTerminalParams {
    pub session_id: String,
    pub peer_name: meerkat_core::comms::PeerName,
    pub request_id: String,
    pub status: PeerResponseTerminalStatusWire,
    pub result: serde_json::Value,
}

/// Handle the canonical high-level JSON-RPC external-event method through the
/// runtime-backed admission path.
pub async fn handle_external_event(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ExternalEventParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, &runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };

    let outcome = match params.event {
        SessionExternalEventEnvelope::GenericJson {
            event_type,
            payload,
            blocks,
        } => {
            runtime
                .accept_external_event_via_runtime(&session_id, event_type, payload, blocks)
                .await
        }
        SessionExternalEventEnvelope::PeerResponseTerminal { .. } => {
            return RpcResponse::error(
                id,
                crate::error::INVALID_PARAMS,
                "peer_response_terminal is reserved on session/external_event; use session/peer_response_terminal",
            );
        }
    };

    match outcome {
        Ok(outcome) => match to_wire_accept_result(outcome) {
            Ok(result) => RpcResponse::success(id, result),
            Err(message) => RpcResponse::error(id, crate::error::INTERNAL_ERROR, message),
        },
        Err(err) => RpcResponse::error(id, err.code, err.message),
    }
}

/// Handle `session/peer_response_terminal` through the typed runtime-backed
/// admission path.
pub async fn handle_peer_response_terminal(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: PeerResponseTerminalParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, &runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };

    let outcome = runtime
        .accept_peer_response_terminal_via_runtime(
            &session_id,
            params.peer_name,
            params.request_id,
            params.status,
            params.result,
        )
        .await;

    match outcome {
        Ok(outcome) => match to_wire_accept_result(outcome) {
            Ok(result) => RpcResponse::success(id, result),
            Err(message) => RpcResponse::error(id, crate::error::INTERNAL_ERROR, message),
        },
        Err(err) => RpcResponse::error(id, err.code, err.message),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_external_event_params_deserialization() {
        let json = r#"{"session_id":"sid_123","kind":"generic_json","event_type":"github","payload":{"event":"email","from":"john"}}"#;
        let params: ExternalEventParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.session_id, "sid_123");
        assert!(matches!(
            params.event,
            SessionExternalEventEnvelope::GenericJson { .. }
        ));
        if let SessionExternalEventEnvelope::GenericJson {
            event_type,
            payload,
            ..
        } = params.event
        {
            assert_eq!(event_type, "github");
            assert_eq!(payload["event"], "email");
        }
    }

    #[test]
    fn test_external_event_params_reject_variant_deserialization() {
        let json = r#"{"session_id":"sid_123","kind":"peer_response_terminal","peer_name":"analyst","request_id":"req-1","status":"completed","result":{"token":"amber"}}"#;
        let params: ExternalEventParams = serde_json::from_str(json).unwrap();
        assert!(matches!(
            params.event,
            SessionExternalEventEnvelope::PeerResponseTerminal { .. }
        ));
        if let SessionExternalEventEnvelope::PeerResponseTerminal {
            peer_name,
            request_id,
            status,
            result,
        } = params.event
        {
            assert_eq!(peer_name.as_str(), "analyst");
            assert_eq!(request_id, "req-1");
            assert_eq!(status, PeerResponseTerminalStatusWire::Completed);
            assert_eq!(result["token"], "amber");
        }
    }

    #[test]
    fn test_peer_response_terminal_params_deserialization() {
        let json = r#"{"session_id":"sid_123","peer_name":"analyst","request_id":"req-1","status":"failed","result":null}"#;
        let params: PeerResponseTerminalParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.session_id, "sid_123");
        assert_eq!(params.peer_name.as_str(), "analyst");
        assert_eq!(params.request_id, "req-1");
        assert_eq!(params.status, PeerResponseTerminalStatusWire::Failed);
        assert_eq!(params.result, serde_json::Value::Null);
    }
}
