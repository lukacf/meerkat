use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{ArbitraryJson, JsonSchema};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Tool {
    #[serde(rename = "function")]
    Function {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// JSON Schema for tool parameters (intentionally untyped).
        parameters: JsonSchema,
    },
    #[serde(rename = "mcp")]
    Mcp(McpToolConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpToolConfig {
    pub server_label: String,
    pub server_url: Option<String>,
    pub connector_id: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub authorization: Option<String>,
    #[serde(rename = "tool_names")]
    pub allowed_tools: Option<Vec<String>>,
    pub require_approval: Option<RequireApproval>,
    pub server_description: Option<String>,
}

impl McpToolConfig {
    /// # Errors
    /// Returns an error if neither `server_url` nor `connector_id` is provided.
    #[allow(clippy::result_large_err)]
    pub fn validate(&self) -> Result<(), crate::error::Error> {
        if self.server_url.is_none() && self.connector_id.is_none() {
            return Err(crate::error::Error::InvalidClientEvent(
                "mcp tool requires server_url or connector_id".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Always,
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalFilter {
    #[serde(rename = "tool_names")]
    pub tool_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum RequireApproval {
    Mode(ApprovalMode),
    Filter(ApprovalFilter),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoiceMode {
    Auto,
    None,
    Required,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    Mode(ToolChoiceMode),
    Specific {
        #[serde(rename = "type")]
        kind: String,
        name: Option<String>,
        server_label: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema for MCP tool input (intentionally untyped).
    pub input_schema: Option<JsonSchema>,
    /// MCP annotations are free-form JSON (spec-defined extensions).
    pub annotations: Option<ArbitraryJson>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpError {
    Protocol {
        code: i32,
        message: String,
    },
    ToolExecution {
        message: String,
    },
    Http {
        code: i32,
        message: String,
    },
    #[serde(other)]
    Unknown,
}
