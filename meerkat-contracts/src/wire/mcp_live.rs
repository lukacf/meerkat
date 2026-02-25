//! Live MCP operation wire contracts.

use serde::{Deserialize, Serialize};

/// Shared status for live MCP operations.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum McpLiveOpStatus {
    Staged,
    Applied,
    Rejected,
}

/// Shared operation kind for live MCP operations.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum McpLiveOperation {
    Add,
    Remove,
    Reload,
}

/// Request payload for `mcp/add`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct McpAddParams {
    pub session_id: String,
    pub server_name: String,
    pub server_config: serde_json::Value,
    #[serde(default)]
    pub persisted: bool,
}

/// Request payload for `mcp/remove`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct McpRemoveParams {
    pub session_id: String,
    pub server_name: String,
    #[serde(default)]
    pub persisted: bool,
}

/// Request payload for optional `mcp/reload`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct McpReloadParams {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    #[serde(default)]
    pub persisted: bool,
}

/// Response payload for live MCP operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct McpLiveOpResponse {
    pub session_id: String,
    pub operation: McpLiveOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    pub status: McpLiveOpStatus,
    pub persisted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_at_turn: Option<u32>,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn mcp_add_params_roundtrip() {
        let params = McpAddParams {
            session_id: "s_123".to_string(),
            server_name: "filesystem".to_string(),
            server_config: serde_json::json!({"cmd":"npx","args":["-y","@modelcontextprotocol/server-filesystem"]}),
            persisted: true,
        };
        let json = serde_json::to_value(&params).expect("serialize");
        let parsed: McpAddParams = serde_json::from_value(json).expect("deserialize");
        assert_eq!(parsed, params);
    }

    #[test]
    fn mcp_live_op_response_roundtrip() {
        let response = McpLiveOpResponse {
            session_id: "s_123".to_string(),
            operation: McpLiveOperation::Remove,
            server_name: Some("filesystem".to_string()),
            status: McpLiveOpStatus::Staged,
            persisted: false,
            applied_at_turn: Some(9),
        };
        let json = serde_json::to_value(&response).expect("serialize");
        let parsed: McpLiveOpResponse = serde_json::from_value(json).expect("deserialize");
        assert_eq!(parsed, response);
    }

    #[test]
    fn mcp_live_op_response_rejects_invalid_operation() {
        let json = serde_json::json!({
            "session_id": "s_123",
            "operation": "unknown",
            "server_name": "filesystem",
            "status": "staged",
            "persisted": false
        });
        let err = serde_json::from_value::<McpLiveOpResponse>(json).expect_err("must fail");
        assert!(err.to_string().contains("unknown variant"));
    }

    #[test]
    fn mcp_live_op_response_rejects_invalid_status() {
        let json = serde_json::json!({
            "session_id": "s_123",
            "operation": "add",
            "server_name": "filesystem",
            "status": "queued",
            "persisted": false
        });
        let err = serde_json::from_value::<McpLiveOpResponse>(json).expect_err("must fail");
        assert!(err.to_string().contains("unknown variant"));
    }

    #[test]
    fn mcp_add_params_rejects_malformed_payload() {
        let missing = serde_json::json!({
            "server_name": "filesystem",
            "server_config": {}
        });
        let err = serde_json::from_value::<McpAddParams>(missing).expect_err("missing session_id");
        assert!(err.to_string().contains("missing field"));

        let wrong_type = serde_json::json!({
            "session_id": "s_123",
            "server_name": "filesystem",
            "server_config": {},
            "persisted": "not-bool"
        });
        let err =
            serde_json::from_value::<McpAddParams>(wrong_type).expect_err("invalid persisted");
        assert!(err.to_string().contains("invalid type"));
    }
}
