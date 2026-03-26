//! Schema emission — generates JSON schema artifacts.
//!
//! Enabled via `--features schema`. The `emit-schemas` binary writes
//! schema files to `artifacts/schemas/`.

#[cfg(feature = "schema")]
pub fn emit_all_schemas(output_dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use schemars::schema_for;
    use serde_json::{Map, Value};
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

    // Events — includes canonical event type inventory plus schema-ready
    // payload definitions where available.
    let events = serde_json::json!({
        "AgentEvent": schema_for!(meerkat_core::AgentEvent),
        "ScopedAgentEvent": schema_for!(meerkat_core::ScopedAgentEvent),
        "WireEvent": {
            "description": "Event envelope: session_id, sequence, event (AgentEvent), contract_version",
            "known_event_types": crate::KNOWN_AGENT_EVENT_TYPES,
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
            },
            "note": "AgentEvent schema is emitted above. known_event_types stays as a lightweight canonical inventory for surface drift checks."
        }
    });
    fs::write(
        output_dir.join("events.json"),
        serde_json::to_string_pretty(&events)?,
    )?;

    // RPC methods — structural description of the method surface.
    // This is documentation, not a consumable schema for codegen.
    let rpc_methods = serde_json::json!({
        "methods": crate::rpc_method_catalog(crate::RpcMethodCatalogOptions::documented_surface()),
        "notifications": crate::rpc_notification_catalog(
            crate::RpcMethodCatalogOptions::documented_surface()
        )
    });
    fs::write(
        output_dir.join("rpc-methods.json"),
        serde_json::to_string_pretty(&rpc_methods)?,
    )?;

    // REST OpenAPI — endpoint listing snapshot, not a full OpenAPI spec.
    // Request/response body schemas are intentionally omitted, but the path
    // inventory should stay aligned with the live router surface.
    let rest_paths: Map<String, Value> = crate::rest_path_catalog()
        .into_iter()
        .map(|path| {
            let operations = path
                .operations
                .into_iter()
                .map(|operation| {
                    let mut operation_map = Map::new();
                    operation_map.insert(
                        "summary".to_string(),
                        Value::String(operation.summary.to_string()),
                    );
                    if let Some(description) = operation.description {
                        operation_map.insert(
                            "description".to_string(),
                            Value::String(description.to_string()),
                        );
                    }
                    (operation.method.to_string(), Value::Object(operation_map))
                })
                .collect();
            (path.path.to_string(), Value::Object(operations))
        })
        .collect();
    let rest_openapi = serde_json::json!({
        "openapi": "3.0.0",
        "info": {
            "title": "Meerkat REST API",
            "version": crate::version::ContractVersion::CURRENT.to_string(),
        },
        "paths": Value::Object(rest_paths),
    });
    fs::write(
        output_dir.join("rest-openapi.json"),
        serde_json::to_string_pretty(&rest_openapi)?,
    )?;

    Ok(())
}
