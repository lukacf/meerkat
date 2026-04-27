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

    fn write_pretty_json(
        path: std::path::PathBuf,
        value: &Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut body = serde_json::to_string_pretty(value)?;
        body.push('\n');
        fs::write(path, body)?;
        Ok(())
    }

    // Version
    let version_schema = serde_json::json!({
        "contract_version": crate::version::ContractVersion::CURRENT.to_string(),
    });
    write_pretty_json(output_dir.join("version.json"), &version_schema)?;

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
        "MobDefinitionInput": schema_for!(crate::wire::MobDefinitionInput),
        "MobCreateResult": schema_for!(crate::wire::MobCreateResult),
        "MobMemberSendParams": schema_for!(crate::wire::MobMemberSendParams),
        "MobMemberSendResult": schema_for!(crate::wire::MobMemberSendResult),
        "MobIngressInteractionParams": schema_for!(crate::wire::MobIngressInteractionParams),
        "MobIngressInteractionResult": schema_for!(crate::wire::MobIngressInteractionResult),
        "WireHandlingMode": schema_for!(crate::wire::WireHandlingMode),
        "WireRenderClass": schema_for!(crate::wire::WireRenderClass),
        "WireRenderSalience": schema_for!(crate::wire::WireRenderSalience),
        "WireRenderMetadata": schema_for!(crate::wire::WireRenderMetadata),
        "MobWireResult": schema_for!(crate::wire::MobWireResult),
        "MobUnwireResult": schema_for!(crate::wire::MobUnwireResult),
        "WireRuntimeState": schema_for!(crate::wire::WireRuntimeState),
        "RuntimeStateResult": schema_for!(crate::wire::RuntimeStateResult),
        "WireRealtimeAttachmentStatus": schema_for!(crate::wire::WireRealtimeAttachmentStatus),
        "RuntimeRealtimeAttachmentStatusResult": schema_for!(crate::wire::RuntimeRealtimeAttachmentStatusResult),
        "RealtimeChannelTarget": schema_for!(crate::wire::RealtimeChannelTarget),
        "RealtimeChannelRole": schema_for!(crate::wire::RealtimeChannelRole),
        "RealtimeTurningMode": schema_for!(crate::wire::RealtimeTurningMode),
        "RealtimeInputKind": schema_for!(crate::wire::RealtimeInputKind),
        "RealtimeOutputKind": schema_for!(crate::wire::RealtimeOutputKind),
        "RealtimeReconnectPolicy": schema_for!(crate::wire::RealtimeReconnectPolicy),
        "RealtimeCapabilities": schema_for!(crate::wire::RealtimeCapabilities),
        "RealtimeChannelState": schema_for!(crate::wire::RealtimeChannelState),
        "RealtimeChannelStatus": schema_for!(crate::wire::RealtimeChannelStatus),
        "RealtimeOpenInfo": schema_for!(crate::wire::RealtimeOpenInfo),
        "RealtimeStatusResult": schema_for!(crate::wire::RealtimeStatusResult),
        "RealtimeCapabilitiesResult": schema_for!(crate::wire::RealtimeCapabilitiesResult),
        "RealtimeTextChunk": schema_for!(crate::wire::RealtimeTextChunk),
        "RealtimeTextDelta": schema_for!(crate::wire::RealtimeTextDelta),
        "RealtimeAudioChunk": schema_for!(crate::wire::RealtimeAudioChunk),
        "RealtimeVideoChunk": schema_for!(crate::wire::RealtimeVideoChunk),
        "RealtimeInputChunk": schema_for!(crate::wire::RealtimeInputChunk),
        "RealtimeOutputChunk": schema_for!(crate::wire::RealtimeOutputChunk),
        "RealtimeEvent": schema_for!(crate::wire::RealtimeEvent),
        "RealtimeChannelOpenFrame": schema_for!(crate::wire::RealtimeChannelOpenFrame),
        "RealtimeChannelInputFrame": schema_for!(crate::wire::RealtimeChannelInputFrame),
        "RealtimeChannelOpenedFrame": schema_for!(crate::wire::RealtimeChannelOpenedFrame),
        "RealtimeChannelStatusFrame": schema_for!(crate::wire::RealtimeChannelStatusFrame),
        "RealtimeChannelEventFrame": schema_for!(crate::wire::RealtimeChannelEventFrame),
        "RealtimeChannelErrorFrame": schema_for!(crate::wire::RealtimeChannelErrorFrame),
        "RealtimeChannelClosedFrame": schema_for!(crate::wire::RealtimeChannelClosedFrame),
        "RealtimeClientFrame": schema_for!(crate::wire::RealtimeClientFrame),
        "RealtimeServerFrame": schema_for!(crate::wire::RealtimeServerFrame),
        "RuntimeAcceptOutcomeType": schema_for!(crate::wire::RuntimeAcceptOutcomeType),
        "WireInputLifecycleState": schema_for!(crate::wire::WireInputLifecycleState),
        "WireInputStateHistoryEntry": schema_for!(crate::wire::WireInputStateHistoryEntry),
        "WireInputState": schema_for!(crate::wire::WireInputState),
        "RuntimeAcceptResult": schema_for!(crate::wire::RuntimeAcceptResult),
        "RuntimeRetireResult": schema_for!(crate::wire::RuntimeRetireResult),
        "RuntimeResetResult": schema_for!(crate::wire::RuntimeResetResult),
        "InputStateResult": schema_for!(crate::wire::InputStateResult),
        "InputListResult": schema_for!(crate::wire::InputListResult),
        "WireContentBlock": schema_for!(crate::wire::WireContentBlock),
        "WireContentInput": schema_for!(crate::wire::WireContentInput),
        "WireToolResultContent": schema_for!(crate::wire::WireToolResultContent),
        "WireAssistantBlock": schema_for!(crate::wire::WireAssistantBlock),
        "WireAssistantImageRef": schema_for!(crate::wire::WireAssistantImageRef),
        "WireGenerateImageRequest": schema_for!(crate::wire::WireGenerateImageRequest),
        "WireGenerateImageExecutionPlan": schema_for!(crate::wire::WireGenerateImageExecutionPlan),
        "WireImageGenerationToolResult": schema_for!(crate::wire::WireImageGenerationToolResult),
        "WireImageOperationPhase": schema_for!(crate::wire::WireImageOperationPhase),
        "WireSwitchTurnIntent": schema_for!(crate::wire::WireSwitchTurnIntent),
        "WireSwitchTurnControlResult": schema_for!(crate::wire::WireSwitchTurnControlResult),
        "WireSwitchTurnPhase": schema_for!(crate::wire::WireSwitchTurnPhase),
        "WireModelRoutingApprovalPhase": schema_for!(crate::wire::WireModelRoutingApprovalPhase),
        "WireModelRoutingApprovalRequest": schema_for!(crate::wire::WireModelRoutingApprovalRequest),
        "WireScopedModelOverride": schema_for!(crate::wire::WireScopedModelOverride),
        "WireSessionModelRoutingStatus": schema_for!(crate::wire::WireSessionModelRoutingStatus),
        "WireProviderMeta": schema_for!(crate::wire::WireProviderMeta),
        "WireSessionHistory": schema_for!(crate::wire::WireSessionHistory),
        "WireSessionInfo": schema_for!(crate::wire::WireSessionInfo),
        "WireSessionMessage": schema_for!(crate::wire::WireSessionMessage),
        "WireSessionSummary": schema_for!(crate::wire::WireSessionSummary),
        "WireStopReason": schema_for!(crate::wire::WireStopReason),
        "WireToolCall": schema_for!(crate::wire::WireToolCall),
        "WireToolResult": schema_for!(crate::wire::WireToolResult),
        "ExecutionPlacement": schema_for!(meerkat_core::ExecutionPlacement),
        "ExecutionPlacementIdentity": schema_for!(meerkat_core::ExecutionPlacementIdentity),
        "ScheduleListResult": schema_for!(crate::wire::ScheduleListResult),
        "ScheduleOccurrencesResult": schema_for!(crate::wire::ScheduleOccurrencesResult),
        // Phase 4c — connection/auth wire types.
        "WireConnectionRef": schema_for!(crate::wire::WireConnectionRef),
        "WireBackendProfile": schema_for!(crate::wire::WireBackendProfile),
        "WireAuthProfile": schema_for!(crate::wire::WireAuthProfile),
        "WireProviderBinding": schema_for!(crate::wire::WireProviderBinding),
        "WireRealmConnectionSet": schema_for!(crate::wire::WireRealmConnectionSet),
        "WireAuthStatus": schema_for!(crate::wire::WireAuthStatus),
        "WireAuthError": schema_for!(crate::wire::WireAuthError),
        "CommsCommandRequest": schema_for!(crate::wire::CommsCommandRequest),
    });
    write_pretty_json(output_dir.join("wire-types.json"), &wire_types)?;

    // Params (only contracts-owned param types)
    let params = serde_json::json!({
        "CoreCreateParams": schema_for!(crate::wire::CoreCreateParams),
        "CommsParams": schema_for!(crate::wire::CommsParams),
        "SkillsParams": schema_for!(crate::wire::SkillsParams),
        "McpAddParams": schema_for!(crate::wire::McpAddParams),
        "McpRemoveParams": schema_for!(crate::wire::McpRemoveParams),
        "McpReloadParams": schema_for!(crate::wire::McpReloadParams),
        "MobCreateParams": schema_for!(crate::wire::MobCreateParams),
        "MobWireParams": schema_for!(crate::wire::MobWireParams),
        "MobUnwireParams": schema_for!(crate::wire::MobUnwireParams),
        "RealtimeOpenRequest": schema_for!(crate::wire::RealtimeOpenRequest),
        "RealtimeStatusParams": schema_for!(crate::wire::RealtimeStatusParams),
        "RealtimeCapabilitiesParams": schema_for!(crate::wire::RealtimeCapabilitiesParams),
        "RuntimeStateParams": schema_for!(crate::wire::RuntimeStateParams),
        "RuntimeAcceptParams": schema_for!(crate::wire::RuntimeAcceptParams),
        "RuntimeRetireParams": schema_for!(crate::wire::RuntimeRetireParams),
        "RuntimeResetParams": schema_for!(crate::wire::RuntimeResetParams),
        "InputStateParams": schema_for!(crate::wire::InputStateParams),
        "InputListParams": schema_for!(crate::wire::InputListParams),
        "RuntimeRealtimeAttachmentStatusParams": schema_for!(crate::wire::RuntimeRealtimeAttachmentStatusParams),
        "ScheduleIdParams": schema_for!(crate::wire::ScheduleIdParams),
        "ListSchedulesParams": schema_for!(crate::wire::ListSchedulesParams),
        "ScheduleOccurrencesParams": schema_for!(crate::wire::ScheduleOccurrencesParams),
        "UpdateScheduleParams": schema_for!(crate::wire::UpdateScheduleParams),
    });
    write_pretty_json(output_dir.join("params.json"), &params)?;

    // Errors
    let errors = serde_json::json!({
        "ErrorCode": schema_for!(crate::error::ErrorCode),
        "ErrorCategory": schema_for!(crate::error::ErrorCategory),
        "WireError": schema_for!(crate::error::WireError),
        "CapabilityHint": schema_for!(crate::error::CapabilityHint),
    });
    write_pretty_json(output_dir.join("errors.json"), &errors)?;

    // Capabilities
    let capabilities = serde_json::json!({
        "CapabilityId": schema_for!(crate::capability::CapabilityId),
        "CapabilityScope": schema_for!(crate::capability::CapabilityScope),
        "CapabilityStatus": schema_for!(crate::capability::CapabilityStatus),
        "CapabilitiesResponse": schema_for!(crate::capability::CapabilitiesResponse),
    });
    write_pretty_json(output_dir.join("capabilities.json"), &capabilities)?;

    // Runtime host projections
    let runtime_host = serde_json::json!({
        "RuntimeHostIdScope": schema_for!(crate::wire::RuntimeHostIdScope),
        "RuntimeHostHealthStatus": schema_for!(crate::wire::RuntimeHostHealthStatus),
        "RuntimeHostFeatureFlags": schema_for!(crate::wire::RuntimeHostFeatureFlags),
        "RuntimeHostRealmProjection": schema_for!(crate::wire::RuntimeHostRealmProjection),
        "RuntimeHostEndpointProjection": schema_for!(crate::wire::RuntimeHostEndpointProjection),
        "RuntimeHostCapabilities": schema_for!(crate::wire::RuntimeHostCapabilities),
        "RuntimeHostHealth": schema_for!(crate::wire::RuntimeHostHealth),
        "RuntimeHostInfo": schema_for!(crate::wire::RuntimeHostInfo),
    });
    write_pretty_json(output_dir.join("runtime-host.json"), &runtime_host)?;

    // Models catalog
    let models = serde_json::json!({
        "WireModelTier": schema_for!(crate::wire::WireModelTier),
        "WireModelProfile": schema_for!(crate::wire::WireModelProfile),
        "CatalogModelEntry": schema_for!(crate::wire::CatalogModelEntry),
        "ProviderCatalog": schema_for!(crate::wire::ProviderCatalog),
        "ModelsCatalogResponse": schema_for!(crate::wire::ModelsCatalogResponse),
    });
    write_pretty_json(output_dir.join("models.json"), &models)?;

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
    write_pretty_json(output_dir.join("events.json"), &events)?;

    // RPC methods — structural description of the method surface.
    // This is documentation, not a consumable schema for codegen.
    let rpc_methods = serde_json::json!({
        "methods": crate::rpc_method_catalog(crate::RpcMethodCatalogOptions::documented_surface()),
        "notifications": crate::rpc_notification_catalog(
            crate::RpcMethodCatalogOptions::documented_surface()
        )
    });
    write_pretty_json(output_dir.join("rpc-methods.json"), &rpc_methods)?;

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
    write_pretty_json(output_dir.join("rest-openapi.json"), &rest_openapi)?;

    Ok(())
}
