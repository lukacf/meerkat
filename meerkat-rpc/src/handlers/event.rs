//! `event/push` handler — push external events into a session's inbox.

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

use super::parse_params;

/// Parameters for `event/push`.
#[derive(Deserialize)]
pub struct PushEventParams {
    pub session_id: String,
    pub payload: Box<RawValue>,
    pub source: Option<String>,
}

/// Handle `event/push` — inject an external event into a session's inbox.
///
/// The event is queued for processing at the next turn boundary (host mode drain).
/// Does NOT trigger an immediate LLM call.
pub async fn handle_push(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let params: PushEventParams = match parse_params(params) {
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

    // Format the event body with source metadata
    let body = if let Some(ref source) = params.source {
        format!("[EVENT from {}] {}", source, params.payload.get())
    } else {
        format!("[EVENT] {}", params.payload.get())
    };

    // Get the event injector for this session
    let injector = match runtime.event_injector(&session_id).await {
        Some(i) => i,
        None => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Session not found: {session_id}"),
            );
        }
    };

    // Inject the event
    match injector.inject(body, meerkat_core::PlainEventSource::Rpc) {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"queued": true})),
        Err(meerkat_core::EventInjectorError::Full) => {
            RpcResponse::error(id, error::INTERNAL_ERROR, "Event inbox is full")
        }
        Err(meerkat_core::EventInjectorError::Closed) => {
            RpcResponse::error(id, error::INTERNAL_ERROR, "Session has been shut down")
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_push_event_params_deserialization() {
        let json = r#"{"session_id":"sid_123","payload":{"event":"email","from":"john"},"source":"github"}"#;
        let params: PushEventParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.session_id, "sid_123");
        assert_eq!(params.source.as_deref(), Some("github"));
        assert!(params.payload.get().contains("email"));
    }

    #[test]
    fn test_push_event_params_without_source() {
        let json = r#"{"session_id":"sid_123","payload":"hello"}"#;
        let params: PushEventParams = serde_json::from_str(json).unwrap();
        assert!(params.source.is_none());
    }

    #[test]
    fn test_event_body_formatting_with_source() {
        let source = Some("github".to_string());
        let payload_raw = r#"{"pr":42}"#;
        let body = if let Some(ref s) = source {
            format!("[EVENT from {}] {}", s, payload_raw)
        } else {
            format!("[EVENT] {}", payload_raw)
        };
        assert_eq!(body, r#"[EVENT from github] {"pr":42}"#);
    }

    #[test]
    fn test_event_body_formatting_without_source() {
        let source: Option<String> = None;
        let payload_raw = r#""hello""#;
        let body = if let Some(ref s) = source {
            format!("[EVENT from {}] {}", s, payload_raw)
        } else {
            format!("[EVENT] {}", payload_raw)
        };
        assert_eq!(body, r#"[EVENT] "hello""#);
    }
}
