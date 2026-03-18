//! Canonical `session/external_event` handler for runtime-backed external events.

use serde::Deserialize;
use serde_json::value::RawValue;
use std::sync::Arc;

use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

use super::{parse_params, parse_session_id_for_runtime};

/// Parameters for `session/external_event`.
#[derive(Deserialize)]
pub struct ExternalEventParams {
    pub session_id: String,
    pub payload: serde_json::Value,
    pub source: Option<String>,
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

    let source_name = params.source.unwrap_or_else(|| "rpc".to_string());
    match runtime
        .accept_external_event_via_runtime(&session_id, params.payload, source_name)
        .await
    {
        Ok(outcome) => RpcResponse::success(id, outcome),
        Err(err) => RpcResponse::error(id, err.code, err.message),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_external_event_params_deserialization() {
        let json = r#"{"session_id":"sid_123","payload":{"event":"email","from":"john"},"source":"github"}"#;
        let params: ExternalEventParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.session_id, "sid_123");
        assert_eq!(params.source.as_deref(), Some("github"));
        assert_eq!(params.payload["event"], "email");
    }

    #[test]
    fn test_external_event_params_without_source() {
        let json = r#"{"session_id":"sid_123","payload":"hello"}"#;
        let params: ExternalEventParams = serde_json::from_str(json).unwrap();
        assert!(params.source.is_none());
    }
}
