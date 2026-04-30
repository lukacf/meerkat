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
        "WireRunResult": schema_for!(crate::wire::WireRunResult),
        "McpLiveOpStatus": schema_for!(crate::wire::McpLiveOpStatus),
        "McpLiveOperation": schema_for!(crate::wire::McpLiveOperation),
        "McpLiveOpResponse": schema_for!(crate::wire::McpLiveOpResponse),
        "WireTrustedPeerSpec": schema_for!(crate::wire::WireTrustedPeerSpec),
        "MobPeerTarget": schema_for!(crate::wire::MobPeerTarget),
        "WireMobBackendKind": schema_for!(crate::wire::WireMobBackendKind),
        "WireMobRuntimeMode": schema_for!(crate::wire::WireMobRuntimeMode),
        "WireRuntimeBinding": schema_for!(crate::wire::WireRuntimeBinding),
        "WireMemberLaunchMode": schema_for!(crate::wire::WireMemberLaunchMode),
        "WireForkContext": schema_for!(crate::wire::WireForkContext),
        "WireToolAccessPolicy": schema_for!(crate::wire::WireToolAccessPolicy),
        "WireBudgetSplitPolicy": schema_for!(crate::wire::WireBudgetSplitPolicy),
        "WireToolFilter": schema_for!(crate::wire::WireToolFilter),
        "WireMobToolConfig": schema_for!(crate::wire::WireMobToolConfig),
        "WireMobProfile": schema_for!(crate::wire::WireMobProfile),
        "MobDefinitionInput": schema_for!(crate::wire::MobDefinitionInput),
        "MobCreateResult": schema_for!(crate::wire::MobCreateResult),
        "MobListResult": schema_for!(crate::wire::MobListResult),
        "MobStatusResult": schema_for!(crate::wire::MobStatusResult),
        "MobLifecycleResult": schema_for!(crate::wire::MobLifecycleResult),
        "MobSpawnResult": schema_for!(crate::wire::MobSpawnResult),
        "MobSpawnManyResult": schema_for!(crate::wire::MobSpawnManyResult),
        "MobRetireResult": schema_for!(crate::wire::MobRetireResult),
        "MobRespawnResult": schema_for!(crate::wire::MobRespawnResult),
        "MobMembersResult": schema_for!(crate::wire::MobMembersResult),
        "MobEventsResult": schema_for!(crate::wire::MobEventsResult),
        "MobMemberSendParams": schema_for!(crate::wire::MobMemberSendParams),
        "MobMemberSendResult": schema_for!(crate::wire::MobMemberSendResult),
        "MobIngressInteractionParams": schema_for!(crate::wire::MobIngressInteractionParams),
        "MobIngressInteractionResult": schema_for!(crate::wire::MobIngressInteractionResult),
        "MobAppendSystemContextResult": schema_for!(crate::wire::MobAppendSystemContextResult),
        "MobFlowsResult": schema_for!(crate::wire::MobFlowsResult),
        "MobFlowRunResult": schema_for!(crate::wire::MobFlowRunResult),
        "MobFlowStatusResult": schema_for!(crate::wire::MobFlowStatusResult),
        "MobFlowCancelResult": schema_for!(crate::wire::MobFlowCancelResult),
        "MobHelperResult": schema_for!(crate::wire::MobHelperResult),
        "MobForceCancelResult": schema_for!(crate::wire::MobForceCancelResult),
        "MobMemberStatusResult": schema_for!(crate::wire::MobMemberStatusResult),
        "MobSnapshotResult": schema_for!(crate::wire::MobSnapshotResult),
        "MobDestroyResult": schema_for!(crate::wire::MobDestroyResult),
        "MobRotateSupervisorResult": schema_for!(crate::wire::MobRotateSupervisorResult),
        "MobWaitMembersResult": schema_for!(crate::wire::MobWaitMembersResult),
        "MobEnsureMemberResult": schema_for!(crate::wire::MobEnsureMemberResult),
        "MobReconcileResult": schema_for!(crate::wire::MobReconcileResult),
        "MobListMembersMatchingResult": schema_for!(crate::wire::MobListMembersMatchingResult),
        "MobSubmitWorkResult": schema_for!(crate::wire::MobSubmitWorkResult),
        "MobCancelWorkResult": schema_for!(crate::wire::MobCancelWorkResult),
        "MobCancelAllWorkResult": schema_for!(crate::wire::MobCancelAllWorkResult),
        "MobProfileLookupResult": schema_for!(crate::wire::MobProfileLookupResult),
        "MobProfileListResult": schema_for!(crate::wire::MobProfileListResult),
        "MobProfileDeleteResult": schema_for!(crate::wire::MobProfileDeleteResult),
        "MobStreamOpenResult": schema_for!(crate::wire::MobStreamOpenResult),
        "MobStreamCloseResult": schema_for!(crate::wire::MobStreamCloseResult),
        "WireHandlingMode": schema_for!(crate::wire::WireHandlingMode),
        "WireRenderClass": schema_for!(crate::wire::WireRenderClass),
        "WireRenderSalience": schema_for!(crate::wire::WireRenderSalience),
        "WireRenderMetadata": schema_for!(crate::wire::WireRenderMetadata),
        "MobWireResult": schema_for!(crate::wire::MobWireResult),
        "MobUnwireResult": schema_for!(crate::wire::MobUnwireResult),
        "WireRuntimeState": schema_for!(crate::wire::WireRuntimeState),
        "RuntimeStateResult": schema_for!(crate::wire::RuntimeStateResult),
        "PeerResponseTerminalStatusWire": schema_for!(crate::wire::PeerResponseTerminalStatusWire),
        "SessionExternalEventEnvelope": schema_for!(crate::wire::SessionExternalEventEnvelope),
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
        "WireBindingIdentity": schema_for!(crate::wire::WireBindingIdentity),
        "WireAuthProfileCreated": schema_for!(crate::wire::WireAuthProfileCreated),
        "WireAuthProfileDetail": schema_for!(crate::wire::WireAuthProfileDetail),
        "WireAuthProfileCleared": schema_for!(crate::wire::WireAuthProfileCleared),
        "WireLoginStart": schema_for!(crate::wire::WireLoginStart),
        "WireLoginReady": schema_for!(crate::wire::WireLoginReady),
        "WireDeviceStart": schema_for!(crate::wire::WireDeviceStart),
        "WireDeviceCompleteResult": schema_for!(crate::wire::WireDeviceCompleteResult),
        "WireProvisionApiKeyResult": schema_for!(crate::wire::WireProvisionApiKeyResult),
        "WireRealmSummary": schema_for!(crate::wire::WireRealmSummary),
        "WireRealmList": schema_for!(crate::wire::WireRealmList),
        "WireAuthProfilesList": schema_for!(crate::wire::WireAuthProfilesList),
        "WireAuthStatus": schema_for!(crate::wire::WireAuthStatus),
        "WireAuthStatusDetail": schema_for!(crate::wire::WireAuthStatusDetail),
        "WireAuthError": schema_for!(crate::wire::WireAuthError),
        "SkillEntry": schema_for!(crate::wire::SkillEntry),
        "SkillListResponse": schema_for!(crate::wire::SkillListResponse),
        "SkillInspectResponse": schema_for!(crate::wire::SkillInspectResponse),
        "CommsCommandRequest": schema_for!(crate::wire::CommsCommandRequest),
        "CommsSendResult": schema_for!(crate::wire::CommsSendResult),
        "PeerId": schema_for!(crate::wire::PeerId),
        "PeerName": schema_for!(crate::wire::WireCommsPeerName),
        "PeerTransport": schema_for!(crate::wire::PeerTransport),
        "PeerAddress": schema_for!(crate::wire::PeerAddress),
        "PeerDirectorySource": schema_for!(crate::wire::PeerDirectorySource),
        "PeerSendability": schema_for!(crate::wire::PeerSendability),
        "PeerCapabilitySet": schema_for!(crate::wire::PeerCapabilitySet),
        "PeerReachability": schema_for!(crate::wire::PeerReachability),
        "PeerReachabilityReason": schema_for!(crate::wire::PeerReachabilityReason),
        "PeerDirectoryEntry": schema_for!(crate::wire::PeerDirectoryEntry),
        "PeerDirectoryListing": schema_for!(crate::wire::PeerDirectoryListing),
        "CommsPeersResult": schema_for!(crate::wire::CommsPeersResult),
        "SessionStreamOpenResult": schema_for!(crate::wire::SessionStreamOpenResult),
        "SessionStreamCloseResult": schema_for!(crate::wire::SessionStreamCloseResult),
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
        "MobIdParams": schema_for!(crate::wire::MobIdParams),
        "MobMemberParams": schema_for!(crate::wire::MobMemberParams),
        "MobSpawnParams": schema_for!(crate::wire::MobSpawnParams),
        "MobSpawnManyParams": schema_for!(crate::wire::MobSpawnManyParams),
        "MobRespawnParams": schema_for!(crate::wire::MobRespawnParams),
        "MobEventsParams": schema_for!(crate::wire::MobEventsParams),
        "MobWireParams": schema_for!(crate::wire::MobWireParams),
        "MobUnwireParams": schema_for!(crate::wire::MobUnwireParams),
        "MobLifecycleParams": schema_for!(crate::wire::MobLifecycleParams),
        "MobAppendSystemContextParams": schema_for!(crate::wire::MobAppendSystemContextParams),
        "MobFlowRunParams": schema_for!(crate::wire::MobFlowRunParams),
        "MobFlowStatusParams": schema_for!(crate::wire::MobFlowStatusParams),
        "MobFlowCancelParams": schema_for!(crate::wire::MobFlowCancelParams),
        "MobSpawnHelperParams": schema_for!(crate::wire::MobSpawnHelperParams),
        "MobForkHelperParams": schema_for!(crate::wire::MobForkHelperParams),
        "MobTurnStartParams": schema_for!(crate::wire::MobTurnStartParams),
        "MobWaitParams": schema_for!(crate::wire::MobWaitParams),
        "MobProfileCreateParams": schema_for!(crate::wire::MobProfileCreateParams),
        "MobProfileNameParams": schema_for!(crate::wire::MobProfileNameParams),
        "MobProfileUpdateParams": schema_for!(crate::wire::MobProfileUpdateParams),
        "MobProfileDeleteParams": schema_for!(crate::wire::MobProfileDeleteParams),
        "MobStreamOpenParams": schema_for!(crate::wire::MobStreamOpenParams),
        "MobStreamCloseParams": schema_for!(crate::wire::MobStreamCloseParams),
        "MobMemberSendParams": schema_for!(crate::wire::MobMemberSendParams),
        "MobIngressInteractionParams": schema_for!(crate::wire::MobIngressInteractionParams),
        "MobEnsureMemberParams": schema_for!(crate::wire::MobEnsureMemberParams),
        "MobReconcileParams": schema_for!(crate::wire::MobReconcileParams),
        "MobListMembersMatchingParams": schema_for!(crate::wire::MobListMembersMatchingParams),
        "MobSubmitWorkParams": schema_for!(crate::wire::MobSubmitWorkParams),
        "MobCancelWorkParams": schema_for!(crate::wire::MobCancelWorkParams),
        "MobCancelAllWorkParams": schema_for!(crate::wire::MobCancelAllWorkParams),
        "RealmIdParams": schema_for!(crate::wire::RealmIdParams),
        "BindingIdParams": schema_for!(crate::wire::BindingIdParams),
        "CreateProfileParams": schema_for!(crate::wire::CreateProfileParams),
        "LoginStartParams": schema_for!(crate::wire::LoginStartParams),
        "LoginCompleteParams": schema_for!(crate::wire::LoginCompleteParams),
        "DeviceStartParams": schema_for!(crate::wire::DeviceStartParams),
        "DeviceCompleteParams": schema_for!(crate::wire::DeviceCompleteParams),
        "ProvisionApiKeyParams": schema_for!(crate::wire::ProvisionApiKeyParams),
        "RealtimeOpenRequest": schema_for!(crate::wire::RealtimeOpenRequest),
        "RealtimeStatusParams": schema_for!(crate::wire::RealtimeStatusParams),
        "RealtimeCapabilitiesParams": schema_for!(crate::wire::RealtimeCapabilitiesParams),
        "RuntimeStateParams": schema_for!(crate::wire::RuntimeStateParams),
        "RuntimeAcceptParams": schema_for!(crate::wire::RuntimeAcceptParams),
        "RuntimeRetireParams": schema_for!(crate::wire::RuntimeRetireParams),
        "RuntimeResetParams": schema_for!(crate::wire::RuntimeResetParams),
        "SessionPeerResponseTerminalParams": schema_for!(crate::wire::SessionPeerResponseTerminalParams),
        "InputStateParams": schema_for!(crate::wire::InputStateParams),
        "InputListParams": schema_for!(crate::wire::InputListParams),
        "RuntimeRealtimeAttachmentStatusParams": schema_for!(crate::wire::RuntimeRealtimeAttachmentStatusParams),
        "SessionStreamOpenParams": schema_for!(crate::wire::SessionStreamOpenParams),
        "SessionStreamCloseParams": schema_for!(crate::wire::SessionStreamCloseParams),
        "CommsSendParams": schema_for!(crate::wire::CommsSendParams),
        "CommsPeersParams": schema_for!(crate::wire::CommsPeersParams),
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

    fn schema_ref(name: &str) -> Value {
        serde_json::json!({ "$ref": format!("#/components/schemas/{name}") })
    }

    fn media_content(content_type: &str, schema_name: &str) -> Value {
        serde_json::json!({
            content_type: {
                "schema": schema_ref(schema_name),
            }
        })
    }

    fn rewrite_component_refs(value: &mut Value) {
        match value {
            Value::Object(object) => {
                if let Some(Value::String(reference)) = object.get_mut("$ref")
                    && let Some(name) = reference.strip_prefix("#/$defs/")
                {
                    *reference = format!("#/components/schemas/{name}");
                }
                for nested in object.values_mut() {
                    rewrite_component_refs(nested);
                }
            }
            Value::Array(values) => {
                for nested in values {
                    rewrite_component_refs(nested);
                }
            }
            _ => {}
        }
    }

    fn add_component_schema(components: &mut Map<String, Value>, name: String, mut schema: Value) {
        let defs = schema
            .as_object_mut()
            .and_then(|object| object.remove("$defs"));
        if let Some(Value::Object(defs)) = defs {
            for (def_name, def_schema) in defs {
                add_component_schema(components, def_name, def_schema);
            }
        }
        if let Some(object) = schema.as_object_mut() {
            object.remove("$schema");
        }
        rewrite_component_refs(&mut schema);
        components.entry(name).or_insert(schema);
    }

    fn add_component_section(components: &mut Map<String, Value>, section: &Value) {
        if let Some(schemas) = section.as_object() {
            for (name, schema) in schemas {
                add_component_schema(components, name.clone(), schema.clone());
            }
        }
    }

    fn object_schema(properties: Vec<(&str, Value)>, required: Vec<&str>) -> Value {
        let mut property_map = Map::new();
        for (name, schema) in properties {
            property_map.insert(name.to_string(), schema);
        }
        serde_json::json!({
            "type": "object",
            "properties": Value::Object(property_map),
            "required": required,
        })
    }

    fn closed_object_schema(properties: Vec<(&str, Value)>, required: Vec<&str>) -> Value {
        let mut schema = object_schema(properties, required);
        if let Value::Object(object) = &mut schema {
            object.insert("additionalProperties".to_string(), Value::Bool(false));
        }
        schema
    }

    fn string_schema() -> Value {
        serde_json::json!({ "type": "string" })
    }

    fn bool_schema() -> Value {
        serde_json::json!({ "type": "boolean" })
    }

    fn integer_schema() -> Value {
        serde_json::json!({ "type": "integer", "minimum": 0 })
    }

    fn json_value_schema() -> Value {
        serde_json::json!({
            "description": "Arbitrary JSON value.",
        })
    }

    fn nullable(schema: Value) -> Value {
        serde_json::json!({ "anyOf": [schema, { "type": "null" }] })
    }

    fn rest_manual_components() -> Map<String, Value> {
        let mut components = Map::new();
        let json_value = schema_ref("JsonValue");
        let content_input = serde_json::json!({
            "oneOf": [
                { "type": "string" },
                schema_ref("WireContentInput")
            ]
        });
        let labels = serde_json::json!({
            "type": "object",
            "additionalProperties": { "type": "string" }
        });
        let string_array = serde_json::json!({
            "type": "array",
            "items": { "type": "string" }
        });

        components.insert("JsonValue".to_string(), json_value_schema());
        components.insert(
            "PlainTextResponse".to_string(),
            serde_json::json!({ "type": "string" }),
        );
        components.insert(
            "SseEventStream".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Server-sent event stream."
            }),
        );
        components.insert(
            "StatusResponse".to_string(),
            serde_json::json!({
                "type": "object",
                "additionalProperties": true
            }),
        );
        components.insert(
            "ListSessionsResponse".to_string(),
            object_schema(
                vec![(
                    "sessions",
                    serde_json::json!({
                        "type": "array",
                        "items": schema_ref("WireSessionSummary")
                    }),
                )],
                vec!["sessions"],
            ),
        );
        components.insert(
            "SessionDetailsResponse".to_string(),
            object_schema(
                vec![
                    ("session_id", string_schema()),
                    ("session_ref", string_schema()),
                    ("created_at", string_schema()),
                    ("updated_at", string_schema()),
                    ("message_count", integer_schema()),
                    ("total_tokens", integer_schema()),
                    ("labels", labels.clone()),
                ],
                vec![
                    "session_id",
                    "session_ref",
                    "created_at",
                    "updated_at",
                    "message_count",
                    "total_tokens",
                ],
            ),
        );
        components.insert(
            "ConfigEnvelope".to_string(),
            object_schema(
                vec![
                    ("config", json_value.clone()),
                    ("generation", integer_schema()),
                    ("realm_id", string_schema()),
                    ("instance_id", string_schema()),
                    ("backend", string_schema()),
                    (
                        "resolved_paths",
                        serde_json::json!({
                            "type": "object",
                            "additionalProperties": { "type": "string" }
                        }),
                    ),
                ],
                vec!["config", "generation"],
            ),
        );
        components.insert(
            "RestCreateSessionRequest".to_string(),
            object_schema(
                vec![
                    ("prompt", content_input.clone()),
                    ("system_prompt", string_schema()),
                    ("model", string_schema()),
                    ("provider", string_schema()),
                    ("max_tokens", integer_schema()),
                    ("output_schema", json_value.clone()),
                    ("structured_output_retries", integer_schema()),
                    ("verbose", bool_schema()),
                    ("keep_alive", nullable(bool_schema())),
                    ("comms_name", string_schema()),
                    ("peer_meta", json_value.clone()),
                    ("hooks_override", json_value.clone()),
                    ("enable_builtins", bool_schema()),
                    ("enable_shell", bool_schema()),
                    ("enable_memory", bool_schema()),
                    ("enable_mob", bool_schema()),
                    ("budget_limits", json_value.clone()),
                    ("provider_params", json_value.clone()),
                    (
                        "preload_skills",
                        serde_json::json!({
                            "type": "array",
                            "items": json_value
                        }),
                    ),
                    (
                        "skill_refs",
                        serde_json::json!({
                            "type": "array",
                            "items": json_value
                        }),
                    ),
                    ("labels", labels),
                    ("additional_instructions", string_array.clone()),
                    ("app_context", json_value.clone()),
                    (
                        "shell_env",
                        serde_json::json!({
                            "type": "object",
                            "additionalProperties": { "type": "string" }
                        }),
                    ),
                ],
                vec!["prompt"],
            ),
        );
        components.insert(
            "RestContinueSessionRequest".to_string(),
            object_schema(
                vec![
                    ("session_id", string_schema()),
                    ("prompt", content_input),
                    ("system_prompt", string_schema()),
                    ("output_schema", json_value.clone()),
                    ("structured_output_retries", integer_schema()),
                    ("keep_alive", nullable(bool_schema())),
                    ("comms_name", string_schema()),
                    ("peer_meta", json_value.clone()),
                    ("verbose", bool_schema()),
                    ("model", string_schema()),
                    ("provider", string_schema()),
                    ("max_tokens", integer_schema()),
                    ("hooks_override", json_value.clone()),
                    (
                        "skill_refs",
                        serde_json::json!({
                            "type": "array",
                            "items": json_value
                        }),
                    ),
                    ("flow_tool_overlay", json_value.clone()),
                    ("additional_instructions", string_array),
                ],
                vec!["session_id", "prompt"],
            ),
        );
        components.insert(
            "RestAppendSystemContextRequest".to_string(),
            object_schema(
                vec![
                    ("text", string_schema()),
                    ("source", string_schema()),
                    ("idempotency_key", string_schema()),
                ],
                vec!["text"],
            ),
        );
        components.insert(
            "RestPeerResponseTerminalRequest".to_string(),
            closed_object_schema(
                vec![
                    ("peer_id", schema_ref("PeerId")),
                    ("display_name", schema_ref("PeerName")),
                    ("request_id", schema_ref("PeerCorrelationId")),
                    ("status", schema_ref("PeerResponseTerminalStatusWire")),
                    ("result", json_value.clone()),
                ],
                vec!["peer_id", "request_id", "status", "result"],
            ),
        );
        components.insert(
            "RestSetConfigRequest".to_string(),
            serde_json::json!({
                "oneOf": [
                    schema_ref("JsonValue"),
                    object_schema(
                        vec![
                            ("config", schema_ref("JsonValue")),
                            ("expected_generation", integer_schema()),
                        ],
                        vec!["config"],
                    )
                ]
            }),
        );
        components.insert(
            "RestPatchConfigRequest".to_string(),
            serde_json::json!({
                "oneOf": [
                    schema_ref("JsonValue"),
                    object_schema(
                        vec![
                            ("patch", schema_ref("JsonValue")),
                            ("expected_generation", integer_schema()),
                        ],
                        vec!["patch"],
                    )
                ]
            }),
        );
        components.insert(
            "RestScheduleToolCallRequest".to_string(),
            object_schema(
                vec![("name", string_schema()), ("arguments", json_value.clone())],
                vec!["name"],
            ),
        );
        components.insert(
            "RestCommsSendRequest".to_string(),
            object_schema(
                vec![
                    ("session_id", string_schema()),
                    ("command", schema_ref("CommsCommandRequest")),
                ],
                vec!["session_id", "command"],
            ),
        );
        components.insert(
            "RestAuthBindingRequest".to_string(),
            object_schema(
                vec![
                    ("realm_id", string_schema()),
                    ("binding_id", string_schema()),
                    ("profile_id", string_schema()),
                ],
                vec!["realm_id", "binding_id"],
            ),
        );
        components.insert(
            "RestAuthProfileCreateRequest".to_string(),
            serde_json::json!({
                "type": "object",
                "additionalProperties": true,
                "required": ["realm_id", "binding_id", "auth_method"],
            }),
        );
        components.insert(
            "RestAuthLoginStartRequest".to_string(),
            serde_json::json!({
                "type": "object",
                "additionalProperties": true,
                "required": ["provider"],
            }),
        );
        components.insert(
            "RestMobHelperRequest".to_string(),
            object_schema(
                vec![
                    ("prompt", string_schema()),
                    ("agent_identity", string_schema()),
                    ("role_name", string_schema()),
                    ("runtime_mode", string_schema()),
                    ("backend", string_schema()),
                ],
                vec!["prompt"],
            ),
        );
        components.insert(
            "RestMobForkHelperRequest".to_string(),
            object_schema(
                vec![
                    ("source_member_id", string_schema()),
                    ("prompt", string_schema()),
                    ("agent_identity", string_schema()),
                    ("role_name", string_schema()),
                    ("fork_context", json_value),
                    ("runtime_mode", string_schema()),
                    ("backend", string_schema()),
                ],
                vec!["source_member_id", "prompt"],
            ),
        );
        components.insert(
            "RestMobWaitRequest".to_string(),
            object_schema(
                vec![
                    (
                        "member_ids",
                        serde_json::json!({
                            "type": "array",
                            "items": { "type": "string" }
                        }),
                    ),
                    ("timeout_ms", integer_schema()),
                ],
                vec![],
            ),
        );
        components
    }

    #[derive(Clone, Copy)]
    struct RestOperationContract {
        request_schema: Option<&'static str>,
        request_required: bool,
        response_schema: &'static str,
        response_content_type: &'static str,
    }

    impl RestOperationContract {
        const fn json(response_schema: &'static str) -> Self {
            Self {
                request_schema: None,
                request_required: false,
                response_schema,
                response_content_type: "application/json",
            }
        }

        const fn with_json_request(
            request_schema: &'static str,
            response_schema: &'static str,
        ) -> Self {
            Self {
                request_schema: Some(request_schema),
                request_required: true,
                response_schema,
                response_content_type: "application/json",
            }
        }

        const fn with_optional_json_request(
            request_schema: &'static str,
            response_schema: &'static str,
        ) -> Self {
            Self {
                request_schema: Some(request_schema),
                request_required: false,
                response_schema,
                response_content_type: "application/json",
            }
        }

        const fn text(response_schema: &'static str) -> Self {
            Self {
                request_schema: None,
                request_required: false,
                response_schema,
                response_content_type: "text/plain",
            }
        }

        const fn event_stream(response_schema: &'static str) -> Self {
            Self {
                request_schema: None,
                request_required: false,
                response_schema,
                response_content_type: "text/event-stream",
            }
        }
    }

    fn rest_operation_contract(path: &str, method: &str) -> RestOperationContract {
        match (path, method) {
            ("/sessions", "get") => RestOperationContract::json("ListSessionsResponse"),
            ("/sessions", "post") => RestOperationContract::with_json_request(
                "RestCreateSessionRequest",
                "WireRunResult",
            ),
            ("/sessions/{id}", "get") => RestOperationContract::json("SessionDetailsResponse"),
            ("/sessions/{id}", "delete") => RestOperationContract::json("StatusResponse"),
            ("/sessions/{id}/history", "get") => RestOperationContract::json("WireSessionHistory"),
            ("/sessions/{id}/interrupt", "post") => RestOperationContract::json("StatusResponse"),
            ("/sessions/{id}/system_context", "post") => RestOperationContract::with_json_request(
                "RestAppendSystemContextRequest",
                "StatusResponse",
            ),
            ("/sessions/{id}/messages", "post") => RestOperationContract::with_json_request(
                "RestContinueSessionRequest",
                "WireRunResult",
            ),
            ("/sessions/{id}/external-events", "post") => {
                RestOperationContract::with_json_request("JsonValue", "StatusResponse")
            }
            ("/sessions/{id}/peer-response-terminal", "post") => {
                RestOperationContract::with_json_request(
                    "RestPeerResponseTerminalRequest",
                    "StatusResponse",
                )
            }
            ("/sessions/{id}/mcp/add", "post") => {
                RestOperationContract::with_json_request("McpAddParams", "McpLiveOpResponse")
            }
            ("/sessions/{id}/mcp/remove", "post") => {
                RestOperationContract::with_json_request("McpRemoveParams", "McpLiveOpResponse")
            }
            ("/sessions/{id}/mcp/reload", "post") => {
                RestOperationContract::with_json_request("McpReloadParams", "McpLiveOpResponse")
            }
            ("/sessions/{id}/events" | "/mob/{id}/events", "get") => {
                RestOperationContract::event_stream("SseEventStream")
            }
            ("/sessions/{id}/status", "get") => RestOperationContract::json("RuntimeStateResult"),
            ("/sessions/{id}/realtime-attachment-status", "get") => {
                RestOperationContract::json("RuntimeRealtimeAttachmentStatusResult")
            }
            ("/sessions/{id}/submit", "post") => RestOperationContract::with_json_request(
                "RuntimeAcceptParams",
                "RuntimeAcceptResult",
            ),
            ("/sessions/{id}/retire", "post") => RestOperationContract::json("RuntimeRetireResult"),
            ("/sessions/{id}/reset", "post") => RestOperationContract::json("RuntimeResetResult"),
            ("/sessions/{id}/submissions", "get") => RestOperationContract::json("InputListResult"),
            ("/sessions/{session_id}/submissions/{submission_id}", "get") => {
                RestOperationContract::json("InputStateResult")
            }
            ("/schedule/call", "post") => {
                RestOperationContract::with_json_request("RestScheduleToolCallRequest", "JsonValue")
            }
            ("/schedules", "get") => RestOperationContract::json("ScheduleListResult"),
            ("/schedules", "post") => {
                RestOperationContract::with_json_request("JsonValue", "JsonValue")
            }
            ("/schedules/{id}", "get" | "delete")
            | ("/schedules/{id}/pause" | "/schedules/{id}/resume", "post") => {
                RestOperationContract::json("JsonValue")
            }
            ("/schedules/{id}", "patch") => {
                RestOperationContract::with_json_request("JsonValue", "JsonValue")
            }
            ("/schedules/{id}/occurrences", "get") => {
                RestOperationContract::json("ScheduleOccurrencesResult")
            }
            ("/comms/send", "post") => {
                RestOperationContract::with_json_request("RestCommsSendRequest", "JsonValue")
            }
            ("/comms/peers", "get") => RestOperationContract::json("CommsPeersResult"),
            ("/config", "get") => RestOperationContract::json("ConfigEnvelope"),
            ("/config", "put") => {
                RestOperationContract::with_json_request("RestSetConfigRequest", "ConfigEnvelope")
            }
            ("/config", "patch") => {
                RestOperationContract::with_json_request("RestPatchConfigRequest", "ConfigEnvelope")
            }
            ("/skills", "get") => RestOperationContract::json("SkillListResponse"),
            ("/capabilities", "get") => RestOperationContract::json("CapabilitiesResponse"),
            ("/runtime/host_info", "get") => RestOperationContract::json("RuntimeHostInfo"),
            ("/runtime/capabilities", "get") => {
                RestOperationContract::json("RuntimeHostCapabilities")
            }
            ("/runtime/health", "get") => RestOperationContract::json("RuntimeHostHealth"),
            ("/models/catalog", "get") => RestOperationContract::json("ModelsCatalogResponse"),
            ("/realtime/open_info", "post") => {
                RestOperationContract::with_json_request("RealtimeOpenRequest", "RealtimeOpenInfo")
            }
            ("/realtime/status", "post") => RestOperationContract::with_json_request(
                "RealtimeStatusParams",
                "RealtimeStatusResult",
            ),
            ("/realtime/capabilities", "post") => RestOperationContract::with_json_request(
                "RealtimeCapabilitiesParams",
                "RealtimeCapabilitiesResult",
            ),
            ("/mob/{id}/spawn-helper", "post") => {
                RestOperationContract::with_json_request("RestMobHelperRequest", "JsonValue")
            }
            ("/mob/{id}/fork-helper", "post") => {
                RestOperationContract::with_json_request("RestMobForkHelperRequest", "JsonValue")
            }
            ("/mob/{id}/wait-kickoff", "post") => {
                RestOperationContract::with_optional_json_request("RestMobWaitRequest", "JsonValue")
            }
            ("/mob/{id}/members/{agent_identity}/status", "get")
            | (
                "/mob/{id}/members/{agent_identity}/cancel"
                | "/mob/{id}/members/{agent_identity}/respawn",
                "post",
            ) => RestOperationContract::json("JsonValue"),
            ("/health", "get") => RestOperationContract::text("PlainTextResponse"),
            ("/auth/profiles", "get") => RestOperationContract::json("WireAuthProfilesList"),
            ("/auth/profiles", "post") => RestOperationContract::with_json_request(
                "RestAuthProfileCreateRequest",
                "WireAuthProfileCreated",
            ),
            ("/auth/bindings/{binding_id}", "get") => {
                RestOperationContract::json("WireAuthProfileDetail")
            }
            ("/auth/bindings/{binding_id}", "delete")
            | ("/auth/bindings/{binding_id}/logout", "post") => {
                RestOperationContract::json("WireAuthProfileCleared")
            }
            ("/auth/bindings/{binding_id}/test", "post")
            | ("/auth/bindings/{binding_id}/status", "get") => {
                RestOperationContract::json("WireAuthStatusDetail")
            }
            ("/auth/login/start", "post") => RestOperationContract::with_json_request(
                "RestAuthLoginStartRequest",
                "WireLoginStart",
            ),
            ("/auth/login/complete", "post") => {
                RestOperationContract::with_json_request("JsonValue", "WireLoginReady")
            }
            ("/auth/login/device/start", "post") => {
                RestOperationContract::with_json_request("JsonValue", "WireDeviceStart")
            }
            ("/auth/login/device/complete", "post") => {
                RestOperationContract::with_json_request("JsonValue", "WireLoginReady")
            }
            ("/realms", "get") => RestOperationContract::json("WireRealmList"),
            ("/realms/{id}", "get") => RestOperationContract::json("WireRealmConnectionSet"),
            _ => RestOperationContract::json("JsonValue"),
        }
    }

    fn rest_operation_id(method: &str, path: &str) -> String {
        let mut operation_id = method.to_string();
        for segment in path.split('/').filter(|segment| !segment.is_empty()) {
            operation_id.push('_');
            operation_id.push_str(
                &segment
                    .trim_start_matches('{')
                    .trim_end_matches('}')
                    .replace('-', "_"),
            );
        }
        operation_id
    }

    fn rest_path_parameters(path: &str) -> Vec<Value> {
        let mut seen = Vec::new();
        for segment in path.split('/') {
            if let Some(name) = segment
                .strip_prefix('{')
                .and_then(|value| value.strip_suffix('}'))
                && !seen.contains(&name)
            {
                seen.push(name);
            }
        }
        seen.into_iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "in": "path",
                    "required": true,
                    "schema": { "type": "string" },
                })
            })
            .collect()
    }

    fn rest_responses(contract: RestOperationContract) -> Value {
        serde_json::json!({
            "200": {
                "description": "Successful response",
                "content": media_content(contract.response_content_type, contract.response_schema),
            },
            "default": {
                "description": "Error response",
                "content": media_content("application/json", "WireError"),
            }
        })
    }

    let mut rest_components = rest_manual_components();
    for section in [
        &wire_types,
        &params,
        &errors,
        &capabilities,
        &runtime_host,
        &models,
    ] {
        add_component_section(&mut rest_components, section);
    }

    // REST OpenAPI — generated wire contract for the documented route surface.
    let rest_paths: Map<String, Value> = crate::rest_path_catalog()
        .into_iter()
        .map(|path| {
            let operations = path
                .operations
                .into_iter()
                .map(|operation| {
                    let contract = rest_operation_contract(path.path, operation.method);
                    let mut operation_map = Map::new();
                    operation_map.insert(
                        "operationId".to_string(),
                        Value::String(rest_operation_id(operation.method, path.path)),
                    );
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
                    let parameters = rest_path_parameters(path.path);
                    if !parameters.is_empty() {
                        operation_map.insert("parameters".to_string(), Value::Array(parameters));
                    }
                    if let Some(request_schema) = contract.request_schema {
                        operation_map.insert(
                            "requestBody".to_string(),
                            serde_json::json!({
                                "required": contract.request_required,
                                "content": media_content("application/json", request_schema),
                            }),
                        );
                    }
                    operation_map.insert("responses".to_string(), rest_responses(contract));
                    (operation.method.to_string(), Value::Object(operation_map))
                })
                .collect();
            (path.path.to_string(), Value::Object(operations))
        })
        .collect();
    let rest_openapi = serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Meerkat REST API",
            "version": crate::version::ContractVersion::CURRENT.to_string(),
        },
        "paths": Value::Object(rest_paths),
        "components": {
            "schemas": Value::Object(rest_components),
        },
    });
    write_pretty_json(output_dir.join("rest-openapi.json"), &rest_openapi)?;

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_output_dir(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "meerkat-contracts-{test_name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create schema output dir");
        dir
    }

    fn assert_schema_accepts(schema: &serde_json::Value, instance: &serde_json::Value) {
        let validator = jsonschema::validator_for(schema).expect("schema compiles");
        let errors = validator
            .iter_errors(instance)
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "schema should accept {instance}, got errors: {errors:?}"
        );
    }

    fn assert_schema_rejects(schema: &serde_json::Value, instance: &serde_json::Value) {
        let validator = jsonschema::validator_for(schema).expect("schema compiles");
        assert!(
            !validator.is_valid(instance),
            "schema should reject invalid shape {instance}"
        );
    }

    #[test]
    fn emitted_schemas_catalog_auth_status_detail() {
        let output_dir = temp_output_dir("auth-status-detail");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let wire_types: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("wire-types.json")).unwrap()).unwrap();
        assert!(
            wire_types.get("WireAuthStatus").is_some(),
            "legacy auth status schema must remain emitted"
        );
        let detail = wire_types
            .get("WireAuthStatusDetail")
            .expect("detailed auth status schema must be emitted");
        let detail_props = detail
            .pointer("/properties")
            .expect("detail schema has properties");
        for field in [
            "realm_id",
            "binding_id",
            "connection_ref",
            "profile_id",
            "has_refresh_token",
        ] {
            assert!(
                detail_props.get(field).is_some(),
                "WireAuthStatusDetail schema missing {field}"
            );
        }

        let rpc_methods: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("rpc-methods.json")).unwrap())
                .unwrap();
        let auth_status = rpc_methods["methods"]
            .as_array()
            .expect("methods array")
            .iter()
            .find(|method| method["name"] == "auth/status/get")
            .expect("auth/status/get catalog entry");
        assert_eq!(
            auth_status["result_type"], "WireAuthStatusDetail",
            "auth/status/get should catalog its concrete detailed response"
        );

        fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn emitted_rpc_catalog_carries_typed_auth_and_mob_contracts() {
        let output_dir = temp_output_dir("typed-rpc-catalog");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let rpc_methods: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("rpc-methods.json")).unwrap())
                .unwrap();
        let methods = rpc_methods["methods"].as_array().expect("methods array");

        for (name, params_type, result_type) in [
            (
                "auth/login/device_complete",
                "DeviceCompleteParams",
                "WireDeviceCompleteResult",
            ),
            (
                "auth/profile/create",
                "CreateProfileParams",
                "WireAuthProfileCreated",
            ),
            (
                "mob/ensure_member",
                "MobEnsureMemberParams",
                "MobEnsureMemberResult",
            ),
            (
                "mob/submit_work",
                "MobSubmitWorkParams",
                "MobSubmitWorkResult",
            ),
            (
                "mob/ingress_interaction",
                "MobIngressInteractionParams",
                "MobIngressInteractionResult",
            ),
            ("mob/spawn", "MobSpawnParams", "MobSpawnResult"),
            ("mob/spawn_many", "MobSpawnManyParams", "MobSpawnManyResult"),
            ("mob/events", "MobEventsParams", "MobEventsResult"),
            (
                "mob/append_system_context",
                "MobAppendSystemContextParams",
                "MobAppendSystemContextResult",
            ),
            (
                "session/stream_open",
                "SessionStreamOpenParams",
                "SessionStreamOpenResult",
            ),
            (
                "session/stream_close",
                "SessionStreamCloseParams",
                "SessionStreamCloseResult",
            ),
            ("comms/send", "CommsSendParams", "CommsSendResult"),
            ("comms/peers", "CommsPeersParams", "CommsPeersResult"),
        ] {
            let method = methods
                .iter()
                .find(|method| method["name"] == name)
                .unwrap_or_else(|| panic!("missing emitted RPC catalog entry for {name}"));
            assert_eq!(
                method["params_type"], params_type,
                "{name} emitted params_type drifted"
            );
            assert_eq!(
                method["result_type"], result_type,
                "{name} emitted result_type drifted"
            );
        }

        let cancel_all_work = methods
            .iter()
            .find(|method| method["name"] == "mob/cancel_all_work")
            .expect("missing emitted RPC catalog entry for mob/cancel_all_work");
        assert_eq!(
            cancel_all_work["params_type"], "MobCancelAllWorkParams",
            "mob/cancel_all_work emitted params_type drifted"
        );
        assert_eq!(
            cancel_all_work["result_type"], "MobCancelAllWorkResult",
            "mob/cancel_all_work emitted result_type drifted"
        );

        fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn emitted_comms_and_session_stream_schemas_validate_public_shapes() {
        let output_dir = temp_output_dir("comms-session-stream-contract-shapes");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let params: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("params.json")).unwrap()).unwrap();
        let wire_types: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("wire-types.json")).unwrap()).unwrap();

        let session_stream_open = params
            .get("SessionStreamOpenParams")
            .expect("SessionStreamOpenParams schema must be emitted");
        assert_schema_accepts(
            session_stream_open,
            &serde_json::json!({ "session_id": "sid_test" }),
        );
        assert_schema_rejects(
            session_stream_open,
            &serde_json::json!({ "stream_id": "not-open-params" }),
        );

        let session_stream_close = params
            .get("SessionStreamCloseParams")
            .expect("SessionStreamCloseParams schema must be emitted");
        assert_schema_accepts(
            session_stream_close,
            &serde_json::json!({ "stream_id": uuid::Uuid::nil().to_string() }),
        );
        assert_schema_rejects(
            session_stream_close,
            &serde_json::json!({ "stream_id": 42 }),
        );

        let comms_send = params
            .get("CommsSendParams")
            .expect("CommsSendParams schema must be emitted");
        assert_schema_accepts(
            comms_send,
            &serde_json::json!({
                "session_id": "sid_test",
                "kind": "input",
                "body": "hello"
            }),
        );
        assert_schema_rejects(
            comms_send,
            &serde_json::json!({
                "session_id": "sid_test",
                "kind": "input"
            }),
        );
        assert_schema_rejects(
            comms_send,
            &serde_json::json!({
                "session_id": "sid_test",
                "kind": "bogus",
                "body": "hello"
            }),
        );
        assert_schema_rejects(
            comms_send,
            &serde_json::json!({
                "session_id": "sid_test",
                "kind": "input",
                "body": "hello",
                "unexpected": true
            }),
        );

        let comms_peers = params
            .get("CommsPeersParams")
            .expect("CommsPeersParams schema must be emitted");
        assert_schema_accepts(
            comms_peers,
            &serde_json::json!({ "session_id": "sid_test" }),
        );
        assert_schema_rejects(comms_peers, &serde_json::json!({ "session": "sid_test" }));

        assert_schema_accepts(
            wire_types
                .get("SessionStreamOpenResult")
                .expect("SessionStreamOpenResult schema must be emitted"),
            &serde_json::json!({
                "stream_id": uuid::Uuid::nil().to_string(),
                "session_id": "sid_test",
                "opened": true
            }),
        );
        assert_schema_accepts(
            wire_types
                .get("SessionStreamCloseResult")
                .expect("SessionStreamCloseResult schema must be emitted"),
            &serde_json::json!({
                "stream_id": uuid::Uuid::nil().to_string(),
                "closed": true,
                "already_closed": false
            }),
        );
        assert_schema_accepts(
            wire_types
                .get("CommsSendResult")
                .expect("CommsSendResult schema must be emitted"),
            &serde_json::json!({
                "kind": "input_accepted",
                "interaction_id": uuid::Uuid::nil().to_string(),
                "stream_reserved": true
            }),
        );
        assert_schema_accepts(
            wire_types
                .get("CommsPeersResult")
                .expect("CommsPeersResult schema must be emitted"),
            &serde_json::json!({ "peers": [] }),
        );

        fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn emitted_mob_spawn_params_expose_advanced_fields_as_concrete_wire_schemas() {
        let output_dir = temp_output_dir("mob-spawn-advanced-slots");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let params: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("params.json")).unwrap()).unwrap();
        let spawn = params
            .get("MobSpawnParams")
            .expect("MobSpawnParams schema must be emitted");
        let properties = spawn
            .pointer("/properties")
            .and_then(serde_json::Value::as_object)
            .expect("MobSpawnParams schema must expose properties");

        for (field, expected_ref) in [
            ("launch_mode", "WireMemberLaunchMode"),
            ("tool_access_policy", "WireToolAccessPolicy"),
            ("budget_split_policy", "WireBudgetSplitPolicy"),
            ("inherited_tool_filter", "WireToolFilter"),
            ("override_profile", "WireMobProfile"),
        ] {
            let field_schema = properties
                .get(field)
                .unwrap_or_else(|| panic!("MobSpawnParams missing accepted field {field}"));
            let field_schema = serde_json::to_string(field_schema).unwrap();
            assert!(
                field_schema.contains(&format!("#/$defs/{expected_ref}")),
                "MobSpawnParams.{field} must reference concrete {expected_ref} schema, got {field_schema}"
            );
        }
        assert_eq!(
            spawn.get("additionalProperties"),
            Some(&serde_json::Value::Bool(false)),
            "mob/spawn can be closed only when all accepted fields are in the typed schema"
        );

        fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn emitted_mob_spawn_override_profile_omits_internal_tool_bundles() {
        let output_dir = temp_output_dir("mob-spawn-public-profile");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let params: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("params.json")).unwrap()).unwrap();
        let spawn_profile = params
            .pointer("/MobSpawnParams/$defs/WireMobProfile")
            .expect("MobSpawnParams must define WireMobProfile");
        assert_eq!(
            spawn_profile.get("additionalProperties"),
            Some(&serde_json::Value::Bool(false)),
            "mob/spawn override_profile must fail closed on unknown profile fields"
        );
        let spawn_tool_config = params
            .pointer("/MobSpawnParams/$defs/WireMobToolConfig")
            .expect("MobSpawnParams must define WireMobToolConfig");
        assert_eq!(
            spawn_tool_config.get("additionalProperties"),
            Some(&serde_json::Value::Bool(false)),
            "mob/spawn override_profile.tools must fail closed on unknown tool fields"
        );
        assert!(
            spawn_tool_config
                .pointer("/properties/rust_bundles")
                .is_none(),
            "mob/spawn override_profile.tools must not expose internal rust_bundles"
        );

        let wire_types: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("wire-types.json")).unwrap()).unwrap();
        let public_tool_config = wire_types
            .get("WireMobToolConfig")
            .expect("WireMobToolConfig schema must be emitted");
        assert!(
            public_tool_config
                .pointer("/properties/rust_bundles")
                .is_none(),
            "top-level WireMobToolConfig must not expose internal rust_bundles"
        );

        fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn emitted_mob_turn_start_params_expose_typed_prompt_and_known_overrides() {
        let output_dir = temp_output_dir("mob-turn-start-typed");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let params: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("params.json")).unwrap()).unwrap();
        let turn_start = params
            .get("MobTurnStartParams")
            .expect("MobTurnStartParams schema must be emitted");
        let properties = turn_start
            .pointer("/properties")
            .and_then(serde_json::Value::as_object)
            .expect("MobTurnStartParams schema must expose properties");
        assert_ne!(
            properties.get("prompt"),
            Some(&serde_json::Value::Bool(true)),
            "mob/turn_start prompt must use the canonical content input schema"
        );
        for field in [
            "skill_refs",
            "flow_tool_overlay",
            "additional_instructions",
            "keep_alive",
            "model",
            "provider",
            "max_tokens",
            "system_prompt",
            "output_schema",
            "structured_output_retries",
            "provider_params",
            "clear_provider_params",
            "connection_ref",
            "clear_connection_ref",
        ] {
            assert!(
                properties.contains_key(field),
                "mob/turn_start params missing explicit turn override field {field}"
            );
        }
        assert_eq!(
            turn_start.get("additionalProperties"),
            Some(&serde_json::Value::Bool(false)),
            "mob/turn_start params must fail closed instead of accepting arbitrary flattened overrides"
        );

        fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn emitted_rest_openapi_contains_wire_contracts_not_only_paths() {
        let output_dir = temp_output_dir("rest-openapi-contracts");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let rest_openapi: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("rest-openapi.json")).unwrap())
                .unwrap();
        let components = rest_openapi
            .pointer("/components/schemas")
            .and_then(serde_json::Value::as_object)
            .expect("rest OpenAPI should publish schema components");
        for expected in [
            "RestCreateSessionRequest",
            "RestPeerResponseTerminalRequest",
            "WireRunResult",
            "WireError",
            "ConfigEnvelope",
        ] {
            assert!(
                components.contains_key(expected),
                "rest OpenAPI components missing {expected}"
            );
        }
        let terminal_request = components
            .get("RestPeerResponseTerminalRequest")
            .expect("rest OpenAPI components missing RestPeerResponseTerminalRequest");
        assert_eq!(
            terminal_request.get("additionalProperties"),
            Some(&serde_json::Value::Bool(false)),
            "terminal peer-response request must be closed like RestPeerResponseTerminalBody"
        );
        assert_eq!(
            terminal_request.pointer("/required"),
            Some(&serde_json::json!([
                "peer_id",
                "request_id",
                "status",
                "result"
            ])),
            "terminal peer-response request must require canonical identity and correlation facts"
        );
        let terminal_properties = terminal_request
            .pointer("/properties")
            .and_then(serde_json::Value::as_object)
            .expect("terminal peer-response request must expose object properties");
        assert!(
            terminal_properties.get("peer_name").is_none(),
            "terminal peer-response request must not admit display names as identity"
        );
        assert_eq!(
            terminal_properties
                .get("peer_id")
                .and_then(|schema| schema.pointer("/$ref"))
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/PeerId")
        );
        assert_eq!(
            terminal_properties
                .get("display_name")
                .and_then(|schema| schema.pointer("/$ref"))
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/PeerName")
        );
        assert_eq!(
            terminal_properties
                .get("request_id")
                .and_then(|schema| schema.pointer("/$ref"))
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/PeerCorrelationId")
        );
        assert_eq!(
            terminal_properties
                .get("status")
                .and_then(|schema| schema.pointer("/$ref"))
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/PeerResponseTerminalStatusWire")
        );

        let create_session = &rest_openapi["paths"]["/sessions"]["post"];
        assert_eq!(
            create_session
                .pointer("/requestBody/content/application~1json/schema/$ref")
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/RestCreateSessionRequest")
        );
        assert_eq!(
            create_session
                .pointer("/responses/200/content/application~1json/schema/$ref")
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/WireRunResult")
        );
        assert_eq!(
            create_session
                .pointer("/responses/default/content/application~1json/schema/$ref")
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/WireError")
        );

        let get_session = &rest_openapi["paths"]["/sessions/{id}"]["get"];
        assert_eq!(
            get_session
                .pointer("/parameters/0/name")
                .and_then(serde_json::Value::as_str),
            Some("id")
        );
        assert_eq!(
            get_session
                .pointer("/responses/200/content/application~1json/schema/$ref")
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/SessionDetailsResponse")
        );

        for (path, request_schema) in [
            ("/sessions/{id}/mcp/add", "McpAddParams"),
            ("/sessions/{id}/mcp/remove", "McpRemoveParams"),
            ("/sessions/{id}/mcp/reload", "McpReloadParams"),
        ] {
            let operation = &rest_openapi["paths"][path]["post"];
            let expected_request = format!("#/components/schemas/{request_schema}");
            assert_eq!(
                operation
                    .pointer("/requestBody/content/application~1json/schema/$ref")
                    .and_then(serde_json::Value::as_str),
                Some(expected_request.as_str()),
                "{path} request body contract drifted"
            );
            assert_eq!(
                operation
                    .pointer("/responses/200/content/application~1json/schema/$ref")
                    .and_then(serde_json::Value::as_str),
                Some("#/components/schemas/McpLiveOpResponse"),
                "{path} response contract drifted"
            );
        }

        let wait_kickoff = &rest_openapi["paths"]["/mob/{id}/wait-kickoff"]["post"];
        assert_eq!(
            wait_kickoff
                .pointer("/requestBody/content/application~1json/schema/$ref")
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/RestMobWaitRequest")
        );
        assert_eq!(
            wait_kickoff
                .pointer("/requestBody/required")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            wait_kickoff
                .pointer("/responses/200/content/application~1json/schema/$ref")
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/JsonValue")
        );

        let body = serde_json::to_string(&rest_openapi).unwrap();
        assert!(
            !body.contains("#/$defs/"),
            "OpenAPI component refs must resolve through #/components/schemas"
        );

        fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn emitted_mob_rpc_contract_names_resolve_to_exported_schemas() {
        let output_dir = temp_output_dir("typed-mob-rpc-catalog-resolution");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let exported_contracts = ["params.json", "wire-types.json"]
            .into_iter()
            .flat_map(|file| {
                let value: serde_json::Value =
                    serde_json::from_slice(&fs::read(output_dir.join(file)).unwrap()).unwrap();
                value
                    .as_object()
                    .expect("schema artifact is an object")
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect::<std::collections::BTreeSet<_>>();
        let rpc_methods: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("rpc-methods.json")).unwrap())
                .unwrap();

        for method in rpc_methods["methods"].as_array().expect("methods array") {
            let Some(name) = method["name"].as_str() else {
                continue;
            };
            if !name.starts_with("mob/") {
                continue;
            }
            for field in ["params_type", "result_type"] {
                let Some(contract_name) = method.get(field).and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                assert!(
                    exported_contracts.contains(contract_name),
                    "{name} advertises {field}={contract_name}, but no emitted schema exports that contract"
                );
            }
        }

        fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn emitted_auth_rpc_contract_names_resolve_to_exported_schemas() {
        let output_dir = temp_output_dir("typed-auth-rpc-catalog-resolution");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let exported_contracts = ["params.json", "wire-types.json"]
            .into_iter()
            .flat_map(|file| {
                let value: serde_json::Value =
                    serde_json::from_slice(&fs::read(output_dir.join(file)).unwrap()).unwrap();
                value
                    .as_object()
                    .expect("schema artifact is an object")
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect::<std::collections::BTreeSet<_>>();
        let rpc_methods: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("rpc-methods.json")).unwrap())
                .unwrap();

        for method in rpc_methods["methods"].as_array().expect("methods array") {
            let Some(name) = method["name"].as_str() else {
                continue;
            };
            if !(name.starts_with("auth/") || name.starts_with("realm/")) {
                continue;
            }
            for field in ["params_type", "result_type"] {
                let Some(contract_name) = method.get(field).and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                assert!(
                    exported_contracts.contains(contract_name),
                    "{name} advertises {field}={contract_name}, but no emitted schema exports that contract"
                );
            }
        }

        fs::remove_dir_all(&output_dir).unwrap();
    }
}
