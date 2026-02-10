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
    });
    fs::write(
        output_dir.join("wire-types.json"),
        serde_json::to_string_pretty(&wire_types)?,
    )?;

    // Params (only contracts-owned param types)
    let params = serde_json::json!({
        "CommsParams": schema_for!(crate::wire::CommsParams),
        "SkillsParams": schema_for!(crate::wire::SkillsParams),
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
    // Emit a structural description instead of a full JSON Schema.
    let events = serde_json::json!({
        "WireEvent": {
            "description": "Event envelope: session_id, sequence, event (AgentEvent), contract_version",
            "note": "AgentEvent is a large enum; full JSON Schema requires schemars derives on meerkat-core types"
        }
    });
    fs::write(
        output_dir.join("events.json"),
        serde_json::to_string_pretty(&events)?,
    )?;

    // RPC methods — describes the JSON-RPC method surface
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
        ],
        "notifications": [
            {"name": "session/event", "description": "AgentEvent payload during turns"},
        ]
    });
    fs::write(
        output_dir.join("rpc-methods.json"),
        serde_json::to_string_pretty(&rpc_methods)?,
    )?;

    // REST OpenAPI — describes REST endpoint surface
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
