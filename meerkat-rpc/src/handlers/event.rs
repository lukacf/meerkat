//! Canonical `session/external_event` handler for runtime-backed external events.

use serde::Deserialize;
use serde_json::value::RawValue;
use std::sync::Arc;

use crate::handlers::runtime::to_wire_accept_result;
use crate::protocol::{RpcId, RpcResponse, bounded_collection_limit};
use crate::session_runtime::SessionRuntime;
use meerkat_contracts::{
    EventsLatestCursorParams, EventsLatestCursorResult, EventsListSinceParams,
    EventsSnapshotParams, PeerResponseTerminalStatusWire, SessionExternalEventEnvelope,
};

use super::{RpcResponseExt, parse_params, parse_session_id_for_runtime};

/// Parameters for `session/external_event`.
#[derive(Debug, Deserialize)]
pub struct ExternalEventParams {
    pub session_id: String,
    #[serde(flatten)]
    pub event: SessionExternalEventEnvelope,
}

/// Parameters for `session/peer_response_terminal`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerResponseTerminalParams {
    pub session_id: String,
    pub peer_id: meerkat_core::comms::PeerId,
    #[serde(default)]
    pub display_name: Option<meerkat_core::comms::PeerName>,
    pub request_id: meerkat_core::PeerCorrelationId,
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
            let payload_value: serde_json::Value = match serde_json::from_str(payload.get()) {
                Ok(v) => v,
                Err(err) => {
                    return RpcResponse::error(
                        id,
                        crate::error::INVALID_PARAMS,
                        format!("invalid event payload JSON: {err}"),
                    );
                }
            };
            runtime
                .accept_external_event_via_runtime(&session_id, event_type, payload_value, blocks)
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
            params.peer_id,
            params.display_name,
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

/// Handle `events/latest_cursor` through the runtime's durable replay projection.
pub async fn handle_events_latest_cursor(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: EventsLatestCursorParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    match runtime.event_latest_cursor(params.scope).await {
        Ok(Some(cursor)) => RpcResponse::success(
            id,
            EventsLatestCursorResult {
                contract_version: meerkat_contracts::ContractVersion::CURRENT,
                cursor,
            },
        ),
        Ok(None) => RpcResponse::error(
            id,
            crate::error::INVALID_REQUEST,
            "event replay is not enabled for this runtime host",
        ),
        Err(err) => RpcResponse::error(id, err.code, err.message),
    }
}

/// Handle `events/list_since` through the runtime's durable replay projection.
pub async fn handle_events_list_since(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let mut params: EventsListSinceParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    params.limit = match bounded_collection_limit(params.limit) {
        Ok(limit) => Some(limit),
        Err(message) => return RpcResponse::error(id, crate::error::INVALID_PARAMS, message),
    };

    match runtime.event_list_since(params).await {
        Ok(Some(result)) => RpcResponse::success(id, result),
        Ok(None) => RpcResponse::error(
            id,
            crate::error::INVALID_REQUEST,
            "event replay is not enabled for this runtime host",
        ),
        Err(err) => RpcResponse::error(id, err.code, err.message),
    }
}

/// Handle `events/snapshot` through the owning session service.
pub async fn handle_events_snapshot(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: EventsSnapshotParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    match runtime.event_snapshot(params.scope).await {
        Ok(Some(result)) => RpcResponse::success(id, result),
        Ok(None) => RpcResponse::error(
            id,
            crate::error::INVALID_REQUEST,
            "event replay is not enabled for this runtime host",
        ),
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
            let payload_value: serde_json::Value =
                serde_json::from_str(payload.get()).expect("payload parses");
            assert_eq!(payload_value["event"], "email");
        }
    }

    #[test]
    fn test_external_event_params_reject_variant_deserialization() {
        let json = r#"{"session_id":"sid_123","kind":"peer_response_terminal","peer_id":"00000000-0000-4000-8000-000000000161","display_name":"analyst","request_id":"00000000-0000-4000-8000-000000000162","status":"completed","result":{"token":"amber"}}"#;
        let params: ExternalEventParams = serde_json::from_str(json).unwrap();
        assert!(matches!(
            params.event,
            SessionExternalEventEnvelope::PeerResponseTerminal { .. }
        ));
        if let SessionExternalEventEnvelope::PeerResponseTerminal {
            peer_id,
            display_name,
            request_id,
            status,
            result,
            ..
        } = params.event
        {
            assert_eq!(peer_id.to_string(), "00000000-0000-4000-8000-000000000161");
            assert_eq!(display_name.unwrap().as_str(), "analyst");
            assert_eq!(
                request_id.to_string(),
                "00000000-0000-4000-8000-000000000162"
            );
            assert_eq!(status, PeerResponseTerminalStatusWire::Completed);
            let result_value: serde_json::Value =
                serde_json::from_str(result.get()).expect("result parses");
            assert_eq!(result_value["token"], "amber");
        }
    }

    #[test]
    fn test_peer_response_terminal_params_deserialization() {
        let json = r#"{"session_id":"sid_123","peer_id":"00000000-0000-4000-8000-000000000161","display_name":"analyst","request_id":"00000000-0000-4000-8000-000000000162","status":"failed","result":null}"#;
        let params: PeerResponseTerminalParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.session_id, "sid_123");
        assert_eq!(
            params.peer_id.to_string(),
            "00000000-0000-4000-8000-000000000161"
        );
        assert_eq!(params.display_name.unwrap().as_str(), "analyst");
        assert_eq!(
            params.request_id.to_string(),
            "00000000-0000-4000-8000-000000000162"
        );
        assert_eq!(params.status, PeerResponseTerminalStatusWire::Failed);
        assert_eq!(params.result, serde_json::Value::Null);
    }

    #[test]
    fn test_peer_response_terminal_params_reject_name_only_origin() {
        let json = r#"{"session_id":"sid_123","peer_name":"analyst","request_id":"00000000-0000-4000-8000-000000000162","status":"failed","result":null}"#;
        let err = serde_json::from_str::<PeerResponseTerminalParams>(json).unwrap_err();
        assert!(
            err.to_string().contains("peer_id"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_peer_response_terminal_params_rejects_mixed_peer_name_origin() {
        let json = r#"{"session_id":"sid_123","peer_id":"00000000-0000-4000-8000-000000000161","peer_name":"analyst","request_id":"00000000-0000-4000-8000-000000000162","status":"failed","result":null}"#;
        let err = serde_json::from_str::<PeerResponseTerminalParams>(json).unwrap_err();
        assert!(
            err.to_string().contains("peer_name"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_peer_response_terminal_params_reject_stringly_request_id() {
        let json = r#"{"session_id":"sid_123","peer_id":"00000000-0000-4000-8000-000000000161","request_id":"req-1","status":"failed","result":null}"#;
        let err = serde_json::from_str::<PeerResponseTerminalParams>(json).unwrap_err();
        assert!(
            err.to_string().contains("UUID") || err.to_string().contains("uuid"),
            "unexpected error: {err}"
        );
    }
}
