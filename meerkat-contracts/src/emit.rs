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
        "WireTrustedPeerSpec": schema_for!(crate::wire::WireTrustedPeerSpec),
        "MobPeerTarget": schema_for!(crate::wire::MobPeerTarget),
        "WireHandlingMode": schema_for!(crate::wire::WireHandlingMode),
        "WireRenderClass": schema_for!(crate::wire::WireRenderClass),
        "WireRenderSalience": schema_for!(crate::wire::WireRenderSalience),
        "WireRenderMetadata": schema_for!(crate::wire::WireRenderMetadata),
        "MobSendResult": schema_for!(crate::wire::MobSendResult),
        "MobWireResult": schema_for!(crate::wire::MobWireResult),
        "MobUnwireResult": schema_for!(crate::wire::MobUnwireResult),
        "WireRuntimeState": schema_for!(crate::wire::WireRuntimeState),
        "RuntimeStateResult": schema_for!(crate::wire::RuntimeStateResult),
        "RuntimeAcceptOutcomeType": schema_for!(crate::wire::RuntimeAcceptOutcomeType),
        "WireInputLifecycleState": schema_for!(crate::wire::WireInputLifecycleState),
        "WireInputStateHistoryEntry": schema_for!(crate::wire::WireInputStateHistoryEntry),
        "WireInputState": schema_for!(crate::wire::WireInputState),
        "InputStateResult": schema_for!(crate::wire::InputStateResult),
        "RuntimeAcceptResult": schema_for!(crate::wire::RuntimeAcceptResult),
        "RuntimeRetireResult": schema_for!(crate::wire::RuntimeRetireResult),
        "RuntimeResetResult": schema_for!(crate::wire::RuntimeResetResult),
        "InputListResult": schema_for!(crate::wire::InputListResult),
        "WireContentBlock": schema_for!(crate::wire::WireContentBlock),
        "WireContentInput": schema_for!(crate::wire::WireContentInput),
        "WireToolResultContent": schema_for!(crate::wire::WireToolResultContent),
        "WireAssistantBlock": schema_for!(crate::wire::WireAssistantBlock),
        "WireProviderMeta": schema_for!(crate::wire::WireProviderMeta),
        "WireSessionHistory": schema_for!(crate::wire::WireSessionHistory),
        "WireSessionInfo": schema_for!(crate::wire::WireSessionInfo),
        "WireSessionMessage": schema_for!(crate::wire::WireSessionMessage),
        "WireSessionSummary": schema_for!(crate::wire::WireSessionSummary),
        "WireStopReason": schema_for!(crate::wire::WireStopReason),
        "WireToolCall": schema_for!(crate::wire::WireToolCall),
        "WireToolResult": schema_for!(crate::wire::WireToolResult),
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
        "MobSendParams": schema_for!(crate::wire::MobSendParams),
        "MobWireParams": schema_for!(crate::wire::MobWireParams),
        "MobUnwireParams": schema_for!(crate::wire::MobUnwireParams),
        "RuntimeStateParams": schema_for!(crate::wire::RuntimeStateParams),
        "RuntimeAcceptParams": schema_for!(crate::wire::RuntimeAcceptParams),
        "RuntimeRetireParams": schema_for!(crate::wire::RuntimeRetireParams),
        "RuntimeResetParams": schema_for!(crate::wire::RuntimeResetParams),
        "InputStateParams": schema_for!(crate::wire::InputStateParams),
        "InputListParams": schema_for!(crate::wire::InputListParams),
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

    // Models catalog
    let models = serde_json::json!({
        "WireModelTier": schema_for!(crate::wire::WireModelTier),
        "WireModelProfile": schema_for!(crate::wire::WireModelProfile),
        "CatalogModelEntry": schema_for!(crate::wire::CatalogModelEntry),
        "ProviderCatalog": schema_for!(crate::wire::ProviderCatalog),
        "ModelsCatalogResponse": schema_for!(crate::wire::ModelsCatalogResponse),
    });
    fs::write(
        output_dir.join("models.json"),
        serde_json::to_string_pretty(&models)?,
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
            {"name": "session/history", "description": "Get full session history"},
            {"name": "session/archive", "description": "Remove session"},
            {"name": "session/external_event", "description": "Queue a runtime-backed external event"},
            {"name": "session/stream_open", "description": "Open a session event stream"},
            {"name": "session/stream_close", "description": "Close a session event stream"},
            {"name": "turn/start", "description": "Start a new turn on existing session"},
            {"name": "turn/interrupt", "description": "Cancel in-flight turn"},
            {"name": "comms/send", "description": "Send a comms command from a session"},
            {"name": "comms/peers", "description": "List peers visible to a session"},
            {"name": "mob/create", "description": "Create a mob from a prefab or definition"},
            {"name": "mob/list", "description": "List active mobs"},
            {"name": "mob/status", "description": "Get mob lifecycle status"},
            {"name": "mob/prefabs", "description": "List available mob prefabs"},
            {"name": "mob/tools", "description": "List available mob tools"},
            {"name": "mob/call", "description": "Call a mob tool"},
            {"name": "mob/members", "description": "List members in a mob roster"},
            {"name": "mob/member_status", "description": "Get live status for a mob member"},
            {"name": "mob/spawn", "description": "Spawn a new mob member"},
            {"name": "mob/spawn_many", "description": "Spawn multiple new mob members"},
            {"name": "mob/retire", "description": "Retire a mob member"},
            {"name": "mob/respawn", "description": "Respawn a mob member with topology restore"},
            {"name": "mob/force_cancel", "description": "Force-cancel a mob member"},
            {"name": "mob/lifecycle", "description": "Apply a mob lifecycle action"},
            {
                "name": "mob/send",
                "description": "Deliver content to a mob member",
                "params_type": "MobSendParams",
                "result_type": "MobSendResult"
            },
            {"name": "mob/append_system_context", "description": "Append system context for a mob member"},
            {"name": "mob/spawn_helper", "description": "Spawn a helper member and wait for completion"},
            {"name": "mob/fork_helper", "description": "Fork a helper member and wait for completion"},
            {"name": "mob/flows", "description": "List flows defined for a mob"},
            {"name": "mob/flow_run", "description": "Start a mob flow run"},
            {"name": "mob/flow_status", "description": "Get status for a mob flow run"},
            {"name": "mob/flow_cancel", "description": "Cancel a mob flow run"},
            {"name": "mob/stream_open", "description": "Open a mob event stream"},
            {"name": "mob/stream_close", "description": "Close a mob event stream"},
            {
                "name": "mob/wire",
                "description": "Wire a local mob member to a local or external peer",
                "params_type": "MobWireParams",
                "result_type": "MobWireResult"
            },
            {
                "name": "mob/unwire",
                "description": "Unwire a local mob member from a local or external peer",
                "params_type": "MobUnwireParams",
                "result_type": "MobUnwireResult"
            },
            {
                "name": "runtime/state",
                "description": "Get a session runtime's current state",
                "params_type": "RuntimeStateParams",
                "result_type": "RuntimeStateResult"
            },
            {
                "name": "runtime/accept",
                "description": "Accept a runtime input for a session",
                "params_type": "RuntimeAcceptParams",
                "result_type": "RuntimeAcceptResult"
            },
            {
                "name": "runtime/retire",
                "description": "Retire a session runtime",
                "params_type": "RuntimeRetireParams",
                "result_type": "RuntimeRetireResult"
            },
            {
                "name": "runtime/reset",
                "description": "Reset a session runtime",
                "params_type": "RuntimeResetParams",
                "result_type": "RuntimeResetResult"
            },
            {
                "name": "input/state",
                "description": "Get the state of a specific runtime input",
                "params_type": "InputStateParams",
                "result_type": "InputStateResult"
            },
            {
                "name": "input/list",
                "description": "List active runtime inputs for a session",
                "params_type": "InputListParams",
                "result_type": "InputListResult"
            },
            {"name": "capabilities/get", "description": "Get runtime capabilities"},
            {"name": "models/catalog", "description": "Get the compiled-in model catalog", "result_type": "ModelsCatalogResponse"},
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

    // REST OpenAPI — endpoint listing snapshot, not a full OpenAPI spec.
    // Request/response body schemas are intentionally omitted, but the path
    // inventory should stay aligned with the live router surface.
    let rest_openapi = serde_json::json!({
        "openapi": "3.0.0",
        "info": {
            "title": "Meerkat REST API",
            "version": crate::version::ContractVersion::CURRENT.to_string(),
        },
        "paths": {
            "/sessions": {
                "get": {"summary": "List sessions"},
                "post": {"summary": "Create and run a new session"}
            },
            "/sessions/{id}": {
                "get": {"summary": "Get session details"},
                "delete": {"summary": "Archive a session"}
            },
            "/sessions/{id}/history": {"get": {"summary": "Get full session history"}},
            "/sessions/{id}/interrupt": {"post": {"summary": "Interrupt a running session"}},
            "/sessions/{id}/system_context": {"post": {"summary": "Append system context to a session"}},
            "/sessions/{id}/messages": {"post": {"summary": "Continue session with new message"}},
            "/sessions/{id}/external-events": {"post": {"summary": "Queue a runtime-backed external event"}},
            "/sessions/{id}/events": {"get": {"summary": "SSE event stream"}},
            "/comms/send": {"post": {"summary": "Send a comms message"}},
            "/comms/peers": {"get": {"summary": "List resolved comms peers"}},
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
            "/skills": {"get": {"summary": "List available skills"}},
            "/skills/{id}": {"get": {"summary": "Inspect a skill"}},
            "/capabilities": {"get": {"summary": "Get runtime capabilities"}},
            "/models/catalog": {"get": {"summary": "Get the compiled-in model catalog"}},
            "/runtime/{id}/state": {"get": {"summary": "Get runtime state"}},
            "/runtime/{id}/accept": {"post": {"summary": "Accept a runtime input"}},
            "/runtime/{id}/retire": {"post": {"summary": "Retire a runtime"}},
            "/runtime/{id}/reset": {"post": {"summary": "Reset a runtime"}},
            "/input/{id}/list": {"get": {"summary": "List runtime inputs for a session"}},
            "/input/{session_id}/{input_id}": {"get": {"summary": "Get a runtime input state"}},
            "/mob/prefabs": {"get": {"summary": "List mob prefabs"}},
            "/mob/tools": {"get": {"summary": "List mob tools"}},
            "/mob/call": {"post": {"summary": "Invoke a mob tool call"}},
            "/mob/{id}/events": {"get": {"summary": "SSE mob event stream"}},
            "/mob/{id}/spawn-helper": {"post": {"summary": "Spawn a helper member in a mob"}},
            "/mob/{id}/fork-helper": {"post": {"summary": "Fork a helper member in a mob"}},
            "/health": {"get": {"summary": "Health check"}},
        }
    });
    fs::write(
        output_dir.join("rest-openapi.json"),
        serde_json::to_string_pretty(&rest_openapi)?,
    )?;

    Ok(())
}
