//! Schema emission — generates JSON schema artifacts.
//!
//! Enabled via `--features schema`. The `emit-schemas` binary writes
//! schema files to `artifacts/schemas/`.

#[cfg(feature = "schema")]
pub fn emit_all_schemas(output_dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use schemars::schema_for;
    use std::fs;

    fs::create_dir_all(output_dir)?;

    // Version
    let version_schema = serde_json::json!({
        "contract_version": crate::version::ContractVersion::CURRENT.to_string(),
    });
    fs::write(
        output_dir.join("version.json"),
        serde_json::to_string_pretty(&version_schema)?,
    )?;

    // Wire types (contracts-owned types only — types embedding core types
    // without JsonSchema use serde for serialization but not for schema generation)
    let wire_types = serde_json::json!({
        "WireUsage": schema_for!(crate::wire::WireUsage),
        "ContractVersion": schema_for!(crate::version::ContractVersion),
        "McpLiveOpStatus": schema_for!(crate::wire::McpLiveOpStatus),
        "McpLiveOperation": schema_for!(crate::wire::McpLiveOperation),
        "McpLiveOpResponse": schema_for!(crate::wire::McpLiveOpResponse),
        "WireSessionInfo": schema_for!(crate::wire::WireSessionInfo),
        "WireSessionSummary": schema_for!(crate::wire::WireSessionSummary),
    });
    fs::write(
        output_dir.join("wire-types.json"),
        serde_json::to_string_pretty(&wire_types)?,
    )?;

    // Params (only contracts-owned param types)
    let params = serde_json::json!({
        "CoreCreateParams": schema_for!(crate::wire::CoreCreateParams),
        "CommsParams": schema_for!(crate::wire::CommsParams),
        "SkillsParams": schema_for!(crate::wire::SkillsParams),
        "McpAddParams": schema_for!(crate::wire::McpAddParams),
        "McpRemoveParams": schema_for!(crate::wire::McpRemoveParams),
        "McpReloadParams": schema_for!(crate::wire::McpReloadParams),
    });
    fs::write(
        output_dir.join("params.json"),
        serde_json::to_string_pretty(&params)?,
    )?;

    // Errors
    let errors = serde_json::json!({
        "ErrorCode": schema_for!(crate::error::ErrorCode),
        "ErrorCategory": schema_for!(crate::error::ErrorCategory),
        "WireError": schema_for!(crate::error::WireError),
        "CapabilityHint": schema_for!(crate::error::CapabilityHint),
    });
    fs::write(
        output_dir.join("errors.json"),
        serde_json::to_string_pretty(&errors)?,
    )?;

    // Capabilities
    let capabilities = serde_json::json!({
        "CapabilityId": schema_for!(crate::capability::CapabilityId),
        "CapabilityScope": schema_for!(crate::capability::CapabilityScope),
        "CapabilityStatus": schema_for!(crate::capability::CapabilityStatus),
        "CapabilitiesResponse": schema_for!(crate::capability::CapabilitiesResponse),
    });
    fs::write(
        output_dir.join("capabilities.json"),
        serde_json::to_string_pretty(&capabilities)?,
    )?;

    // Events — WireEvent embeds AgentEvent which lacks JsonSchema.
    // Emit a structural description, NOT a consumable JSON Schema.
    // Codegen should not parse these for type generation.
    let events = serde_json::json!({
        "WireEvent": {
            "description": "Event envelope: session_id, sequence, event (AgentEvent), contract_version",
            "note": "AgentEvent is a large enum; full JSON Schema requires schemars derives on meerkat-core types",
            "known_payloads": {
                "tool_config_changed": {
                    "type": "object",
                    "required": ["payload"],
                    "properties": {
                        "payload": {
                            "type": "object",
                            "required": ["operation", "target", "status", "persisted"],
                            "properties": {
                                "operation": {"type": "string", "enum": ["add", "remove", "reload"]},
                                "target": {"type": "string"},
                                "status": {"type": "string"},
                                "persisted": {"type": "boolean"},
                                "applied_at_turn": {"type": ["integer", "null"], "minimum": 0},
                            }
                        }
                    }
                }
            }
        }
    });
    fs::write(
        output_dir.join("events.json"),
        serde_json::to_string_pretty(&events)?,
    )?;

    // RPC methods — structural description of the method surface.
    // This is documentation, not a consumable schema for codegen.
    let rpc_methods = serde_json::json!({
        "methods": [
            {"name": "initialize", "description": "Handshake, returns server capabilities"},
            {"name": "session/create", "description": "Create session + run first turn"},
            {"name": "session/list", "description": "List active sessions"},
            {"name": "session/read", "description": "Get session state"},
            {"name": "session/archive", "description": "Remove session"},
            {"name": "turn/start", "description": "Start a new turn on existing session"},
            {"name": "turn/interrupt", "description": "Cancel in-flight turn"},
            {"name": "capabilities/get", "description": "Get runtime capabilities"},
            {"name": "config/get", "description": "Read config"},
            {"name": "config/set", "description": "Replace config"},
            {"name": "config/patch", "description": "Merge-patch config"},
            {
                "name": "mcp/add",
                "description": "Stage live MCP server add for a session",
                "params_type": "McpAddParams",
                "result_type": "McpLiveOpResponse"
            },
            {
                "name": "mcp/remove",
                "description": "Stage live MCP server remove for a session",
                "params_type": "McpRemoveParams",
                "result_type": "McpLiveOpResponse"
            },
            {
                "name": "mcp/reload",
                "description": "Optional skeleton for live MCP reload",
                "params_type": "McpReloadParams",
                "result_type": "McpLiveOpResponse"
            },
        ],
        "notifications": [
            {"name": "initialized", "description": "Client notification acknowledged silently (no response)"},
            {"name": "session/event", "description": "AgentEvent payload during turns"},
        ]
    });
    fs::write(
        output_dir.join("rpc-methods.json"),
        serde_json::to_string_pretty(&rpc_methods)?,
    )?;

    // REST OpenAPI — stub endpoint listing, not a full OpenAPI spec.
    // No request/response body schemas included.
    let rest_openapi = serde_json::json!({
        "openapi": "3.0.0",
        "info": {
            "title": "Meerkat REST API",
            "version": crate::version::ContractVersion::CURRENT.to_string(),
        },
        "paths": {
            "/sessions": {"post": {"summary": "Create and run a new session"}},
            "/sessions/{id}": {"get": {"summary": "Get session details"}},
            "/sessions/{id}/messages": {"post": {"summary": "Continue session with new message"}},
            "/sessions/{id}/events": {"get": {"summary": "SSE event stream"}},
            "/sessions/{id}/mcp/add": {"post": {
                "summary": "Stage live MCP server addition",
                "description": "Requires mcp_live capability. Check GET /capabilities.",
            }},
            "/sessions/{id}/mcp/remove": {"post": {
                "summary": "Stage live MCP server removal",
                "description": "Requires mcp_live capability. Check GET /capabilities.",
            }},
            "/sessions/{id}/mcp/reload": {"post": {
                "summary": "Stage live MCP server reload",
                "description": "Requires mcp_live capability. Check GET /capabilities.",
            }},
            "/capabilities": {"get": {"summary": "Get runtime capabilities"}},
            "/config": {
                "get": {"summary": "Get config"},
                "put": {"summary": "Replace config"},
                "patch": {"summary": "Patch config"},
            },
            "/health": {"get": {"summary": "Health check"}},
        }
    });
    fs::write(
        output_dir.join("rest-openapi.json"),
        serde_json::to_string_pretty(&rest_openapi)?,
    )?;

    Ok(())
}
