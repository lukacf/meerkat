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

    fn unique_contract_values(groups: &[&[&'static str]]) -> Vec<&'static str> {
        let mut values = Vec::new();
        for group in groups {
            for value in *group {
                if !values.contains(value) {
                    values.push(*value);
                }
            }
        }
        values
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
        "WireCallbackPending": schema_for!(crate::wire::WireCallbackPending),
        "WirePendingToolCall": schema_for!(crate::wire::WirePendingToolCall),
        "WireCallbackPendingStatus": schema_for!(crate::wire::WireCallbackPendingStatus),
        "HelpExecutionMode": schema_for!(crate::wire::HelpExecutionMode),
        "HelpRequest": schema_for!(crate::wire::HelpRequest),
        "HelpResponse": schema_for!(crate::wire::HelpResponse),
        "McpLiveOpStatus": schema_for!(crate::wire::McpLiveOpStatus),
        "McpLiveOperation": schema_for!(crate::wire::McpLiveOperation),
        "McpLiveOpResponse": schema_for!(crate::wire::McpLiveOpResponse),
        "StreamReadStatus": schema_for!(crate::wire::StreamReadStatus),
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
        "MobRunResultParams": schema_for!(crate::wire::MobRunResultParams),
        "MobRunResult": schema_for!(crate::wire::MobRunResult),
        "WireMobRunResultEnvelope": schema_for!(crate::wire::WireMobRunResultEnvelope),
        // #54/#159/#235/#313: typed mob run / member-connectivity / error wire types
        "WireMobRun": schema_for!(crate::wire::WireMobRun),
        "WireMobRunStatus": schema_for!(crate::wire::WireMobRunStatus),
        "WirePeerConnectivity": schema_for!(crate::wire::WirePeerConnectivity),
        "WirePeerConnectivitySnapshot": schema_for!(crate::wire::WirePeerConnectivitySnapshot),
        "WireUnreachablePeer": schema_for!(crate::wire::WireUnreachablePeer),
        "WireMobError": schema_for!(crate::wire::WireMobError),
        "MobFlowCancelResult": schema_for!(crate::wire::MobFlowCancelResult),
        "MobHelperResult": schema_for!(crate::wire::MobHelperResult),
        "MobForceCancelResult": schema_for!(crate::wire::MobForceCancelResult),
        "MobMemberStatusResult": schema_for!(crate::wire::MobMemberStatusResult),
        "MobSnapshotResult": schema_for!(crate::wire::MobSnapshotResult),
        "MobDestroyResult": schema_for!(crate::wire::MobDestroyResult),
        "MobRotateSupervisorResult": schema_for!(crate::wire::MobRotateSupervisorResult),
        "SupervisorRotationReportWire": schema_for!(crate::wire::SupervisorRotationReportWire),
        "SupervisorRotationIncompleteDataWire": schema_for!(crate::wire::SupervisorRotationIncompleteDataWire),
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
        "MobWireMembersBatchEdge": schema_for!(crate::wire::MobWireMembersBatchEdge),
        "MobWireMembersBatchParams": schema_for!(crate::wire::MobWireMembersBatchParams),
        "MobWireMembersBatchResult": schema_for!(crate::wire::MobWireMembersBatchResult),
        "MobUnwireResult": schema_for!(crate::wire::MobUnwireResult),
        "WireRuntimeState": schema_for!(crate::wire::WireRuntimeState),
        "RuntimeStateResult": schema_for!(crate::wire::RuntimeStateResult),
        "PeerResponseTerminalStatusWire": schema_for!(crate::wire::PeerResponseTerminalStatusWire),
        "SessionExternalEventEnvelope": schema_for!(crate::wire::SessionExternalEventEnvelope),
        "RealtimeTurningMode": schema_for!(crate::wire::RealtimeTurningMode),
        "RealtimeInputKind": schema_for!(crate::wire::RealtimeInputKind),
        "RealtimeOutputKind": schema_for!(crate::wire::RealtimeOutputKind),
        "RealtimeCapabilities": schema_for!(crate::wire::RealtimeCapabilities),
        "RealtimeTextChunk": schema_for!(crate::wire::RealtimeTextChunk),
        "RealtimeAudioChunk": schema_for!(crate::wire::RealtimeAudioChunk),
        "RealtimeVideoChunk": schema_for!(crate::wire::RealtimeVideoChunk),
        "RealtimeImageChunk": schema_for!(crate::wire::RealtimeImageChunk),
        "RealtimeInputChunk": schema_for!(crate::wire::RealtimeInputChunk),
        "LiveOpenParams": schema_for!(crate::wire::LiveOpenParams),
        "LiveOpenTransport": schema_for!(crate::wire::LiveOpenTransport),
        "LiveOpenResult": schema_for!(crate::wire::LiveOpenResult),
        "LiveWebrtcAnswerParams": schema_for!(crate::wire::LiveWebrtcAnswerParams),
        "LiveWebrtcAnswerResult": schema_for!(crate::wire::LiveWebrtcAnswerResult),
        "LiveChannelParams": schema_for!(crate::wire::LiveChannelParams),
        "LiveStatusResult": schema_for!(crate::wire::LiveStatusResult),
        "LiveSendInputParams": schema_for!(crate::wire::LiveSendInputParams),
        "LiveInputChunkWire": schema_for!(crate::wire::LiveInputChunkWire),
        "LiveTruncateParams": schema_for!(crate::wire::LiveTruncateParams),
        // G9 (P2): emit `LiveCommitInputParams` and `WireLiveResponseModality`
        // at the top level so SDK codegen produces typed shapes (param
        // struct + discriminated modality union) for `live/commit_input`
        // instead of falling back to the opaque `LiveChannelParams` shape.
        "LiveCommitInputParams": schema_for!(crate::wire::LiveCommitInputParams),
        "WireLiveResponseModality": schema_for!(crate::wire::WireLiveResponseModality),
        // R4-5 (P3): emit the typed `live/refresh` result so SDK codegen
        // produces a typed shape (TypedDict / interface) carrying the typed
        // `LiveRefreshStatus` discriminator, instead of falling back to
        // opaque `Value` / `Any` / `unknown`.
        "LiveRefreshResult": schema_for!(crate::wire::LiveRefreshResult),
        "LiveRefreshStatus": schema_for!(crate::wire::LiveRefreshStatus),
        "LiveCloseResult": schema_for!(crate::wire::LiveCloseResult),
        "LiveCloseStatus": schema_for!(crate::wire::LiveCloseStatus),
        // #234: typed live-command results (send_input/commit_input/interrupt/truncate)
        // replace the prior ad-hoc `Value` results from the RPC handler.
        "LiveSendInputResult": schema_for!(crate::wire::LiveSendInputResult),
        "LiveSendInputErrorData": schema_for!(crate::wire::LiveSendInputErrorData),
        "LiveSendInputStatus": schema_for!(crate::wire::LiveSendInputStatus),
        "LiveCommitInputResult": schema_for!(crate::wire::LiveCommitInputResult),
        "LiveCommitInputStatus": schema_for!(crate::wire::LiveCommitInputStatus),
        "LiveInterruptResult": schema_for!(crate::wire::LiveInterruptResult),
        "LiveInterruptStatus": schema_for!(crate::wire::LiveInterruptStatus),
        "LiveTruncateResult": schema_for!(crate::wire::LiveTruncateResult),
        "LiveTruncateStatus": schema_for!(crate::wire::LiveTruncateStatus),
        // CC5/CC6: emit the typed wire mirrors at the top level so SDK
        // codegen produces named typed shapes (TypedDict / interface /
        // discriminated union) instead of inlining them as anonymous `Any`
        // / `unknown` blobs inside `LiveOpenResult`.
        "WireLiveChannelCapabilities": schema_for!(crate::wire::WireLiveChannelCapabilities),
        "WireLiveContinuityMode": schema_for!(crate::wire::WireLiveContinuityMode),
        // G8 (P2): emit `WireLiveTransportBootstrap` at the top level so
        // SDK codegen produces a typed discriminated union (TS) /
        // tagged-variant TypedDict (Python) for `LiveOpenResult.transport`
        // instead of `unknown` / `Any`.
        "WireLiveTransportBootstrap": schema_for!(crate::wire::WireLiveTransportBootstrap),
        // FIX-SDK-OBS: emit `WireLiveAdapterObservation` and its supporting
        // typed wire mirrors so SDK codegen sees the discriminated union of
        // adapter observations (R5-4 identity fields on
        // `assistant_audio_chunk`, R5-9 `command_rejected` typed channel
        // survives error) rather than treating them as opaque blobs.
        // The Rust owner is `WireRealtimeTranscriptEvent`; its schema keeps
        // the historical `RealtimeTranscriptEvent` name for SDK compatibility
        // while excluding the internal byte-bearing `UserContentFinal` event.
        "RealtimeTranscriptEvent": schema_for!(crate::wire::WireRealtimeTranscriptEvent),
        "WireLiveAdapterObservation": schema_for!(crate::wire::WireLiveAdapterObservation),
        "WireLiveAdapterStatus": schema_for!(crate::wire::WireLiveAdapterStatus),
        "WireLiveDegradationReason": schema_for!(crate::wire::WireLiveDegradationReason),
        "WireLiveAdapterErrorCode": schema_for!(crate::wire::WireLiveAdapterErrorCode),
        "RuntimeAcceptOutcomeType": schema_for!(crate::wire::RuntimeAcceptOutcomeType),
        "WireInputLifecycleState": schema_for!(crate::wire::WireInputLifecycleState),
        "WireInputStateHistoryEntry": schema_for!(crate::wire::WireInputStateHistoryEntry),
        "WireInputState": schema_for!(crate::wire::WireInputState),
        "RuntimeAcceptResult": schema_for!(crate::wire::RuntimeAcceptResult),
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
        "WireScopedModelOverride": schema_for!(crate::wire::WireScopedModelOverride),
        "WireSessionModelRoutingStatus": schema_for!(crate::wire::WireSessionModelRoutingStatus),
        "WireProviderMeta": schema_for!(crate::wire::WireProviderMeta),
        "WireSessionHistory": schema_for!(crate::wire::WireSessionHistory),
        "SessionForkResult": schema_for!(meerkat_core::SessionForkResult),
        "SessionTranscriptRewriteResult": schema_for!(meerkat_core::SessionTranscriptRewriteResult),
        "TranscriptRewriteReason": schema_for!(meerkat_core::TranscriptRewriteReason),
        "TranscriptRewriteSelection": schema_for!(meerkat_core::TranscriptRewriteSelection),
        "TranscriptRewriteMessage": schema_for!(crate::wire::TranscriptRewriteMessage),
        "TranscriptEditRunningBehavior": schema_for!(meerkat_core::TranscriptEditRunningBehavior),
        "WireSessionInfo": schema_for!(crate::wire::WireSessionInfo),
        "WireSessionMessage": schema_for!(crate::wire::WireSessionMessage),
        "WireSessionTranscriptRevision": schema_for!(crate::wire::WireSessionTranscriptRevision),
        "WireSessionTranscriptRevisionList": schema_for!(crate::wire::WireSessionTranscriptRevisionList),
        "WireSessionSummary": schema_for!(crate::wire::WireSessionSummary),
        "WireStopReason": schema_for!(crate::wire::WireStopReason),
        "WireToolResult": schema_for!(crate::wire::WireToolResult),
        "ExecutionPlacement": schema_for!(meerkat_core::ExecutionPlacement),
        "ExecutionPlacementIdentity": schema_for!(meerkat_core::ExecutionPlacementIdentity),
        "ScheduleListResult": schema_for!(crate::wire::ScheduleListResult),
        "ScheduleOccurrencesResult": schema_for!(crate::wire::ScheduleOccurrencesResult),
        "AttentionBindingRequest": schema_for!(meerkat_workgraph::AttentionBindingRequest),
        "AttentionBindingResult": schema_for!(meerkat_workgraph::AttentionBindingResult),
        "AttentionDelegatedAuthority": schema_for!(meerkat_workgraph::AttentionDelegatedAuthority),
        "AttentionContinueOutcome": schema_for!(meerkat_workgraph::AttentionContinueOutcome),
        "AttentionContinueResult": schema_for!(meerkat_workgraph::AttentionContinueResult),
        "AttentionListRequest": schema_for!(meerkat_workgraph::AttentionListRequest),
        "AttentionListResult": schema_for!(meerkat_workgraph::AttentionListResult),
        "AttentionPauseRequest": schema_for!(meerkat_workgraph::AttentionPauseRequest),
        "AttentionResumeRequest": schema_for!(meerkat_workgraph::AttentionResumeRequest),
        "AttentionContextProjection": schema_for!(meerkat_workgraph::AttentionContextProjection),
        "AttentionProjectionPolicy": schema_for!(meerkat_workgraph::AttentionProjectionPolicy),
        "AttentionProjectionRequest": schema_for!(meerkat_workgraph::AttentionProjectionRequest),
        "AttentionProjectionResult": schema_for!(meerkat_workgraph::AttentionProjectionResult),
        "AttentionProjectionText": schema_for!(meerkat_workgraph::AttentionProjectionText),
        "AttentionReassignRequest": schema_for!(meerkat_workgraph::AttentionReassignRequest),
        "AttentionReassignResult": schema_for!(meerkat_workgraph::AttentionReassignResult),
        "GoalAttentionTarget": schema_for!(meerkat_workgraph::GoalAttentionTarget),
        "GoalConfirmRequest": schema_for!(meerkat_workgraph::GoalConfirmRequest),
        "GoalConfirmResult": schema_for!(meerkat_workgraph::GoalConfirmResult),
        "GoalCreateRequest": schema_for!(meerkat_workgraph::GoalCreateRequest),
        "GoalCreateResult": schema_for!(meerkat_workgraph::GoalCreateResult),
        "GoalRequestCloseRequest": schema_for!(meerkat_workgraph::GoalRequestCloseRequest),
        "GoalRequestCloseResult": schema_for!(meerkat_workgraph::GoalRequestCloseResult),
        "GoalTerminalStatus": schema_for!(meerkat_workgraph::GoalTerminalStatus),
        "GoalStatusRequest": schema_for!(meerkat_workgraph::GoalStatusRequest),
        "GoalStatusResult": schema_for!(meerkat_workgraph::GoalStatusResult),
        "PolicyEscalateRequest": schema_for!(meerkat_workgraph::PolicyEscalateRequest),
        "PublicGoalCompletionPolicy": schema_for!(meerkat_workgraph::PublicGoalCompletionPolicy),
        "PublicGoalCreateRequest": schema_for!(meerkat_workgraph::PublicGoalCreateRequest),
        "PublicGoalRequestCloseRequest": schema_for!(meerkat_workgraph::PublicGoalRequestCloseRequest),
        "ProjectedAttentionAuthority": schema_for!(meerkat_workgraph::ProjectedAttentionAuthority),
        "WorkAttentionBinding": schema_for!(meerkat_workgraph::WorkAttentionBinding),
        "WorkAttentionBindingId": schema_for!(meerkat_workgraph::WorkAttentionBindingId),
        "WorkAttentionMode": schema_for!(meerkat_workgraph::WorkAttentionMode),
        "WorkAttentionStatus": schema_for!(meerkat_workgraph::WorkAttentionStatus),
        "WorkAttentionTarget": schema_for!(meerkat_workgraph::WorkAttentionTarget),
        "WorkCompletionPolicy": schema_for!(meerkat_workgraph::WorkCompletionPolicy),
        "WorkItem": schema_for!(meerkat_workgraph::WorkItem),
        "WorkItemRef": schema_for!(meerkat_workgraph::WorkItemRef),
        "WorkGraphSnapshot": schema_for!(meerkat_workgraph::WorkGraphSnapshot),
        "WorkGraphItemsResponse": schema_for!(meerkat_workgraph::WorkGraphItemsResponse),
        "WorkGraphEventsResponse": schema_for!(meerkat_workgraph::WorkGraphEventsResponse),
        // Phase 4c — auth-binding wire types.
        "WireAuthBindingRef": schema_for!(crate::wire::WireAuthBindingRef),
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
        "BridgeAck": schema_for!(crate::wire::BridgeAck),
        "BridgeBindPayload": schema_for!(crate::wire::BridgeBindPayload),
        "BridgeBindResponse": schema_for!(crate::wire::BridgeBindResponse),
        "BridgeBootstrapToken": schema_for!(crate::wire::supervisor_bridge::BridgeBootstrapToken),
        "BridgeCapabilities": schema_for!(crate::wire::BridgeCapabilities),
        "BridgeCommand": schema_for!(crate::wire::BridgeCommand),
        "BridgeDeliveryOutcome": schema_for!(crate::wire::BridgeDeliveryOutcome),
        "BridgeDeliveryPayload": schema_for!(crate::wire::BridgeDeliveryPayload),
        "BridgeDeliveryRejectionCause": schema_for!(crate::wire::BridgeDeliveryRejectionCause),
        "BridgeDeliveryResponse": schema_for!(crate::wire::BridgeDeliveryResponse),
        "BridgeDestroyResponse": schema_for!(crate::wire::BridgeDestroyResponse),
        "BridgeHardCancelPayload": schema_for!(crate::wire::BridgeHardCancelPayload),
        "BridgeMemberRuntimeState": schema_for!(crate::wire::BridgeMemberRuntimeState),
        "BridgeMobPeerOverlayHandoff": schema_for!(crate::wire::BridgeMobPeerOverlayHandoff),
        "BridgeObservationResponse": schema_for!(crate::wire::BridgeObservationResponse),
        "BridgePeerConnectivity": schema_for!(crate::wire::BridgePeerConnectivity),
        "BridgePeerSpec": schema_for!(crate::wire::BridgePeerSpec),
        "BridgePeerWiringPayload": schema_for!(crate::wire::BridgePeerWiringPayload),
        "BridgeProtocolVersion": schema_for!(crate::wire::BridgeProtocolVersion),
        "BridgeRejectionCause": schema_for!(crate::wire::supervisor_bridge::BridgeRejectionCause),
        "BridgeReply": schema_for!(crate::wire::BridgeReply),
        "BridgeRetireResponse": schema_for!(crate::wire::BridgeRetireResponse),
        "BridgeSupervisorDelivery": schema_for!(crate::wire::BridgeSupervisorDelivery),
        "BridgeSupervisorPayload": schema_for!(crate::wire::BridgeSupervisorPayload),
        "BridgeSupervisorRotationObservation": schema_for!(crate::wire::BridgeSupervisorRotationObservation),
        "BridgeSupervisorRotationObserve": schema_for!(crate::wire::BridgeSupervisorRotationObserve),
        "BridgeSupervisorRotationOperationReceipt": schema_for!(crate::wire::BridgeSupervisorRotationOperationReceipt),
        "BridgeSupervisorRotationPendingPhase": schema_for!(crate::wire::BridgeSupervisorRotationPendingPhase),
        "BridgeSupervisorRotationRejectionCause": schema_for!(crate::wire::BridgeSupervisorRotationRejectionCause),
        "BridgeSupervisorRotationRejectionReceipt": schema_for!(crate::wire::BridgeSupervisorRotationRejectionReceipt),
        "BridgeSupervisorRotationState": schema_for!(crate::wire::BridgeSupervisorRotationState),
        "BridgeSupervisorRotationSubmit": schema_for!(crate::wire::BridgeSupervisorRotationSubmit),
        "BridgeSupervisorRotationTargetReceipt": schema_for!(crate::wire::BridgeSupervisorRotationTargetReceipt),
        "SupervisorRotationOperationId": schema_for!(crate::wire::SupervisorRotationOperationId),
        "CommsChecksumTokenParams": schema_for!(crate::wire::CommsChecksumTokenParams),
        "CommsChecksumTokenResult": schema_for!(crate::wire::CommsChecksumTokenResult),
        "CommsChecksumTokenResultIntent": schema_for!(crate::wire::CommsChecksumTokenResultIntent),
        "CommsCommandRequest": schema_for!(crate::wire::CommsCommandRequest),
        "CommsPeerLifecycleParams": schema_for!(crate::wire::CommsPeerLifecycleParams),
        "CommsPeerRequestIntent": schema_for!(crate::wire::CommsPeerRequestIntent),
        "CommsPeerRequestParams": schema_for!(crate::wire::CommsPeerRequestParams),
        "CommsPeerResponseResult": schema_for!(crate::wire::CommsPeerResponseResult),
        "CommsSendResult": schema_for!(crate::wire::CommsSendResult),
        "CommsSendErrorData": schema_for!(crate::wire::CommsSendErrorData),
        "PeerId": schema_for!(crate::wire::PeerId),
        "PeerName": schema_for!(crate::wire::WireCommsPeerName),
        "PeerTransport": schema_for!(crate::wire::PeerTransport),
        "PeerAddress": schema_for!(crate::wire::PeerAddress),
        "PeerDirectorySource": schema_for!(crate::wire::PeerDirectorySource),
        "PeerSendability": schema_for!(crate::wire::PeerSendability),
        "PeerCapabilitySet": schema_for!(crate::wire::PeerCapabilitySet),
        "PeerDirectoryEntry": schema_for!(crate::wire::PeerDirectoryEntry),
        "PeerDirectoryListing": schema_for!(crate::wire::PeerDirectoryListing),
        "CommsPeersResult": schema_for!(crate::wire::CommsPeersResult),
        "AgentEventEnvelope": schema_for!(meerkat_core::EventEnvelope<meerkat_core::AgentEvent>),
        "SessionStreamOpenResult": schema_for!(crate::wire::SessionStreamOpenResult),
        "SessionStreamCloseResult": schema_for!(crate::wire::SessionStreamCloseResult),
        // RPC method result types previously declared handler-local in
        // meerkat-rpc (catalog named them; no schema was emitted).
        "ListSessionsResult": schema_for!(crate::wire::ListSessionsResult),
        "InjectSystemContextResult": schema_for!(crate::wire::InjectSystemContextResult),
        "DeferredCreateResult": schema_for!(crate::wire::DeferredCreateResult),
        "ScheduleToolsResult": schema_for!(crate::wire::ScheduleToolsResult),
        "CallbackToolDefinition": schema_for!(crate::wire::CallbackToolDefinition),
        "ToolsRegisterResult": schema_for!(crate::wire::ToolsRegisterResult),
        "InterruptResult": schema_for!(crate::wire::InterruptResult),
        "ConfigEnvelope": schema_for!(meerkat_core::ConfigEnvelope),
        "ConfigSetParams": schema_for!(crate::wire::ConfigSetParams),
        "ConfigPatchParams": schema_for!(crate::wire::ConfigPatchParams),
        "ServerCapabilities": schema_for!(crate::wire::ServerCapabilities),
        "ConfigWriteResult": schema_for!(crate::wire::ConfigWriteResult),
        "WorkItemsResult": schema_for!(crate::wire::WorkItemsResult),
        "WorkEventsResult": schema_for!(crate::wire::WorkEventsResult),
        "WirePromptInput": schema_for!(crate::wire::WirePromptInput),
        "ApprovalRecord": schema_for!(crate::wire::ApprovalRecord),
        "ApprovalListResult": schema_for!(crate::wire::ApprovalListResult),
        "ArtifactRecord": schema_for!(meerkat_core::ArtifactRecord),
        "ArtifactListResult": schema_for!(crate::wire::ArtifactListResult),
        "ArtifactDownloadResult": schema_for!(crate::wire::ArtifactDownloadResult),
        "BlobPayload": schema_for!(meerkat_core::BlobPayload),
        "EventsLatestCursorResult": schema_for!(crate::wire::EventsLatestCursorResult),
        "EventsListSinceResult": schema_for!(crate::wire::EventsListSinceResult),
        "EventsSnapshotResult": schema_for!(crate::wire::EventsSnapshotResult),
        "Schedule": schema_for!(crate::wire::Schedule),
    });
    write_pretty_json(output_dir.join("wire-types.json"), &wire_types)?;

    // Params (only contracts-owned param types)
    let params = serde_json::json!({
        "CoreCreateParams": schema_for!(crate::wire::CoreCreateParams),
        "HelpRequest": schema_for!(crate::wire::HelpRequest),
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
        "MobRunParams": schema_for!(crate::wire::MobRunParams),
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
        "SessionPeerResponseTerminalParams": schema_for!(crate::wire::SessionPeerResponseTerminalParams),
        "SessionStreamOpenParams": schema_for!(crate::wire::SessionStreamOpenParams),
        "SessionStreamCloseParams": schema_for!(crate::wire::SessionStreamCloseParams),
        "ForkSessionAtParams": schema_for!(crate::wire::ForkSessionAtParams),
        "ForkSessionReplaceParams": schema_for!(crate::wire::ForkSessionReplaceParams),
        "RewriteSessionTranscriptParams": schema_for!(crate::wire::RewriteSessionTranscriptParams),
        "ReadSessionTranscriptRevisionParams": schema_for!(crate::wire::ReadSessionTranscriptRevisionParams),
        "ListSessionTranscriptRevisionsParams": schema_for!(crate::wire::ListSessionTranscriptRevisionsParams),
        "RestoreSessionTranscriptRevisionParams": schema_for!(crate::wire::RestoreSessionTranscriptRevisionParams),
        "CommsSendParams": schema_for!(crate::wire::CommsSendParams),
        "CommsPeersParams": schema_for!(crate::wire::CommsPeersParams),
        "ScheduleIdParams": schema_for!(crate::wire::ScheduleIdParams),
        "ListSchedulesParams": schema_for!(crate::wire::ListSchedulesParams),
        "ScheduleOccurrencesParams": schema_for!(crate::wire::ScheduleOccurrencesParams),
        "UpdateScheduleParams": schema_for!(crate::wire::UpdateScheduleParams),
        "ScheduleToolCallParams": schema_for!(crate::wire::ScheduleToolCallParams),
        "CreateScheduleRequest": schema_for!(meerkat_schedule::CreateScheduleRequest),
        "UpdateScheduleRequest": schema_for!(meerkat_schedule::UpdateScheduleRequest),
        "ApprovalRequestParams": schema_for!(crate::wire::ApprovalRequestParams),
        "ApprovalGetParams": schema_for!(crate::wire::ApprovalGetParams),
        "ApprovalListParams": schema_for!(crate::wire::ApprovalListParams),
        "ApprovalDecideParams": schema_for!(crate::wire::ApprovalDecideParams),
        "ArtifactListParams": schema_for!(crate::wire::ArtifactListParams),
        "ArtifactIdParams": schema_for!(crate::wire::ArtifactIdParams),
        "ArtifactDownloadParams": schema_for!(crate::wire::ArtifactDownloadParams),
        "BlobGetParams": schema_for!(crate::wire::BlobGetParams),
        "EventsLatestCursorParams": schema_for!(crate::wire::EventsLatestCursorParams),
        "EventsListSinceParams": schema_for!(crate::wire::EventsListSinceParams),
        "EventsSnapshotParams": schema_for!(crate::wire::EventsSnapshotParams),
        "ListSessionsParams": schema_for!(crate::wire::ListSessionsParams),
        "ReadSessionParams": schema_for!(crate::wire::ReadSessionParams),
        "ReadSessionHistoryParams": schema_for!(crate::wire::ReadSessionHistoryParams),
        "ArchiveSessionParams": schema_for!(crate::wire::ArchiveSessionParams),
        "InjectSystemContextParams": schema_for!(crate::wire::InjectSystemContextParams),
        "SessionInputStateSelector": schema_for!(crate::wire::SessionInputStateSelector),
        "SessionInputStateParams": schema_for!(crate::wire::SessionInputStateParams),
        "SessionInputStateResult": schema_for!(crate::wire::SessionInputStateResult),
        "InterruptParams": schema_for!(crate::wire::InterruptParams),
        "ToolsRegisterParams": schema_for!(crate::wire::ToolsRegisterParams),
        "WorkGraphIdParams": schema_for!(meerkat_workgraph::WorkGraphIdParams),
        "WorkItemFilter": schema_for!(meerkat_workgraph::WorkItemFilter),
        "ReadyWorkFilter": schema_for!(meerkat_workgraph::ReadyWorkFilter),
        "WorkGraphSnapshotFilter": schema_for!(meerkat_workgraph::WorkGraphSnapshotFilter),
        "WorkGraphEventFilter": schema_for!(meerkat_workgraph::WorkGraphEventFilter),
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
        "WireResolvedModelCapabilities": schema_for!(crate::wire::WireResolvedModelCapabilities),
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

    // Auth/connection semantic vocabulary consumed by generated SDK helpers.
    // Values are projected from typed provider/auth/source/state enums so Web
    // helpers fail closed without carrying local string authority.
    let openai_backend_kinds: Vec<&'static str> =
        meerkat_core::provider_matrix::OpenAiBackendKind::ALL
            .iter()
            .copied()
            .map(meerkat_core::provider_matrix::OpenAiBackendKind::as_str)
            .collect();
    let anthropic_backend_kinds: Vec<&'static str> =
        meerkat_core::provider_matrix::AnthropicBackendKind::ALL
            .iter()
            .copied()
            .map(meerkat_core::provider_matrix::AnthropicBackendKind::as_str)
            .collect();
    let google_backend_kinds: Vec<&'static str> =
        meerkat_core::provider_matrix::GoogleBackendKind::ALL
            .iter()
            .copied()
            .map(meerkat_core::provider_matrix::GoogleBackendKind::as_str)
            .collect();
    let self_hosted_backend_kinds = ["self_hosted", "openai_compatible"];
    let backend_kinds = unique_contract_values(&[
        &openai_backend_kinds,
        &anthropic_backend_kinds,
        &google_backend_kinds,
        &self_hosted_backend_kinds,
    ]);

    let openai_auth_methods: Vec<&'static str> =
        meerkat_core::provider_matrix::OpenAiAuthMethod::ALL
            .iter()
            .copied()
            .map(meerkat_core::provider_matrix::OpenAiAuthMethod::as_str)
            .collect();
    let anthropic_auth_methods: Vec<&'static str> =
        meerkat_core::provider_matrix::AnthropicAuthMethod::ALL
            .iter()
            .copied()
            .map(meerkat_core::provider_matrix::AnthropicAuthMethod::as_str)
            .collect();
    let google_auth_methods: Vec<&'static str> =
        meerkat_core::provider_matrix::GoogleAuthMethod::ALL
            .iter()
            .copied()
            .map(meerkat_core::provider_matrix::GoogleAuthMethod::as_str)
            .collect();
    let self_hosted_auth_methods = ["api_key", "static_bearer", "none"];
    let auth_methods = unique_contract_values(&[
        &openai_auth_methods,
        &anthropic_auth_methods,
        &google_auth_methods,
        &self_hosted_auth_methods,
    ]);

    let providers: Vec<&'static str> = meerkat_core::Provider::ALL_CONCRETE
        .iter()
        .map(meerkat_core::Provider::as_str)
        .collect();
    let auth_status_states: Vec<&'static str> = meerkat_core::AuthStatusPhase::ALL
        .iter()
        .copied()
        .map(meerkat_core::AuthStatusPhase::as_public_str)
        .collect();
    let auth_connection_contracts = serde_json::json!({
        "providers": providers,
        "backend_kinds": backend_kinds,
        "auth_methods": auth_methods,
        "credential_source_kinds": meerkat_core::CredentialSourceSpec::ALL_KIND_LABELS,
        "auth_status_states": auth_status_states,
        "device_complete_states": ["pending", "slow_down", "access_denied", "expired", "ready"],
        "login_ready_state": "ready",
        "provider_backend_kinds": {
            "openai": openai_backend_kinds,
            "anthropic": anthropic_backend_kinds,
            "gemini": google_backend_kinds,
            "self_hosted": self_hosted_backend_kinds,
        },
        "provider_auth_methods": {
            "openai": openai_auth_methods,
            "anthropic": anthropic_auth_methods,
            "gemini": google_auth_methods,
            "self_hosted": self_hosted_auth_methods,
        }
    });
    write_pretty_json(
        output_dir.join("auth-connection-contracts.json"),
        &auth_connection_contracts,
    )?;

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

    fn collect_component_refs(value: &Value, refs: &mut std::collections::BTreeSet<String>) {
        match value {
            Value::Object(object) => {
                if let Some(Value::String(reference)) = object.get("$ref")
                    && let Some(name) = reference.strip_prefix("#/components/schemas/")
                {
                    refs.insert(name.to_string());
                }
                for nested in object.values() {
                    collect_component_refs(nested, refs);
                }
            }
            Value::Array(values) => {
                for nested in values {
                    collect_component_refs(nested, refs);
                }
            }
            _ => {}
        }
    }

    fn referenced_rest_components(
        components: Map<String, Value>,
        rest_paths: &Value,
    ) -> Map<String, Value> {
        let mut needed = std::collections::BTreeSet::new();
        collect_component_refs(rest_paths, &mut needed);

        let mut queue = needed
            .iter()
            .cloned()
            .collect::<std::collections::VecDeque<_>>();
        while let Some(name) = queue.pop_front() {
            let Some(schema) = components.get(&name) else {
                continue;
            };
            let before = needed.clone();
            collect_component_refs(schema, &mut needed);
            for discovered in needed.difference(&before) {
                queue.push_back(discovered.clone());
            }
        }

        components
            .into_iter()
            .filter(|(name, _schema)| needed.contains(name))
            .collect()
    }

    fn json_value_schema() -> Value {
        serde_json::json!({
            "description": "Arbitrary JSON value.",
        })
    }

    /// REST components that are intrinsic protocol markers rather than
    /// projections of any Rust wire struct. Everything else in the REST
    /// OpenAPI document is emitted via `schema_for!` from the actual wire
    /// types (see `rest_contract_types`), so this list must stay limited to
    /// content-type markers and explicitly declared-untyped pass-throughs.
    fn rest_intrinsic_components() -> Map<String, Value> {
        let mut components = Map::new();
        // Declared-untyped JSON pass-through (e.g. webhook payloads, dynamic
        // tool results). Referencing this marker is an explicit statement
        // that the body has no schema, never a fallback.
        components.insert("JsonValue".to_string(), json_value_schema());
        // text/plain response marker.
        components.insert(
            "PlainTextResponse".to_string(),
            serde_json::json!({ "type": "string" }),
        );
        // text/event-stream response marker.
        components.insert(
            "SseEventStream".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Server-sent event stream."
            }),
        );
        // Ad-hoc `{"status": ...}` acknowledgement envelope. Residual
        // untyped surface: handlers that reference it return handler-built
        // status objects pending a typed acknowledgement contract.
        components.insert(
            "StatusResponse".to_string(),
            serde_json::json!({
                "type": "object",
                "additionalProperties": true
            }),
        );
        components
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

    fn rest_query_parameter(name: &str, schema: Value) -> Value {
        serde_json::json!({
            "name": name,
            "in": "query",
            "required": false,
            "schema": schema,
        })
    }

    fn rest_workgraph_query_parameters(path: &str, method: &str) -> Vec<Value> {
        if method != "get" {
            return Vec::new();
        }
        let string = || serde_json::json!({ "type": "string" });
        let boolean = || serde_json::json!({ "type": "boolean" });
        let integer = || serde_json::json!({ "type": "integer", "format": "int64" });
        let string_array = || {
            serde_json::json!({
                "type": "array",
                "items": { "type": "string" }
            })
        };
        let work_status_array = || {
            serde_json::json!({
                "type": "array",
                "items": { "$ref": "#/components/schemas/WorkStatus" }
            })
        };
        match path {
            "/workgraph/items" | "/workgraph/snapshot" => vec![
                rest_query_parameter("realm_id", string()),
                rest_query_parameter("namespace", string()),
                rest_query_parameter("all_namespaces", boolean()),
                rest_query_parameter("statuses", work_status_array()),
                rest_query_parameter("labels", string_array()),
                rest_query_parameter("include_terminal", boolean()),
                rest_query_parameter("limit", integer()),
            ],
            "/workgraph/items/{id}" => vec![
                rest_query_parameter("realm_id", string()),
                rest_query_parameter("namespace", string()),
            ],
            "/workgraph/ready" => vec![
                rest_query_parameter("realm_id", string()),
                rest_query_parameter("namespace", string()),
                rest_query_parameter("labels", string_array()),
                rest_query_parameter("limit", integer()),
            ],
            "/workgraph/events" => vec![
                rest_query_parameter("realm_id", string()),
                rest_query_parameter("namespace", string()),
                rest_query_parameter("all_namespaces", boolean()),
                rest_query_parameter("after_seq", integer()),
                rest_query_parameter("limit", integer()),
            ],
            _ => Vec::new(),
        }
    }

    fn rest_responses(operation: &crate::RestOperationDescriptor) -> Value {
        serde_json::json!({
            "200": {
                "description": "Successful response",
                "content": media_content(operation.response_content_type, operation.response_schema),
            },
            "default": {
                "description": "Error response",
                "content": media_content("application/json", "WireError"),
            }
        })
    }

    // REST wire-contract components emitted from the actual wire structs —
    // the catalog references these by name, so the advertised body schema is
    // a projection of the type the surface deserializes (no hand-rolled
    // shadow components; see `wire/rest.rs`).
    let rest_contract_types = serde_json::json!({
        "RestCreateSessionRequest": schema_for!(crate::wire::RestCreateSessionRequest),
        "RestContinueSessionRequest": schema_for!(crate::wire::RestContinueSessionRequest),
        "RestAppendSystemContextRequest": schema_for!(crate::wire::RestAppendSystemContextRequest),
        "RestPeerResponseTerminalRequest": schema_for!(crate::wire::RestPeerResponseTerminalRequest),
        "RestSetConfigRequest": schema_for!(crate::wire::RestSetConfigRequest),
        "RestPatchConfigRequest": schema_for!(crate::wire::RestPatchConfigRequest),
        "RestAuthProfileCreateRequest": schema_for!(crate::wire::RestAuthProfileCreateRequest),
        "RestAuthBindingTestRequest": schema_for!(crate::wire::RestAuthBindingTestRequest),
        "RestMobHelperRequest": schema_for!(crate::wire::RestMobHelperRequest),
        "RestMobForkHelperRequest": schema_for!(crate::wire::RestMobForkHelperRequest),
        "RestMobWaitRequest": schema_for!(crate::wire::RestMobWaitRequest),
        "RestMobWireMembersBatchRequest": schema_for!(crate::wire::RestMobWireMembersBatchRequest),
        "RestSessionDetailsResponse": schema_for!(crate::wire::RestSessionDetailsResponse),
        "ConfigEnvelope": schema_for!(meerkat_core::ConfigEnvelope),
        "PublicTurnToolOverlay": schema_for!(meerkat_core::PublicTurnToolOverlay),
        "WorkStatus": schema_for!(meerkat_workgraph::WorkStatus),
    });

    let mut rest_components = rest_intrinsic_components();
    for section in [
        &wire_types,
        &params,
        &errors,
        &capabilities,
        &runtime_host,
        &models,
        &rest_contract_types,
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
                    let mut parameters = rest_path_parameters(path.path);
                    parameters.extend(rest_workgraph_query_parameters(path.path, operation.method));
                    if !parameters.is_empty() {
                        operation_map.insert("parameters".to_string(), Value::Array(parameters));
                    }
                    if let Some(request_schema) = operation.request_schema {
                        operation_map.insert(
                            "requestBody".to_string(),
                            serde_json::json!({
                                "required": operation.request_required,
                                "content": media_content("application/json", request_schema),
                            }),
                        );
                    }
                    operation_map.insert("responses".to_string(), rest_responses(&operation));
                    (operation.method.to_string(), Value::Object(operation_map))
                })
                .collect();
            (path.path.to_string(), Value::Object(operations))
        })
        .collect();
    let rest_components =
        referenced_rest_components(rest_components, &Value::Object(rest_paths.clone()));
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
            "auth_binding",
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
    fn emitted_schemas_export_named_realtime_image_chunk() {
        let output_dir = temp_output_dir("realtime-image-chunk");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let wire_types: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("wire-types.json")).unwrap()).unwrap();
        let image = wire_types
            .get("RealtimeImageChunk")
            .expect("RealtimeImageChunk must be a top-level SDK schema");
        let properties = image
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("RealtimeImageChunk schema properties");
        assert!(properties.contains_key("idempotency_key"));
        assert!(properties.contains_key("mime_type"));
        assert!(properties.contains_key("data"));
        let required = image
            .get("required")
            .and_then(serde_json::Value::as_array)
            .expect("RealtimeImageChunk schema required fields");
        for field in ["idempotency_key", "mime_type", "data"] {
            assert!(
                required.iter().any(|value| value == field),
                "RealtimeImageChunk schema must require {field}"
            );
        }

        let live_image = wire_types
            .get("LiveInputChunkWire")
            .and_then(|schema| schema.get("oneOf"))
            .and_then(serde_json::Value::as_array)
            .and_then(|variants| {
                variants.iter().find(|variant| {
                    variant
                        .pointer("/properties/kind/const")
                        .and_then(serde_json::Value::as_str)
                        == Some("image")
                })
            })
            .expect("LiveInputChunkWire image variant");
        assert!(live_image.pointer("/properties/idempotency_key").is_some());
        assert!(
            live_image
                .get("required")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|required| {
                    required.iter().any(|value| value == "idempotency_key")
                })
        );

        fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn emitted_supervisor_rotation_schemas_are_named_and_uuid_typed() {
        let output_dir = temp_output_dir("supervisor-rotation");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let wire_types: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("wire-types.json")).unwrap()).unwrap();
        for name in [
            "BridgeSupervisorDelivery",
            "BridgeSupervisorRotationObservation",
            "BridgeSupervisorRotationObserve",
            "BridgeSupervisorRotationOperationReceipt",
            "BridgeSupervisorRotationPendingPhase",
            "BridgeSupervisorRotationRejectionCause",
            "BridgeSupervisorRotationRejectionReceipt",
            "BridgeSupervisorRotationState",
            "BridgeSupervisorRotationSubmit",
            "BridgeSupervisorRotationTargetReceipt",
            "SupervisorRotationOperationId",
        ] {
            assert!(
                wire_types.get(name).is_some(),
                "wire schema inventory must contain {name}"
            );
        }
        assert_eq!(
            wire_types["SupervisorRotationOperationId"]["type"],
            serde_json::json!("string")
        );

        fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn emitted_schedule_schema_matches_flat_public_projection() {
        let output_dir = temp_output_dir("flat-public-schedule");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let wire_types: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("wire-types.json")).unwrap()).unwrap();
        let schedule_schema = wire_types
            .get("Schedule")
            .expect("Schedule public schema must be emitted");
        let properties = schedule_schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("Schedule schema properties");
        assert!(properties.contains_key("planning_horizon_days"));
        assert!(properties.contains_key("planning_horizon_occurrences"));
        assert!(properties.contains_key("created_at_utc"));
        assert!(properties.contains_key("updated_at_utc"));
        assert!(
            !properties.contains_key("config"),
            "public Schedule must flatten ScheduleConfig"
        );
        assert!(
            !properties.contains_key("machine_state"),
            "public Schedule must not expose durable machine authority"
        );

        let now = chrono::Utc::now();
        let public_schedule = crate::wire::Schedule {
            schedule_id: meerkat_schedule::ScheduleId::new(),
            phase: meerkat_schedule::SchedulePhase::Active,
            revision: meerkat_schedule::ScheduleRevision::initial(),
            trigger: meerkat_schedule::TriggerSpec::Once { due_at_utc: now },
            target: meerkat_schedule::TargetBinding::host_runnable(
                meerkat_schedule::HostRunnableTargetBinding {
                    runnable: meerkat_schedule::HostRunnableName::parse("contract-test")
                        .expect("valid runnable name"),
                    params: None,
                },
            ),
            misfire_policy: meerkat_schedule::MisfirePolicy::default(),
            overlap_policy: meerkat_schedule::OverlapPolicy::default(),
            missing_target_policy: meerkat_schedule::MissingTargetPolicy::default(),
            next_occurrence_ordinal: meerkat_schedule::OccurrenceOrdinal::default(),
            planning_cursor_utc: None,
            superseded_ack_ids: std::collections::BTreeSet::new(),
            config: meerkat_schedule::ScheduleConfig {
                name: Some("contract test".to_string()),
                description: None,
                planning_horizon_days: 30,
                planning_horizon_occurrences: 64,
                labels: std::collections::BTreeMap::new(),
                created_at_utc: now,
                updated_at_utc: now,
                deleted_at_utc: None,
            },
        };
        let public_value = serde_json::to_value(public_schedule).expect("serialize schedule");
        assert!(public_value.get("config").is_none());
        assert!(public_value.get("machine_state").is_none());
        assert_schema_accepts(schedule_schema, &public_value);

        let mut nested_config = public_value;
        let object = nested_config.as_object_mut().expect("schedule object");
        let config = serde_json::json!({
            "planning_horizon_days": object.remove("planning_horizon_days").unwrap(),
            "planning_horizon_occurrences": object
                .remove("planning_horizon_occurrences")
                .unwrap(),
            "created_at_utc": object.remove("created_at_utc").unwrap(),
            "updated_at_utc": object.remove("updated_at_utc").unwrap()
        });
        object.insert("config".to_string(), config);
        assert_schema_rejects(schedule_schema, &nested_config);

        fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn emitted_realtime_transcript_schema_excludes_internal_user_content() {
        let output_dir = temp_output_dir("safe-realtime-transcript-event");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let wire_types: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("wire-types.json")).unwrap()).unwrap();
        let transcript = wire_types
            .get("RealtimeTranscriptEvent")
            .expect("safe realtime transcript event must be a top-level SDK schema");
        let schema_text = serde_json::to_string(transcript).expect("schema serializes");
        assert!(schema_text.contains("user_transcript_final"));
        assert!(!schema_text.contains("user_content_final"));

        let validator = jsonschema::validator_for(transcript).expect("schema compiles");
        assert!(validator.is_valid(&serde_json::json!({
            "type": "user_transcript_final",
            "item_id": "item_user",
            "previous_item_id": null,
            "content_index": 0,
            "text": "hello"
        })));
        assert!(!validator.is_valid(&serde_json::json!({
            "type": "user_content_final",
            "item_id": "item_private",
            "previous_item_id": null,
            "content_index": 0,
            "content": []
        })));

        fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn emitted_auth_connection_contracts_carry_closed_web_vocabularies() {
        let output_dir = temp_output_dir("auth-connection-contracts");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let contracts: serde_json::Value = serde_json::from_slice(
            &fs::read(output_dir.join("auth-connection-contracts.json")).unwrap(),
        )
        .unwrap();

        assert_eq!(
            contracts["providers"],
            serde_json::json!(["anthropic", "openai", "gemini", "self_hosted"])
        );
        assert!(
            contracts["backend_kinds"]
                .as_array()
                .expect("backend kinds")
                .contains(&serde_json::json!("openai_api")),
            "backend vocabulary must include provider-matrix kinds"
        );
        assert!(
            contracts["auth_methods"]
                .as_array()
                .expect("auth methods")
                .contains(&serde_json::json!("managed_chatgpt_oauth")),
            "auth vocabulary must include provider-matrix methods"
        );
        assert!(
            contracts["credential_source_kinds"]
                .as_array()
                .expect("credential source kinds")
                .contains(&serde_json::json!("external_resolver")),
            "source vocabulary must include CredentialSourceSpec labels"
        );
        assert_eq!(
            contracts["auth_status_states"],
            serde_json::json!([
                "valid",
                "expiring",
                "expired",
                "reauth_required",
                "refresh_failed",
                "released",
                "absent",
                "missing_credential"
            ])
        );
        assert_eq!(
            contracts["provider_backend_kinds"]["openai"],
            serde_json::json!(["openai_api", "chatgpt_backend", "azure_openai"])
        );
        assert!(
            contracts["provider_auth_methods"]["openai"]
                .as_array()
                .expect("openai auth methods")
                .contains(&serde_json::json!("azure_api_key")),
            "provider auth relation map must include Azure OpenAI api-key auth"
        );
        assert!(
            contracts["provider_auth_methods"]["openai"]
                .as_array()
                .expect("openai auth methods")
                .contains(&serde_json::json!("managed_chatgpt_oauth")),
            "provider auth relation map must include provider-specific methods"
        );
        assert!(
            !contracts["provider_auth_methods"]["openai"]
                .as_array()
                .expect("openai auth methods")
                .contains(&serde_json::json!("google_oauth")),
            "provider auth relation map must not flatten other provider methods"
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
            "turn_tool_overlay",
            "additional_instructions",
            "keep_alive",
            "model",
            "provider",
            "max_tokens",
            "system_prompt",
            "output_schema",
            "structured_output_retries",
            "provider_params",
            "auth_binding",
        ] {
            assert!(
                properties.contains_key(field),
                "mob/turn_start params missing explicit turn override field {field}"
            );
        }
        // The legacy `clear_*: bool` split is gone: `provider_params` and
        // `auth_binding` now carry the tri-state `WireTurnMetadataOverride`
        // shape (a oneOf with a `set`/`clear` `action` discriminant). Confirm
        // the `clear_*` siblings are no longer emitted and that each override
        // property resolves to that tagged shape.
        for legacy in ["clear_provider_params", "clear_auth_binding"] {
            assert!(
                !properties.contains_key(legacy),
                "mob/turn_start params must not expose retired split field {legacy}"
            );
        }
        let defs = turn_start
            .pointer("/$defs")
            .and_then(serde_json::Value::as_object)
            .expect("MobTurnStartParams schema must expose $defs");
        for field in ["provider_params", "auth_binding"] {
            let ref_path = properties[field]
                .pointer("/anyOf/0/$ref")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("{field} must reference a tri-state override schema"));
            let def_name = ref_path.strip_prefix("#/$defs/").unwrap_or_else(|| {
                panic!("{field} override $ref must point into $defs: {ref_path}")
            });
            let override_schema = defs
                .get(def_name)
                .unwrap_or_else(|| panic!("override def {def_name} must be emitted"));
            let variants = override_schema
                .pointer("/oneOf")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| panic!("{field} override must be a oneOf tri-state"));
            let actions: Vec<&str> = variants
                .iter()
                .filter_map(|variant| {
                    variant
                        .pointer("/properties/action/const")
                        .and_then(serde_json::Value::as_str)
                })
                .collect();
            assert!(
                actions.contains(&"set") && actions.contains(&"clear"),
                "{field} override must expose set/clear `action` discriminant, got {actions:?}"
            );
        }
        // `MobTurnStartParams` now folds the legacy split form via a custom
        // `Deserialize` over an internal `deny_unknown_fields` shadow struct, so
        // the derived schema no longer carries a top-level
        // `additionalProperties: false` annotation. The fail-closed guarantee is
        // preserved at the deserializer instead: arbitrary flattened overrides
        // are rejected at the serde boundary.
        let unknown = serde_json::json!({
            "mob_id": "m1",
            "agent_identity": "w1",
            "prompt": "continue",
            "unexpected_override": true
        });
        let err = serde_json::from_value::<crate::wire::MobTurnStartParams>(unknown)
            .expect_err("mob/turn_start params must fail closed on unknown overrides");
        assert!(
            err.to_string().contains("unknown field"),
            "unexpected error: {err}"
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
        // `display_name` is `Option<PeerName>`: schemars emits the nullable
        // union with the typed ref as its first arm.
        assert_eq!(
            terminal_properties
                .get("display_name")
                .and_then(|schema| schema.pointer("/anyOf/0/$ref"))
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

        // Drift-regression lock: the create-session component is emitted from
        // the actual wire struct, so surface fields (the historically missing
        // `enable_web_search`) are always part of the advertised contract.
        let create_session_properties = components
            .get("RestCreateSessionRequest")
            .and_then(|schema| schema.pointer("/properties"))
            .and_then(serde_json::Value::as_object)
            .expect("RestCreateSessionRequest must expose object properties");
        for surface_field in ["enable_web_search", "prompt", "keep_alive", "skill_refs"] {
            assert!(
                create_session_properties.contains_key(surface_field),
                "RestCreateSessionRequest schema missing live surface field {surface_field}"
            );
        }
        let continue_session_properties = components
            .get("RestContinueSessionRequest")
            .and_then(|schema| schema.pointer("/properties"))
            .and_then(serde_json::Value::as_object)
            .expect("RestContinueSessionRequest must expose object properties");
        assert!(
            continue_session_properties.contains_key("enable_web_search"),
            "RestContinueSessionRequest schema missing live surface field enable_web_search"
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
            Some("#/components/schemas/RestSessionDetailsResponse")
        );

        let workgraph_items = &rest_openapi["paths"]["/workgraph/items"]["get"];
        let workgraph_item_query_names = workgraph_items["parameters"]
            .as_array()
            .expect("workgraph items parameters")
            .iter()
            .filter(|parameter| {
                parameter.get("in").and_then(serde_json::Value::as_str) == Some("query")
            })
            .filter_map(|parameter| parameter.get("name").and_then(serde_json::Value::as_str))
            .collect::<std::collections::BTreeSet<_>>();
        for expected in [
            "realm_id",
            "namespace",
            "all_namespaces",
            "statuses",
            "labels",
            "include_terminal",
            "limit",
        ] {
            assert!(
                workgraph_item_query_names.contains(expected),
                "WorkGraph items REST OpenAPI must expose query parameter {expected}"
            );
        }

        for descriptor in meerkat_workgraph::workgraph_rest_path_catalog() {
            for catalog_operation in descriptor.operations {
                let operation = &rest_openapi["paths"][descriptor.path][catalog_operation.method];
                let expected =
                    format!("#/components/schemas/{}", catalog_operation.response_schema);
                assert_eq!(
                    operation
                        .pointer("/responses/200/content/application~1json/schema/$ref")
                        .and_then(serde_json::Value::as_str),
                    Some(expected.as_str()),
                    "{} {} must be present in generated REST OpenAPI",
                    catalog_operation.method,
                    descriptor.path
                );
                if let Some(request_schema) = catalog_operation.request_schema {
                    let expected_request = format!("#/components/schemas/{request_schema}");
                    assert_eq!(
                        operation
                            .pointer("/requestBody/content/application~1json/schema/$ref")
                            .and_then(serde_json::Value::as_str),
                        Some(expected_request.as_str()),
                        "{} {} must expose its generated REST request body",
                        catalog_operation.method,
                        descriptor.path
                    );
                }
            }
        }

        for retired in [
            "/sessions/{id}/submit",
            "/sessions/{id}/retire",
            "/sessions/{id}/reset",
            "/sessions/{id}/submissions",
            "/sessions/{session_id}/submissions/{submission_id}",
        ] {
            assert!(
                rest_openapi["paths"].get(retired).is_none(),
                "retired REST runtime/session control mirror must not be emitted: {retired}"
            );
        }
        let components_json = serde_json::to_string(components).unwrap();
        for retired_schema in [
            "RuntimeAcceptParams",
            "RuntimeAcceptResult",
            "RuntimeAcceptOutcomeType",
            "RuntimeRetireParams",
            "RuntimeResetParams",
            "RuntimeRetireResult",
            "RuntimeResetResult",
            "InputStateParams",
            "InputListParams",
            "InputStateResult",
            "InputListResult",
            "WireInputLifecycleState",
            "WireInputState",
            "WireInputStateHistoryEntry",
        ] {
            assert!(
                !components_json.contains(retired_schema),
                "old REST runtime/session control schema must not be carried by OpenAPI: {retired_schema}"
            );
        }

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
        let wire_members_batch = &rest_openapi["paths"]["/mob/{id}/wire-members-batch"]["post"];
        assert_eq!(
            wire_members_batch
                .pointer("/requestBody/content/application~1json/schema/$ref")
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/RestMobWireMembersBatchRequest")
        );
        assert_eq!(
            wire_members_batch
                .pointer("/responses/200/content/application~1json/schema/$ref")
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/MobWireMembersBatchResult")
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
        fn collect_openapi_component_refs(
            value: &serde_json::Value,
            refs: &mut std::collections::BTreeSet<String>,
        ) {
            match value {
                serde_json::Value::Object(object) => {
                    if let Some(serde_json::Value::String(reference)) = object.get("$ref")
                        && let Some(name) = reference.strip_prefix("#/components/schemas/")
                    {
                        refs.insert(name.to_string());
                    }
                    for nested in object.values() {
                        collect_openapi_component_refs(nested, refs);
                    }
                }
                serde_json::Value::Array(values) => {
                    for nested in values {
                        collect_openapi_component_refs(nested, refs);
                    }
                }
                _ => {}
            }
        }
        let mut refs = std::collections::BTreeSet::new();
        collect_openapi_component_refs(&rest_openapi, &mut refs);
        for reference in refs {
            assert!(
                components.contains_key(&reference),
                "OpenAPI component ref must resolve: {reference}"
            );
        }

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

    #[test]
    fn emitted_workgraph_rpc_contract_names_resolve_to_exported_schemas() {
        let output_dir = temp_output_dir("typed-workgraph-rpc-catalog-resolution");
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
            if !(name.starts_with("workgraph/goal/") || name.starts_with("workgraph/attention/")) {
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
    fn emitted_workgraph_goal_attention_contracts_are_exported() {
        let output_dir = temp_output_dir("workgraph-goal-attention-contracts");
        emit_all_schemas(&output_dir).expect("emit schemas");

        let wire_types: serde_json::Value =
            serde_json::from_slice(&fs::read(output_dir.join("wire-types.json")).unwrap()).unwrap();
        let contracts = wire_types.as_object().expect("wire-types object");
        for expected in [
            "AttentionBindingRequest",
            "AttentionBindingResult",
            "AttentionContinueOutcome",
            "AttentionContinueResult",
            "AttentionDelegatedAuthority",
            "AttentionContextProjection",
            "AttentionListRequest",
            "AttentionListResult",
            "AttentionPauseRequest",
            "AttentionProjectionPolicy",
            "AttentionProjectionRequest",
            "AttentionProjectionResult",
            "AttentionProjectionText",
            "AttentionReassignRequest",
            "AttentionReassignResult",
            "GoalAttentionTarget",
            "GoalConfirmRequest",
            "GoalConfirmResult",
            "GoalCreateRequest",
            "GoalCreateResult",
            "GoalRequestCloseRequest",
            "GoalRequestCloseResult",
            "GoalTerminalStatus",
            "GoalStatusRequest",
            "GoalStatusResult",
            "PolicyEscalateRequest",
            "PublicGoalCompletionPolicy",
            "PublicGoalCreateRequest",
            "PublicGoalRequestCloseRequest",
            "ProjectedAttentionAuthority",
            "WorkAttentionBinding",
            "WorkAttentionBindingId",
            "WorkAttentionMode",
            "WorkAttentionStatus",
            "WorkAttentionTarget",
            "WorkCompletionPolicy",
            "WorkItemRef",
        ] {
            assert!(
                contracts.contains_key(expected),
                "missing WorkGraph goal/attention schema {expected}"
            );
        }

        let goal_create = contracts
            .get("GoalCreateRequest")
            .expect("GoalCreateRequest schema");
        assert!(
            goal_create
                .pointer("/properties/target")
                .is_some_and(|target| target.to_string().contains("GoalAttentionTarget")),
            "goal create must use the narrow host target contract"
        );

        fs::remove_dir_all(&output_dir).unwrap();
    }
}
