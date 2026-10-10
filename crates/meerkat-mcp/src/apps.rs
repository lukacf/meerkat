//! MCP Apps protocol data retained for the host, never projected into model input.
//!
//! App authors supply only standard MCP tools and resources. These records are
//! the runtime's durable association between a tool result and its registration.

use meerkat_core::{ToolAudience, ToolDef};
use rmcp::model::{CallToolResult, Tool};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MCP_APPS_EXTENSION: &str = "io.modelcontextprotocol/ui";
pub const MCP_APP_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";

/// Select only when a runtime has an actual MCP Apps presentation host.
pub struct McpAppsClientServiceFactory;

struct AppsClient;

impl rmcp::ClientHandler for AppsClient {
    fn get_info(&self) -> rmcp::model::ClientInfo {
        let mut info = rmcp::model::ClientInfo::default();
        info.capabilities.extensions = Some(
            [(
                MCP_APPS_EXTENSION.into(),
                serde_json::json!({
                    "mimeTypes": [MCP_APP_RESOURCE_MIME_TYPE]
                })
                .as_object()
                .cloned()
                .unwrap_or_default(),
            )]
            .into(),
        );
        info
    }
}

impl crate::McpClientServiceFactory for McpAppsClientServiceFactory {
    fn create(
        &self,
        _config: &meerkat_core::McpServerConfig,
    ) -> Result<Box<dyn rmcp::service::DynService<rmcp::RoleClient>>, crate::McpError> {
        use rmcp::service::ServiceExt;
        Ok(AppsClient.into_dyn())
    }
}

/// Stored on the original committed ToolResult's host-only extension carrier.
/// A caller-supplied copy is never accepted as evidence of an invocation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpAppInvocation {
    pub registration: McpAppRegistration,
    pub tool: Tool,
    pub arguments: Value,
    pub result: CallToolResult,
    /// Original resource bytes, when captured, remain available for historical
    /// display after the connection is retired. They never authorize fresh IO.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<rmcp::model::ReadResourceResult>,
}

/// The original physical connection. A new connection under the same server
/// name is a different registration for IO, including after process restart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpAppRegistration {
    pub server: String,
    pub connection: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct McpAppCallBinding {
    pub source_tool: String,
    pub invocation: McpAppInvocation,
    pub target: String,
}

pub(crate) fn project_app_result(
    result: &meerkat_core::types::ToolResult,
) -> Result<Value, meerkat_core::ToolError> {
    result
        .host_metadata
        .get(MCP_APPS_EXTENSION)
        .and_then(|value| serde_json::from_value::<McpAppInvocation>(value.clone()).ok())
        .and_then(|invocation| serde_json::to_value(invocation.result).ok())
        .ok_or_else(|| meerkat_core::ToolError::execution_failed(result.text_content()))
}

impl McpAppRegistration {
    pub(crate) fn from_connection(connection: &crate::McpConnection) -> Self {
        static PROCESS: std::sync::LazyLock<uuid::Uuid> =
            std::sync::LazyLock::new(uuid::Uuid::new_v4);
        Self {
            server: connection.config().name.clone(),
            connection: format!("{}:{}", *PROCESS, connection.connection_id()),
        }
    }
}

/// Standard tool UI metadata. Unknown or malformed audiences never grant app
/// access. An omitted visibility field has the MCP Apps model+app default.
pub(crate) fn tool_audience(tool: &Tool) -> Result<ToolAudience, crate::McpError> {
    let Some(ui) = tool.meta.as_ref().and_then(|meta| meta.0.get("ui")) else {
        return Ok(ToolAudience::ModelAndApp);
    };
    let Some(ui) = ui.as_object() else {
        return Err(invalid_ui());
    };
    let Some(visibility) = ui.get("visibility") else {
        return Ok(ToolAudience::ModelAndApp);
    };
    let Some(visibility) = visibility.as_array() else {
        return Err(invalid_ui());
    };
    let mut model = false;
    let mut app = false;
    for audience in visibility {
        match audience.as_str() {
            Some("model") => model = true,
            Some("app") => app = true,
            _ => return Err(invalid_ui()),
        }
    }
    match (model, app) {
        (true, true) => Ok(ToolAudience::ModelAndApp),
        (true, false) => Ok(ToolAudience::Model),
        (false, true) => Ok(ToolAudience::App),
        (false, false) => Ok(ToolAudience::Hidden),
    }
}

pub fn tool_ui_resource_uri(tool: &Tool) -> Option<&str> {
    let meta = &tool.meta.as_ref()?.0;
    // Canonical metadata wins even when invalid; the deprecated value must
    // not silently substitute a different resource selected by the server.
    let resource = meta
        .get("ui")
        .and_then(|ui| ui.get("resourceUri"))
        .or_else(|| meta.get("ui/resourceUri"))?;
    resource
        .as_str()
        .filter(|uri| uri.starts_with("ui://") && uri.len() > "ui://".len())
}

fn invalid_ui() -> crate::McpError {
    crate::McpError::ProtocolError {
        message: "invalid MCP Apps tool metadata".into(),
    }
}

pub(crate) fn project_tool(tool: &Tool, server: &str) -> Result<ToolDef, crate::McpError> {
    Ok(ToolDef::new(
        tool.name.to_string(),
        tool.description.as_deref().unwrap_or_default().to_string(),
        Value::Object((*tool.input_schema).clone()),
    )
    .with_provenance(meerkat_core::types::ToolProvenance {
        kind: meerkat_core::types::ToolSourceKind::Mcp,
        source_id: server.into(),
    })
    .with_audience(tool_audience(tool)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projects_standard_ui_visibility_without_model_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        let tool: Tool = serde_json::from_value(json!({
            "name": "refresh", "inputSchema": {"type": "object"},
            "_meta": {"ui": {"visibility": ["app"], "resourceUri": "ui://test/view"},
                      "private": "host only"}
        }))?;
        let projected = project_tool(&tool, "test")?;
        assert_eq!(projected.audience, ToolAudience::App);
        assert!(!serde_json::to_string(&projected)?.contains("host only"));
        assert_eq!(tool_ui_resource_uri(&tool), Some("ui://test/view"));
        Ok(())
    }

    #[test]
    fn rejects_invalid_explicit_visibility() -> Result<(), serde_json::Error> {
        for visibility in [json!(["unknown"]), json!("app"), json!(["app", 1])] {
            let tool: Tool = serde_json::from_value(json!({
                "name": "test", "inputSchema": {}, "_meta": {"ui": {"visibility": visibility}}
            }))?;
            assert!(tool_audience(&tool).is_err());
        }
        Ok(())
    }

    #[test]
    fn empty_visibility_hides_only_that_tool_from_both_audiences()
    -> Result<(), Box<dyn std::error::Error>> {
        let tool: Tool = serde_json::from_value(json!({
            "name": "hidden", "inputSchema": {}, "_meta": {"ui": {"visibility": []}}
        }))?;
        let projected = project_tool(&tool, "test")?;
        assert_eq!(projected.audience, ToolAudience::Hidden);
        assert!(!projected.audience.allows_model());
        assert!(!projected.audience.allows_app());
        Ok(())
    }

    #[test]
    fn supports_deprecated_flat_resource_uri_but_prefers_canonical_metadata()
    -> Result<(), serde_json::Error> {
        for (meta, expected) in [
            (
                json!({"ui/resourceUri":"ui://test/legacy"}),
                Some("ui://test/legacy"),
            ),
            (
                json!({"ui":{"resourceUri":"ui://test/current"},"ui/resourceUri":"ui://test/legacy"}),
                Some("ui://test/current"),
            ),
            (
                json!({"ui":{"visibility":["model"]},"ui/resourceUri":"ui://test/legacy"}),
                Some("ui://test/legacy"),
            ),
            (
                json!({"ui":{"resourceUri":"https://invalid.test/view"},"ui/resourceUri":"ui://test/legacy"}),
                None,
            ),
            (
                json!({"ui":{"resourceUri":null},"ui/resourceUri":"ui://test/legacy"}),
                None,
            ),
            (json!({"ui/resourceUri":"ui://"}), None),
        ] {
            let tool: Tool =
                serde_json::from_value(json!({"name":"test","inputSchema":{},"_meta":meta}))?;
            assert_eq!(tool_ui_resource_uri(&tool), expected);
        }
        Ok(())
    }
}
