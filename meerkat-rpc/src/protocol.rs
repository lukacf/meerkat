//! JSON-RPC 2.0 message types.
//!
//! Uses `Box<RawValue>` for params/result to avoid early parsing,
//! following the project guideline about pass-through JSON.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// JSON-RPC request or notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<RpcId>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Box<RawValue>>,
}

/// JSON-RPC message identifier (number or string).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcId {
    Num(i64),
    Str(String),
}

/// JSON-RPC response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: Option<RpcId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Box<RawValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC notification (server -> client, no id, no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: serde_json::Value,
}

impl RpcRequest {
    /// Returns true if this is a notification (no id).
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

impl RpcResponse {
    /// Construct a success response with the given result.
    pub fn success(id: Option<RpcId>, result: impl Serialize) -> Self {
        // Serialize the result to a RawValue. If serialization fails,
        // fall back to a JSON null.
        let raw = serde_json::value::to_raw_value(&result).unwrap_or_else(|_| {
            // This is extremely unlikely to fail since we're serializing
            // a type that impl Serialize, but we avoid unwrap in library code.
            serde_json::value::to_raw_value(&serde_json::Value::Null).unwrap_or_else(|_| {
                // Serializing null to RawValue cannot realistically fail.
                // We use a const approach as last resort.
                RawValue::from_string("null".to_string()).unwrap_or_default()
            })
        });
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(raw),
            error: None,
        }
    }

    /// Construct an error response.
    pub fn error(id: Option<RpcId>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    /// Construct an error response with additional data.
    pub fn error_with_data(
        id: Option<RpcId>,
        code: i32,
        message: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: Some(data),
            }),
        }
    }
}

impl RpcNotification {
    /// Construct a new notification.
    pub fn new(method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::error;

    #[test]
    fn request_roundtrip_numeric_id() {
        let json = r#"{"jsonrpc":"2.0","id":42,"method":"session/create","params":{"model":"claude-opus-4-6"}}"#;
        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(RpcId::Num(42)));
        assert_eq!(req.method, "session/create");
        assert!(req.params.is_some());

        // Roundtrip
        let serialized = serde_json::to_string(&req).unwrap();
        let req2: RpcRequest = serde_json::from_str(&serialized).unwrap();
        assert_eq!(req2.id, Some(RpcId::Num(42)));
        assert_eq!(req2.method, "session/create");
    }

    #[test]
    fn request_roundtrip_string_id() {
        let json = r#"{"jsonrpc":"2.0","id":"abc-123","method":"turn/submit","params":{}}"#;
        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, Some(RpcId::Str("abc-123".to_string())));

        let serialized = serde_json::to_string(&req).unwrap();
        let req2: RpcRequest = serde_json::from_str(&serialized).unwrap();
        assert_eq!(req2.id, Some(RpcId::Str("abc-123".to_string())));
    }

    #[test]
    fn notification_serialization_no_id() {
        let json = r#"{"jsonrpc":"2.0","method":"cancel"}"#;
        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert!(req.is_notification());
        assert!(req.id.is_none());

        // When serialized, "id" key should not appear
        let serialized = serde_json::to_string(&req).unwrap();
        assert!(!serialized.contains("\"id\""));
    }

    #[test]
    fn success_response_with_result() {
        let resp = RpcResponse::success(Some(RpcId::Num(1)), serde_json::json!({"status": "ok"}));
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(RpcId::Num(1)));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());

        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(serialized.contains("\"result\""));
        assert!(!serialized.contains("\"error\""));

        // Verify result content
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["result"]["status"], "ok");
    }

    #[test]
    fn error_response_with_code_and_message() {
        let resp = RpcResponse::error(
            Some(RpcId::Num(5)),
            error::METHOD_NOT_FOUND,
            "Method not found",
        );
        assert_eq!(resp.id, Some(RpcId::Num(5)));
        assert!(resp.result.is_none());
        let err = resp.error.as_ref().unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
        assert!(err.data.is_none());

        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(!serialized.contains("\"result\""));
        assert!(serialized.contains("\"error\""));
    }

    #[test]
    fn error_response_without_data_omits_data_field() {
        let resp = RpcResponse::error(
            Some(RpcId::Num(1)),
            error::INTERNAL_ERROR,
            "something broke",
        );
        let serialized = serde_json::to_string(&resp).unwrap();
        // "data" key should not appear when data is None
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert!(parsed["error"].get("data").is_none());
    }

    #[test]
    fn error_response_with_data() {
        let data = serde_json::json!({"detail": "missing field"});
        let resp = RpcResponse::error_with_data(
            Some(RpcId::Num(2)),
            error::INVALID_PARAMS,
            "Invalid params",
            data.clone(),
        );
        let err = resp.error.as_ref().unwrap();
        assert_eq!(err.data, Some(data));

        let serialized = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["error"]["data"]["detail"], "missing field");
    }

    #[test]
    fn rpc_id_deserializes_number() {
        let id: RpcId = serde_json::from_str("42").unwrap();
        assert_eq!(id, RpcId::Num(42));
    }

    #[test]
    fn rpc_id_deserializes_string() {
        let id: RpcId = serde_json::from_str(r#""req-1""#).unwrap();
        assert_eq!(id, RpcId::Str("req-1".to_string()));
    }

    #[test]
    fn request_with_no_params() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"session/list"}"#;
        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert!(req.params.is_none());
        assert!(!req.is_notification());

        // Roundtrip: params should not appear in serialized form
        let serialized = serde_json::to_string(&req).unwrap();
        assert!(!serialized.contains("\"params\""));
    }

    #[test]
    fn response_success_helper() {
        let resp = RpcResponse::success(
            Some(RpcId::Str("x".to_string())),
            serde_json::json!({"sessions": []}),
        );
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(RpcId::Str("x".to_string())));
        assert!(resp.error.is_none());

        let serialized = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["result"]["sessions"], serde_json::json!([]));
    }

    #[test]
    fn notification_constructor() {
        let notif = RpcNotification::new(
            "session/event",
            serde_json::json!({"type": "token", "text": "hello"}),
        );
        assert_eq!(notif.jsonrpc, "2.0");
        assert_eq!(notif.method, "session/event");
        assert_eq!(notif.params["type"], "token");
        assert_eq!(notif.params["text"], "hello");

        // Roundtrip
        let serialized = serde_json::to_string(&notif).unwrap();
        let notif2: RpcNotification = serde_json::from_str(&serialized).unwrap();
        assert_eq!(notif2.method, "session/event");
    }
}
