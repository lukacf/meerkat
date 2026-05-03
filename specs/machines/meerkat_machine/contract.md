# MeerkatMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::meerkat_machine`

## State
- Phase enum: `Initializing | Idle | Attached | Running | Retired | Stopped | Destroyed`
- `session_id`: `Option<SessionId>`
- `active_runtime_id`: `Option<AgentRuntimeId>`
- `active_fence_token`: `Option<FenceToken>`
- `current_run_id`: `Option<RunId>`
- `pre_run_phase`: `Option<PreRunPhase>`
- `turn_phase`: `TurnPhase`
- `primitive_kind`: `Option<TurnPrimitiveKind>`
- `admitted_content_shape`: `Option<ContentShape>`
- `vision_enabled`: `Bool`
- `image_tool_results_enabled`: `Bool`
- `tool_calls_pending`: `u64`
- `pending_op_refs`: `Set<String>`
- `barrier_operation_ids`: `Set<String>`
- `has_barrier_ops`: `Bool`
- `barrier_satisfied`: `Bool`
- `boundary_count`: `u64`
- `cancel_after_boundary`: `Bool`
- `terminal_outcome`: `Option<TurnTerminalOutcome>`
- `terminal_cause_kind`: `Option<TurnTerminalCauseKind>`
- `last_runtime_apply_failure_cause`: `Option<RuntimeApplyFailureCause>`
- `last_runtime_apply_failure_message`: `Option<String>`
- `extraction_attempts`: `u64`
- `max_extraction_retries`: `u64`
- `llm_retry_attempt`: `u64`
- `llm_retry_max_retries`: `u64`
- `llm_retry_selected_delay_ms`: `u64`
- `llm_retry_last_failure_kind`: `Option<LlmRetryFailureKind>`
- `silent_intent_overrides`: `Set<String>`
- `model_routing_baseline_model`: `Option<String>`
- `model_routing_baseline_realtime`: `Option<Bool>`
- `model_routing_topology_epoch`: `u64`
- `model_routing_turn_override_id`: `Option<String>`
- `model_routing_turn_request_id`: `Option<String>`
- `model_routing_turn_target_model`: `Option<String>`
- `model_routing_turn_realtime`: `Option<Bool>`
- `model_routing_turn_remaining_turns`: `Option<u64>`
- `model_routing_operation_override_id`: `Option<String>`
- `model_routing_operation_target_model`: `Option<String>`
- `model_routing_operation_realtime`: `Option<Bool>`
- `model_routing_pending_switch_request_id`: `Option<String>`
- `model_routing_pending_switch_target_model`: `Option<String>`
- `model_routing_pending_switch_realtime`: `Option<Bool>`
- `model_routing_pending_switch_turns`: `Option<u64>`
- `model_routing_pending_switch_phase`: `Option<RoutingSwitchTurnPhase>`
- `model_routing_switch_terminal`: `Map<String, RoutingSwitchTurnTerminal>`
- `model_routing_switch_denials`: `Map<String, RoutingDenialReason>`
- `model_routing_image_operation_phases`: `Map<String, RoutingImageOperationPhase>`
- `model_routing_image_operation_target_models`: `Map<String, String>`
- `model_routing_image_operation_realtime`: `Map<String, Bool>`
- `model_routing_image_operation_requires_scoped_override`: `Map<String, Bool>`
- `model_routing_image_terminals`: `Map<String, RoutingImageTerminal>`
- `model_routing_image_terminal_payloads`: `Map<String, String>`
- `model_routing_image_denials`: `Map<String, RoutingDenialReason>`
- `model_routing_approval_phases`: `Map<String, RoutingApprovalPhase>`
- `model_routing_approval_parent_kind`: `Map<String, RoutingApprovalParentKind>`
- `registration_phase`: `RegistrationPhase`
- `drain_phase`: `DrainPhase`
- `drain_mode`: `Option<DrainMode>`
- `next_staged_visibility_revision`: `u64`
- `active_filter`: `ToolFilter`
- `staged_filter`: `ToolFilter`
- `active_visibility_revision`: `u64`
- `staged_visibility_revision`: `u64`
- `active_deferred_names`: `Set<String>`
- `staged_deferred_names`: `Set<String>`
- `active_deferred_authorities`: `Map<String, ToolVisibilityWitness>`
- `staged_deferred_authorities`: `Map<String, ToolVisibilityWitness>`
- `input_phases`: `Map<String, InputPhase>`
- `input_terminal_kind`: `Map<String, InputTerminalKind>`
- `input_superseded_by`: `Map<String, String>`
- `input_aggregate_id`: `Map<String, String>`
- `input_abandon_reason`: `Map<String, InputAbandonReason>`
- `input_abandon_attempt_count`: `Map<String, u64>`
- `input_attempt_counts`: `Map<String, u64>`
- `input_run_associations`: `Map<String, String>`
- `input_boundary_sequences`: `Map<String, u64>`
- `next_admission_seq`: `u64`
- `input_admission_seq`: `Map<String, u64>`
- `input_lane`: `Map<String, InputLane>`
- `op_statuses`: `Map<String, OperationStatus>`
- `op_completion_seq`: `Map<String, u64>`
- `op_terminal_outcomes`: `Map<String, OperationTerminalOutcomeKind>`
- `op_terminal_payload`: `Map<String, String>`
- `op_kinds`: `Map<String, OperationKind>`
- `op_peer_ready`: `Map<String, Bool>`
- `op_progress_counts`: `Map<String, u64>`
- `active_op_count`: `u64`
- `wait_active`: `Bool`
- `wait_request_id`: `Option<WaitRequestId>`
- `wait_operation_ids`: `Set<String>`
- `wait_operation_id_tokens`: `Set<OperationId>`
- `next_completion_seq`: `u64`
- `known_surfaces`: `Set<String>`
- `active_surfaces`: `Set<String>`
- `visible_surfaces`: `Set<String>`
- `surface_base_state`: `Map<String, ExternalToolSurfaceBaseState>`
- `surface_pending_op`: `Map<String, SurfacePendingOp>`
- `surface_staged_op`: `Map<String, SurfaceStagedOp>`
- `reload_staged_surfaces`: `Set<String>`
- `surface_staged_intent_sequence`: `Map<String, u64>`
- `next_staged_intent_sequence`: `u64`
- `surface_pending_task_sequence`: `Map<String, u64>`
- `next_pending_task_sequence`: `u64`
- `surface_pending_lineage_sequence`: `Map<String, u64>`
- `surface_inflight_calls`: `Map<String, u64>`
- `surface_last_delta_operation`: `Map<String, ExternalToolSurfaceDeltaOperation>`
- `surface_last_delta_phase`: `Map<String, ExternalToolSurfaceDeltaPhase>`
- `snapshot_epoch`: `u64`
- `snapshot_aligned_epoch`: `u64`
- `surface_draining_since_ms`: `Map<String, u64>`
- `surface_removal_timeout_at_ms`: `Map<String, u64>`
- `surface_removal_applied_at_turn`: `Map<String, u64>`
- `surface_phase`: `SurfacePhase`
- `removal_timeout_ms`: `u64`
- `realtime_intent_present`: `Bool`
- `realtime_binding_state`: `RealtimeBindingState`
- `realtime_binding_authority_epoch`: `Option<u64>`
- `realtime_reattach_required`: `Bool`
- `realtime_next_authority_epoch`: `u64`
- `realtime_reconnect_cycle_state`: `RealtimeReconnectCycleState`
- `realtime_reconnect_attempt_count`: `u64`
- `realtime_reconnect_next_retry_at_ms`: `Option<u64>`
- `realtime_reconnect_deadline_at_ms`: `Option<u64>`
- `live_topology_phase`: `LiveTopologyPhase`
- `mcp_server_states`: `Map<McpServerId, McpServerState>`
- `pending_peer_requests`: `Map<PeerCorrelationId, OutboundPeerRequestState>`
- `inbound_peer_requests`: `Map<PeerCorrelationId, InboundPeerRequestState>`
- `last_session_context_updated_at_ms`: `u64`
- `reserved_interaction_streams`: `Set<PeerCorrelationId>`
- `attached_interaction_streams`: `Set<PeerCorrelationId>`
- `realtime_product_turn_phase`: `RealtimeProductTurnPhase`
- `realtime_projection_freshness`: `RealtimeProjectionFreshness`
- `realtime_projection_frontier_ms`: `u64`
- `realtime_reconnect_policy`: `RealtimeReconnectPolicy`
- `peer_ingress_owner_kind`: `PeerIngressOwnerKind`
- `peer_ingress_comms_runtime_id`: `Option<CommsRuntimeId>`
- `peer_ingress_mob_id`: `Option<MobId>`
- `supervisor_binding_kind`: `SupervisorBindingKind`
- `supervisor_bound_name`: `Option<String>`
- `supervisor_bound_peer_id`: `Option<String>`
- `supervisor_bound_address`: `Option<String>`
- `supervisor_bound_epoch`: `Option<u64>`
- `local_endpoint`: `Option<PeerEndpoint>`
- `direct_peer_endpoints`: `Set<PeerEndpoint>`
- `mob_overlay_peer_endpoints`: `Set<PeerEndpoint>`
- `peer_projection_epoch`: `u64`
- `mob_overlay_epoch`: `u64`

## Inputs
- `RegisterSession`(session_id: SessionId)
- `UnregisterSession`(session_id: SessionId)
- `ReconfigureSessionLlmIdentity`(previous_identity: SessionLlmIdentity, previous_visibility_state: SessionToolVisibilityState, previous_capability_surface: Option<SessionLlmCapabilitySurface>, previous_capability_surface_status: SessionLlmCapabilitySurfaceStatus, target_identity: SessionLlmIdentity, target_capability_surface: SessionLlmCapabilitySurface, next_visibility_state: SessionToolVisibilityState, next_capability_base_filter: ToolFilter, next_active_visibility_revision: u64, tool_visibility_delta: SessionToolVisibilityDelta)
- `PrepareBindings`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: SessionId)
- `SetPeerIngressContext`(keep_alive: Bool)
- `NotifyDrainExited`(reason: DrainExitReason)
- `InterruptCurrentRun`
- `CancelAfterBoundary`(reason: String)
- `StagePersistentFilter`(filter: ToolFilter, witnesses: Map<String, ToolVisibilityWitness>)
- `RequestDeferredTools`(authorities: Map<String, ToolVisibilityWitness>)
- `PublishCommittedVisibleSet`(active_filter: ToolFilter, staged_filter: ToolFilter, active_requested_deferred_names: Set<String>, staged_requested_deferred_names: Set<String>, active_deferred_authorities: Map<String, ToolVisibilityWitness>, staged_deferred_authorities: Map<String, ToolVisibilityWitness>, active_visibility_revision: u64, staged_visibility_revision: u64)
- `Recover`
- `Retire`(session_id: SessionId)
- `Reset`
- `StopRuntimeExecutor`(reason: String)
- `RuntimeExecutorExited`
- `Destroy`(session_id: SessionId)
- `EnsureSessionWithExecutor`(session_id: SessionId)
- `SetSilentIntents`(session_id: SessionId, intents: Set<String>)
- `ContainsSession`(session_id: SessionId)
- `SessionHasExecutor`(session_id: SessionId)
- `SessionHasComms`(session_id: SessionId)
- `OpsLifecycleRegistry`(session_id: SessionId)
- `InputState`(session_id: SessionId, input_id: InputId)
- `ListActiveInputs`(session_id: SessionId)
- `Abort`(session_id: SessionId)
- `AbortAll`
- `Wait`(session_id: SessionId)
- `Ingest`(runtime_id: AgentRuntimeId, work_id: WorkId, origin: WorkOrigin)
- `PublishEvent`(kind: String)
- `RuntimeState`(runtime_id: String)
- `RuntimeRealtimeAttachmentStatus`(session_id: SessionId)
- `ModelRoutingStatus`(session_id: SessionId)
- `SetModelRoutingBaseline`(baseline_model: String, realtime_capable: Bool)
- `RequestFiniteSwitchTurn`(request_id: String, target_model: String, turns: u64, target_realtime_capable: Bool, requires_approval: Bool, approval_available: Bool, approval_denied: Bool, realtime_detach_allowed: Bool)
- `RequestUntilChangedSwitchTurn`(request_id: String, target_model: String, target_realtime_capable: Bool, requires_approval: Bool, approval_available: Bool, approval_denied: Bool, realtime_detach_allowed: Bool)
- `CompleteUntilChangedSwitchTurnReconfigure`(request_id: String)
- `AdmitModelRoutingAssistantTurn`
- `BeginImageOperation`(operation_id: String, target_model: String, target_realtime_capable: Bool, requires_approval: Bool, approval_available: Bool, approval_denied: Bool, realtime_detach_allowed: Bool, requires_scoped_override: Bool)
- `ActivateImageOperationOverride`(operation_id: String, target_model: String, target_realtime_capable: Bool)
- `CompleteImageOperation`(operation_id: String, terminal: RoutingImageTerminal, terminal_payload: String)
- `RestoreImageOperationOverride`(operation_id: String)
- `LoadBoundaryReceipt`(runtime_id: String, sequence: u64)
- `AcceptWithCompletion`(input_id: InputId, request_immediate_processing: Bool, interrupt_yielding: Bool, wake_if_idle: Bool)
- `AcceptWithoutWake`(input_id: InputId)
- `Prepare`(session_id: SessionId, run_id: RunId)
- `Commit`(input_id: InputId, run_id: RunId)
- `Fail`(run_id: RunId)
- `RollbackRun`(run_id: RunId)
- `Recycle`
- `StartConversationRun`(run_id: RunId, primitive_kind: TurnPrimitiveKind, admitted_content_shape: ContentShape, vision_enabled: Bool, image_tool_results_enabled: Bool, max_extraction_retries: u64)
- `StartImmediateAppend`(run_id: RunId)
- `StartImmediateContext`(run_id: RunId)
- `PrimitiveApplied`
- `LlmReturnedToolCalls`(tool_count: u64)
- `LlmReturnedTerminal`
- `RegisterPendingOps`(op_refs: Set<String>, barrier_operation_ids: Set<String>)
- `ToolCallsResolved`
- `OpsBarrierSatisfied`(operation_ids: Set<String>)
- `BoundaryContinue`
- `BoundaryComplete`
- `EnterExtraction`(max_extraction_retries: u64)
- `ExtractionStart`
- `ExtractionValidationPassed`
- `ExtractionValidationFailed`(error: String)
- `RecoverableFailure`(failure_kind: LlmRetryFailureKind, retry_attempt: u64, max_retries: u64, selected_delay_ms: u64, error: String)
- `FatalFailure`(terminal_cause_kind: TurnTerminalCauseKind, error: String)
- `RetryRequested`(retry_attempt: u64)
- `CancelNow`
- `RequestCancelAfterBoundary`
- `CancellationObserved`
- `AcknowledgeTerminal`(outcome: TurnTerminalOutcome)
- `TurnLimitReached`
- `BudgetExhausted`
- `TimeBudgetExceeded`
- `ForceCancelNoRun`
- `RunCompleted`(run_id: RunId)
- `RunFailed`(run_id: RunId, runtime_apply_failure_cause: Option<RuntimeApplyFailureCause>, runtime_apply_failure_message: Option<String>, terminal_cause_kind: TurnTerminalCauseKind, error: String)
- `RunCancelled`(run_id: RunId)
- `RecoverInputLifecycle`(input_id: String, phase: InputPhase, terminal_kind: Option<InputTerminalKind>, superseded_by: Option<String>, aggregate_id: Option<String>, abandon_reason: Option<InputAbandonReason>, abandon_attempt_count: u64, attempt_count: u64, run_id: Option<String>, boundary_sequence: Option<u64>, lane: Option<InputLane>)
- `QueueAccepted`(input_id: String)
- `SteerAccepted`(input_id: String)
- `ChangeLane`(input_id: String, new_lane: InputLane)
- `StageForRun`(input_id: String, run_id: String)
- `IncrementAttemptCount`(input_id: String)
- `RollbackStaged`(input_id: String, lane: InputLane)
- `MarkApplied`(input_id: String)
- `MarkAppliedPendingConsumption`(input_id: String)
- `ConsumeInput`(input_id: String)
- `ConsumeOnAccept`(input_id: String)
- `SupersedeInput`(input_id: String, superseded_by: String)
- `CoalesceInput`(input_id: String, aggregate_id: String)
- `AbandonInput`(input_id: String, reason: InputAbandonReason, attempt_count: u64)
- `RecordBoundarySeq`(input_id: String, seq: u64)
- `RegisterOp`(operation_id: String, kind: OperationKind)
- `StartOp`(operation_id: String)
- `CompleteOp`(operation_id: String, outcome: OperationTerminalOutcomeKind, payload: String)
- `FailOp`(operation_id: String, outcome: OperationTerminalOutcomeKind, payload: String)
- `CancelOp`(operation_id: String, outcome: OperationTerminalOutcomeKind, payload: String)
- `AbortOp`(operation_id: String, outcome: OperationTerminalOutcomeKind, payload: String)
- `PeerReadyOp`(operation_id: String)
- `ProgressReportedOp`(operation_id: String)
- `RetireRequestedOp`(operation_id: String)
- `RetireCompletedOp`(operation_id: String, outcome: OperationTerminalOutcomeKind, payload: String)
- `TerminateOp`(operation_id: String, outcome: OperationTerminalOutcomeKind, payload: String)
- `RequestWaitAll`(wait_request_id: WaitRequestId, operation_ids: Set<String>, operation_id_tokens: Set<OperationId>)
- `SatisfyWaitAll`(wait_request_id: WaitRequestId, operation_id_tokens: Set<OperationId>)
- `CancelWaitAll`
- `SpawnDrain`(mode: DrainMode)
- `StopDrain`
- `DrainExitedClean`
- `DrainExitedRespawnable`
- `StageVisibilityFilter`(filter: ToolFilter)
- `CommitVisibilityFilter`(filter: ToolFilter, revision: u64)
- `StageDeferredNames`(names: Set<String>)
- `CommitDeferredNames`(authorities: Map<String, ToolVisibilityWitness>)
- `SyncVisibilityRevisions`(active_revision: u64, staged_revision: u64, active_deferred_names: Set<String>, staged_deferred_names: Set<String>, active_deferred_authorities: Map<String, ToolVisibilityWitness>, staged_deferred_authorities: Map<String, ToolVisibilityWitness>)
- `SurfaceRegister`(surface_id: String)
- `SurfaceStageAdd`(surface_id: String, now_ms: u64)
- `SurfaceStageRemove`(surface_id: String, now_ms: u64)
- `SurfaceStageReload`(surface_id: String, now_ms: u64)
- `SurfaceApplyBoundary`(surface_id: String, now_ms: u64, staged_intent_sequence: u64, applied_at_turn: u64)
- `SurfaceMarkPendingSucceeded`(surface_id: String, pending_task_sequence: u64, staged_intent_sequence: u64)
- `SurfaceMarkPendingFailed`(surface_id: String, pending_task_sequence: u64, staged_intent_sequence: u64, cause: ExternalToolSurfaceFailureCause)
- `SurfaceCallStarted`(surface_id: String)
- `SurfaceCallFinished`(surface_id: String)
- `SurfaceFinalizeRemovalClean`(surface_id: String)
- `SurfaceFinalizeRemovalForced`(surface_id: String)
- `SurfaceSnapshotAligned`(epoch: u64)
- `SurfaceShutdown`
- `ProjectRealtimeIntent`(present: Bool)
- `BeginRealtimeBinding`
- `ReplaceRealtimeBinding`
- `DetachRealtimeBinding`
- `RequireRealtimeReattach`
- `RequireRealtimeReattachForAuthority`(authority_epoch: u64)
- `PublishRealtimeSignal`(authority_epoch: u64, next_binding_state: RealtimeBindingState)
- `BeginRealtimeReconnectCycle`(next_retry_at_ms: Option<u64>, deadline_at_ms: Option<u64>)
- `ScheduleRealtimeReconnectRetry`(next_retry_at_ms: Option<u64>)
- `ExhaustRealtimeReconnectCycle`
- `ClearRealtimeReconnectProgress`
- `BeginLiveTopologyReconfigure`(authority_epoch: u64)
- `MarkLiveTopologyDetached`
- `ApplyLiveTopologyIdentity`
- `ApplyLiveTopologyVisibility`
- `CompleteLiveTopology`
- `AbortLiveTopologyBeforeDetach`
- `FailLiveTopologyAfterDetach`
- `McpServerConnectPending`(server_id: McpServerId)
- `McpServerConnected`(server_id: McpServerId)
- `McpServerFailed`(server_id: McpServerId, error: String)
- `McpServerDisconnected`(server_id: McpServerId)
- `McpServerReload`(server_id: McpServerId)
- `PeerRequestSent`(corr_id: PeerCorrelationId, to: String)
- `PeerResponseProgressArrived`(corr_id: PeerCorrelationId)
- `PeerResponseTerminalArrived`(corr_id: PeerCorrelationId, disposition: PeerTerminalDisposition)
- `PeerRequestTimedOut`(corr_id: PeerCorrelationId)
- `PeerRequestReceived`(corr_id: PeerCorrelationId)
- `PeerResponseReplied`(corr_id: PeerCorrelationId)
- `AdvanceSessionContext`(updated_at_ms: u64)
- `InteractionStreamReserved`(corr_id: PeerCorrelationId)
- `InteractionStreamAttached`(corr_id: PeerCorrelationId)
- `InteractionStreamCompleted`(corr_id: PeerCorrelationId)
- `InteractionStreamExpired`(corr_id: PeerCorrelationId)
- `InteractionStreamClosedEarly`(corr_id: PeerCorrelationId)
- `ProductTurnInFlight`
- `ProductTurnCommitted`
- `ProductOutputStarted`
- `ProductTurnInterrupted`
- `ProductTurnTerminal`
- `RealtimeProjectionAdvanceObserved`(advanced_at_ms: u64)
- `RealtimeProjectionRefreshed`(observed_ms: u64)
- `RealtimeProjectionReset`(baseline_ms: u64)
- `ClassifyRealtimeClientInputSubmitted`
- `ClassifyRealtimeMidTurnActivity`
- `ClassifyRealtimeTurnTerminated`
- `AttachSessionIngress`(comms_runtime_id: CommsRuntimeId)
- `AttachMobIngress`(comms_runtime_id: CommsRuntimeId, mob_id: MobId)
- `DetachIngress`
- `BindSupervisor`(name: String, peer_id: String, address: String, epoch: u64)
- `AuthorizeSupervisor`(name: String, peer_id: String, address: String, epoch: u64)
- `RevokeSupervisor`(peer_id: String, epoch: u64)
- `SupervisorTrustEdgePublished`(peer_id: String, epoch: u64)
- `SupervisorTrustEdgePublishFailed`(peer_id: String, epoch: u64, reason: String)
- `SupervisorTrustEdgeRevoked`(peer_id: String, epoch: u64)
- `SupervisorTrustEdgeRevokeFailed`(peer_id: String, epoch: u64, reason: String)
- `PublishLocalEndpoint`(endpoint: PeerEndpoint)
- `ClearLocalEndpoint`
- `AddDirectPeerEndpoint`(endpoint: PeerEndpoint)
- `RemoveDirectPeerEndpoint`(endpoint: PeerEndpoint)
- `ApplyMobPeerOverlay`(epoch: u64, endpoints: Set<PeerEndpoint>)

## Surface-only Inputs
- `ContainsSession`
- `SessionHasExecutor`
- `SessionHasComms`
- `OpsLifecycleRegistry`
- `InputState`
- `ListActiveInputs`
- `RuntimeState`
- `RuntimeRealtimeAttachmentStatus`
- `ModelRoutingStatus`
- `LoadBoundaryReceipt`

## Signals
- `Initialize`
- `BoundaryApplied`(revision: u64)
- `DrainQueuedRun`(run_id: RunId)
- `ClassifyExternalEnvelope`(item_id: String, from_peer: String, envelope_kind: PeerIngressEnvelopeClass, request_intent: String, lifecycle_kind: PeerIngressLifecycleClass, lifecycle_peer_param: Option<String>, response_status: PeerIngressResponseStatus, in_reply_to: String)
- `ClassifyPlainEvent`(source_name: String)
- `EnsureDrainRunning`

## Effects
- `RuntimeBound`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `RuntimeRetired`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `RuntimeDestroyed`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `TurnRunStarted`(run_id: RunId)
- `TurnBoundaryApplied`(run_id: RunId, boundary_sequence: u64)
- `TurnRunCompleted`(run_id: RunId, outcome: TurnTerminalOutcome)
- `TurnRunFailed`(run_id: RunId, terminal_cause_kind: TurnTerminalCauseKind, error: String)
- `TurnRunCancelled`(run_id: RunId, reason: TurnCancellationReason)
- `TurnCheckCompaction`
- `RequestCancellationAtBoundary`
- `WakeInterrupt`
- `CommittedVisibleSetPublished`(revision: u64)
- `RuntimeNotice`(kind: RuntimeNoticeKind, detail: String)
- `RuntimeEffectFact`(kind: RuntimeEffectKind, reason: String)
- `ModelRoutingStatusChanged`(topology_epoch: u64)
- `SwitchTurnDenied`(request_id: String, reason: RoutingDenialReason)
- `SwitchTurnPersistentReconfigureRequested`(request_id: String, target_model: String)
- `SwitchTurnFiniteOverrideActivated`(request_id: String, target_model: String, turns_remaining: u64)
- `SwitchTurnFiniteOverrideRestored`(request_id: String)
- `ImageOperationPhaseChanged`(operation_id: String, phase: RoutingImageOperationPhase)
- `ImageOperationDenied`(operation_id: String, reason: RoutingDenialReason)
- `ModelRoutingApprovalTerminalized`(approval_id: String, phase: RoutingApprovalPhase)
- `ResolveAdmission`
- `SubmitAdmittedIngressEffect`
- `SubmitRunPrimitive`
- `ResolveCompletionAsTerminated`
- `ApplyControlPlaneCommand`
- `InitiateRecycle`
- `IngressAccepted`
- `PostAdmissionSignal`(signal: PostAdmissionSignalKind)
- `ReadyForRun`
- `InputLifecycleNotice`
- `CompletionResolved`
- `IngressNotice`
- `SilentIntentApplied`
- `CheckCompaction`
- `RecordTerminalOutcome`
- `RecordRunAssociation`
- `RecordBoundarySequence`
- `SubmitOpEvent`(operation_id: String)
- `NotifyOpWatcher`(operation_id: String)
- `ExposeOperationPeer`(operation_id: String)
- `RetainTerminalRecord`(operation_id: String)
- `EvictCompletedRecord`(operation_id: String)
- `CompletionProduced`(seq: u64, operation_id: OperationId, kind: OperationKind)
- `WaitAllSatisfied`(wait_request_id: WaitRequestId, operation_ids: Set<OperationId>)
- `CollectCompletedResult`
- `EnqueueClassifiedEntry`
- `PeerIngressClassified`(class: PeerIngressInputClass, kind: PeerIngressAdmittedKind, auth: PeerIngressAuthClass, lifecycle_kind: Option<PeerIngressLifecycleClass>, lifecycle_peer: Option<String>, request_id: Option<String>, response_terminality: Option<PeerIngressResponseTerminality>)
- `SpawnDrainTask`
- `ScheduleSurfaceCompletion`(surface_id: String, operation: ExternalToolSurfaceDeltaOperation, pending_task_sequence: u64, staged_intent_sequence: u64, applied_at_turn: u64)
- `RefreshVisibleSurfaceSet`(snapshot_epoch: u64)
- `EmitExternalToolDelta`(surface_id: String, operation: ExternalToolSurfaceDeltaOperation, phase: ExternalToolSurfaceDeltaPhase, cause: Option<ExternalToolSurfaceFailureCause>)
- `CloseSurfaceConnection`(surface_id: String)
- `RejectSurfaceCall`(surface_id: String, cause: ExternalToolSurfaceFailureCause)
- `PublishSupervisorTrustEdge`(peer_id: String, name: String, address: String, signing_public_key: Option<String>, epoch: u64)
- `RevokeSupervisorTrustEdge`(peer_id: String, epoch: u64)
- `RealtimeIntentProjected`(present: Bool)
- `RealtimeBindingRotated`(authority_epoch: u64)
- `RealtimeReconnectProgressProjected`(attempt_count: u64, next_retry_at_ms: Option<u64>, deadline_at_ms: Option<u64>)
- `LiveTopologyPhaseChanged`
- `McpServerStateChanged`(server_id: McpServerId, new_state: McpServerState)
- `McpServerReloadRequested`(server_id: McpServerId)
- `PeerInteractionStateChanged`(corr_id: PeerCorrelationId, new_state: OutboundPeerRequestState)
- `PeerInteractionCleanup`(corr_id: PeerCorrelationId)
- `InboundPeerInteractionStateChanged`(corr_id: PeerCorrelationId, new_state: InboundPeerRequestState)
- `SessionContextAdvanced`(updated_at_ms: u64)
- `InteractionStreamStateChanged`(corr_id: PeerCorrelationId, new_state: InteractionStreamState)
- `InteractionStreamCleanup`(corr_id: PeerCorrelationId)
- `RealtimeProductTurnPhaseChanged`(new_phase: RealtimeProductTurnPhase)
- `RealtimeProjectionFreshnessChanged`(new_freshness: RealtimeProjectionFreshness, frontier_ms: u64)
- `RealtimeReconnectPolicyChanged`(new_policy: RealtimeReconnectPolicy)
- `LocalEndpointChanged`(endpoint: Option<PeerEndpoint>)
- `PeerProjectionChanged`(peer_projection_epoch: u64)
- `CommsTrustReconcileRequested`(peer_projection_epoch: u64)

## Helpers
- `deferred_authority_has_identity`(witness: ToolVisibilityWitness) -> `Bool`
- `deferred_authorities_have_identity`(names: Set<String>, witnesses: Map<String, ToolVisibilityWitness>) -> `Bool`

## Invariants
- `fence_requires_bound_runtime`
- `running_has_current_run`
- `current_run_only_while_running_or_retired`
- `staged_surface_ops_are_known_and_sequenced`
- `staged_reload_surfaces_are_active`
- `realtime_binding_epoch_consistency`
- `peer_ingress_owner_consistency`
- `supervisor_binding_consistency`

## Transitions
### `Initialize`
- From: `Initializing`
- On: `Initialize`()
- To: `Idle`

### `RegisterSessionIdle`
- From: `Idle`
- On: `RegisterSession`(session_id)
- To: `Idle`

### `RegisterSessionAttached`
- From: `Attached`
- On: `RegisterSession`(session_id)
- To: `Attached`

### `RegisterSessionRunning`
- From: `Running`
- On: `RegisterSession`(session_id)
- To: `Running`

### `RegisterSessionRetired`
- From: `Retired`
- On: `RegisterSession`(session_id)
- To: `Retired`

### `RegisterSessionStopped`
- From: `Stopped`
- On: `RegisterSession`(session_id)
- To: `Stopped`

### `UnregisterSessionIdle`
- From: `Idle`
- On: `UnregisterSession`(session_id)
- Guards:
  - `session_matches_current`
- To: `Idle`

### `UnregisterSessionAttached`
- From: `Attached`
- On: `UnregisterSession`(session_id)
- Guards:
  - `session_matches_current`
- To: `Idle`

### `UnregisterSessionRunning`
- From: `Running`
- On: `UnregisterSession`(session_id)
- Guards:
  - `session_matches_current`
- To: `Idle`

### `UnregisterSessionRetired`
- From: `Retired`
- On: `UnregisterSession`(session_id)
- Guards:
  - `session_matches_current`
- To: `Idle`

### `UnregisterSessionStopped`
- From: `Stopped`
- On: `UnregisterSession`(session_id)
- Guards:
  - `session_matches_current`
- To: `Idle`

### `ReconfigureSessionLlmIdentityAttached`
- From: `Attached`
- On: `ReconfigureSessionLlmIdentity`(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta)
- Guards:
  - `session_registered`
  - `runtime_is_bound`
- To: `Attached`

### `ReconfigureSessionLlmIdentityRunning`
- From: `Running`
- On: `ReconfigureSessionLlmIdentity`(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta)
- Guards:
  - `session_registered`
  - `runtime_is_bound`
- To: `Running`

### `SetModelRoutingBaselineIdle`
- From: `Idle`
- On: `SetModelRoutingBaseline`(baseline_model, realtime_capable)
- Guards:
  - `session_registered`
- Emits: `ModelRoutingStatusChanged`
- To: `Idle`

### `SetModelRoutingBaselineAttached`
- From: `Attached`
- On: `SetModelRoutingBaseline`(baseline_model, realtime_capable)
- Guards:
  - `session_registered`
- Emits: `ModelRoutingStatusChanged`
- To: `Attached`

### `SetModelRoutingBaselineRunning`
- From: `Running`
- On: `SetModelRoutingBaseline`(baseline_model, realtime_capable)
- Guards:
  - `session_registered`
- Emits: `ModelRoutingStatusChanged`
- To: `Running`

### `SetModelRoutingBaselineRetired`
- From: `Retired`
- On: `SetModelRoutingBaseline`(baseline_model, realtime_capable)
- Guards:
  - `session_registered`
- Emits: `ModelRoutingStatusChanged`
- To: `Retired`

### `SetModelRoutingBaselineStopped`
- From: `Stopped`
- On: `SetModelRoutingBaseline`(baseline_model, realtime_capable)
- Guards:
  - `session_registered`
- Emits: `ModelRoutingStatusChanged`
- To: `Stopped`

### `RequestFiniteSwitchTurnApprovalUnavailableIdle`
- From: `Idle`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_unavailable`
- Emits: `SwitchTurnDenied`
- To: `Idle`

### `RequestFiniteSwitchTurnApprovalUnavailableAttached`
- From: `Attached`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_unavailable`
- Emits: `SwitchTurnDenied`
- To: `Attached`

### `RequestFiniteSwitchTurnApprovalUnavailableRunning`
- From: `Running`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_unavailable`
- Emits: `SwitchTurnDenied`
- To: `Running`

### `RequestFiniteSwitchTurnApprovalDeniedIdle`
- From: `Idle`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `approval_denied`
- Emits: `SwitchTurnDenied`, `ModelRoutingApprovalTerminalized`
- To: `Idle`

### `RequestFiniteSwitchTurnApprovalDeniedAttached`
- From: `Attached`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `approval_denied`
- Emits: `SwitchTurnDenied`, `ModelRoutingApprovalTerminalized`
- To: `Attached`

### `RequestFiniteSwitchTurnApprovalDeniedRunning`
- From: `Running`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `approval_denied`
- Emits: `SwitchTurnDenied`, `ModelRoutingApprovalTerminalized`
- To: `Running`

### `RequestFiniteSwitchTurnRealtimeConflictIdle`
- From: `Idle`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `realtime_conflict`
- Emits: `SwitchTurnDenied`
- To: `Idle`

### `RequestFiniteSwitchTurnRealtimeConflictAttached`
- From: `Attached`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `realtime_conflict`
- Emits: `SwitchTurnDenied`
- To: `Attached`

### `RequestFiniteSwitchTurnRealtimeConflictRunning`
- From: `Running`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `realtime_conflict`
- Emits: `SwitchTurnDenied`
- To: `Running`

### `RequestFiniteSwitchTurnScopedConflictIdle`
- From: `Idle`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `scoped_conflict`
- Emits: `SwitchTurnDenied`
- To: `Idle`

### `RequestFiniteSwitchTurnScopedConflictAttached`
- From: `Attached`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `scoped_conflict`
- Emits: `SwitchTurnDenied`
- To: `Attached`

### `RequestFiniteSwitchTurnScopedConflictRunning`
- From: `Running`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `scoped_conflict`
- Emits: `SwitchTurnDenied`
- To: `Running`

### `RequestFiniteSwitchTurnAcceptedIdle`
- From: `Idle`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `positive_turns`
  - `approval_satisfied`
  - `no_realtime_conflict`
  - `no_scoped_conflict`
- To: `Idle`

### `RequestFiniteSwitchTurnAcceptedAttached`
- From: `Attached`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `positive_turns`
  - `approval_satisfied`
  - `no_realtime_conflict`
  - `no_scoped_conflict`
- To: `Attached`

### `RequestFiniteSwitchTurnAcceptedRunning`
- From: `Running`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `positive_turns`
  - `approval_satisfied`
  - `no_realtime_conflict`
  - `no_scoped_conflict`
- To: `Running`

### `RequestUntilChangedSwitchTurnAcceptedIdle`
- From: `Idle`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_satisfied`
  - `no_realtime_conflict`
- Emits: `SwitchTurnPersistentReconfigureRequested`
- To: `Idle`

### `RequestUntilChangedSwitchTurnAcceptedAttached`
- From: `Attached`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_satisfied`
  - `no_realtime_conflict`
- Emits: `SwitchTurnPersistentReconfigureRequested`
- To: `Attached`

### `RequestUntilChangedSwitchTurnAcceptedRunning`
- From: `Running`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_satisfied`
  - `no_realtime_conflict`
- Emits: `SwitchTurnPersistentReconfigureRequested`
- To: `Running`

### `RequestUntilChangedSwitchTurnRealtimeConflictIdle`
- From: `Idle`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `realtime_conflict`
- Emits: `SwitchTurnDenied`
- To: `Idle`

### `RequestUntilChangedSwitchTurnRealtimeConflictAttached`
- From: `Attached`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `realtime_conflict`
- Emits: `SwitchTurnDenied`
- To: `Attached`

### `RequestUntilChangedSwitchTurnRealtimeConflictRunning`
- From: `Running`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `realtime_conflict`
- Emits: `SwitchTurnDenied`
- To: `Running`

### `RequestUntilChangedSwitchTurnApprovalUnavailableIdle`
- From: `Idle`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_unavailable`
- Emits: `SwitchTurnDenied`
- To: `Idle`

### `RequestUntilChangedSwitchTurnApprovalUnavailableAttached`
- From: `Attached`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_unavailable`
- Emits: `SwitchTurnDenied`
- To: `Attached`

### `RequestUntilChangedSwitchTurnApprovalUnavailableRunning`
- From: `Running`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_unavailable`
- Emits: `SwitchTurnDenied`
- To: `Running`

### `RequestUntilChangedSwitchTurnApprovalDeniedIdle`
- From: `Idle`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `approval_denied`
- Emits: `SwitchTurnDenied`, `ModelRoutingApprovalTerminalized`
- To: `Idle`

### `RequestUntilChangedSwitchTurnApprovalDeniedAttached`
- From: `Attached`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `approval_denied`
- Emits: `SwitchTurnDenied`, `ModelRoutingApprovalTerminalized`
- To: `Attached`

### `RequestUntilChangedSwitchTurnApprovalDeniedRunning`
- From: `Running`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed)
- Guards:
  - `approval_denied`
- Emits: `SwitchTurnDenied`, `ModelRoutingApprovalTerminalized`
- To: `Running`

### `CompleteUntilChangedSwitchTurnReconfigureIdle`
- From: `Idle`
- On: `CompleteUntilChangedSwitchTurnReconfigure`(request_id)
- Guards:
  - `pending_until_changed_reconfigure`
- Emits: `ModelRoutingStatusChanged`
- To: `Idle`

### `CompleteUntilChangedSwitchTurnReconfigureAttached`
- From: `Attached`
- On: `CompleteUntilChangedSwitchTurnReconfigure`(request_id)
- Guards:
  - `pending_until_changed_reconfigure`
- Emits: `ModelRoutingStatusChanged`
- To: `Attached`

### `CompleteUntilChangedSwitchTurnReconfigureRunning`
- From: `Running`
- On: `CompleteUntilChangedSwitchTurnReconfigure`(request_id)
- Guards:
  - `pending_until_changed_reconfigure`
- Emits: `ModelRoutingStatusChanged`
- To: `Running`

### `AdmitPendingFiniteSwitchTurnIdle`
- From: `Idle`
- On: `AdmitModelRoutingAssistantTurn`()
- Guards:
  - `pending_finite`
- Emits: `SwitchTurnFiniteOverrideActivated`, `ModelRoutingStatusChanged`
- To: `Idle`

### `AdmitPendingFiniteSwitchTurnAttached`
- From: `Attached`
- On: `AdmitModelRoutingAssistantTurn`()
- Guards:
  - `pending_finite`
- Emits: `SwitchTurnFiniteOverrideActivated`, `ModelRoutingStatusChanged`
- To: `Attached`

### `AdmitPendingFiniteSwitchTurnRunning`
- From: `Running`
- On: `AdmitModelRoutingAssistantTurn`()
- Guards:
  - `pending_finite`
- Emits: `SwitchTurnFiniteOverrideActivated`, `ModelRoutingStatusChanged`
- To: `Running`

### `DecrementFiniteSwitchTurnIdle`
- From: `Idle`
- On: `AdmitModelRoutingAssistantTurn`()
- Guards:
  - `active_multi_turn`
- To: `Idle`

### `DecrementFiniteSwitchTurnAttached`
- From: `Attached`
- On: `AdmitModelRoutingAssistantTurn`()
- Guards:
  - `active_multi_turn`
- To: `Attached`

### `DecrementFiniteSwitchTurnRunning`
- From: `Running`
- On: `AdmitModelRoutingAssistantTurn`()
- Guards:
  - `active_multi_turn`
- To: `Running`

### `RestoreConsumedFiniteSwitchTurnIdle`
- From: `Idle`
- On: `AdmitModelRoutingAssistantTurn`()
- Guards:
  - `active_one_turn_remaining`
- Emits: `SwitchTurnFiniteOverrideRestored`, `ModelRoutingStatusChanged`
- To: `Idle`

### `RestoreConsumedFiniteSwitchTurnAttached`
- From: `Attached`
- On: `AdmitModelRoutingAssistantTurn`()
- Guards:
  - `active_one_turn_remaining`
- Emits: `SwitchTurnFiniteOverrideRestored`, `ModelRoutingStatusChanged`
- To: `Attached`

### `RestoreConsumedFiniteSwitchTurnRunning`
- From: `Running`
- On: `AdmitModelRoutingAssistantTurn`()
- Guards:
  - `active_one_turn_remaining`
- Emits: `SwitchTurnFiniteOverrideRestored`, `ModelRoutingStatusChanged`
- To: `Running`

### `BeginImageOperationScopedConflictIdle`
- From: `Idle`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `operation_in_operation_conflict`
- Emits: `ImageOperationDenied`
- To: `Idle`

### `BeginImageOperationScopedConflictAttached`
- From: `Attached`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `operation_in_operation_conflict`
- Emits: `ImageOperationDenied`
- To: `Attached`

### `BeginImageOperationScopedConflictRunning`
- From: `Running`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `operation_in_operation_conflict`
- Emits: `ImageOperationDenied`
- To: `Running`

### `BeginImageOperationRealtimeConflictIdle`
- From: `Idle`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `realtime_conflict`
- Emits: `ImageOperationDenied`
- To: `Idle`

### `BeginImageOperationRealtimeConflictAttached`
- From: `Attached`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `realtime_conflict`
- Emits: `ImageOperationDenied`
- To: `Attached`

### `BeginImageOperationRealtimeConflictRunning`
- From: `Running`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `realtime_conflict`
- Emits: `ImageOperationDenied`
- To: `Running`

### `BeginImageOperationApprovalUnavailableIdle`
- From: `Idle`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `approval_unavailable`
- Emits: `ImageOperationDenied`
- To: `Idle`

### `BeginImageOperationApprovalUnavailableAttached`
- From: `Attached`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `approval_unavailable`
- Emits: `ImageOperationDenied`
- To: `Attached`

### `BeginImageOperationApprovalUnavailableRunning`
- From: `Running`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `approval_unavailable`
- Emits: `ImageOperationDenied`
- To: `Running`

### `BeginImageOperationApprovalDeniedIdle`
- From: `Idle`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `approval_denied`
- Emits: `ImageOperationDenied`, `ModelRoutingApprovalTerminalized`
- To: `Idle`

### `BeginImageOperationApprovalDeniedAttached`
- From: `Attached`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `approval_denied`
- Emits: `ImageOperationDenied`, `ModelRoutingApprovalTerminalized`
- To: `Attached`

### `BeginImageOperationApprovalDeniedRunning`
- From: `Running`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `approval_denied`
- Emits: `ImageOperationDenied`, `ModelRoutingApprovalTerminalized`
- To: `Running`

### `BeginImageOperationAcceptedIdle`
- From: `Idle`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `baseline_known`
  - `no_operation_in_operation`
  - `approval_satisfied`
  - `no_realtime_conflict`
- Emits: `ImageOperationPhaseChanged`
- To: `Idle`

### `BeginImageOperationAcceptedAttached`
- From: `Attached`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `baseline_known`
  - `no_operation_in_operation`
  - `approval_satisfied`
  - `no_realtime_conflict`
- Emits: `ImageOperationPhaseChanged`
- To: `Attached`

### `BeginImageOperationAcceptedRunning`
- From: `Running`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `baseline_known`
  - `no_operation_in_operation`
  - `approval_satisfied`
  - `no_realtime_conflict`
- Emits: `ImageOperationPhaseChanged`
- To: `Running`

### `ActivateImageOperationOverrideIdle`
- From: `Idle`
- On: `ActivateImageOperationOverride`(operation_id, target_model, target_realtime_capable)
- Guards:
  - `operation_plan_resolved`
  - `operation_requires_scoped_override`
  - `no_operation_override_active`
- Emits: `ImageOperationPhaseChanged`, `ModelRoutingStatusChanged`
- To: `Idle`

### `ActivateImageOperationOverrideAttached`
- From: `Attached`
- On: `ActivateImageOperationOverride`(operation_id, target_model, target_realtime_capable)
- Guards:
  - `operation_plan_resolved`
  - `operation_requires_scoped_override`
  - `no_operation_override_active`
- Emits: `ImageOperationPhaseChanged`, `ModelRoutingStatusChanged`
- To: `Attached`

### `ActivateImageOperationOverrideRunning`
- From: `Running`
- On: `ActivateImageOperationOverride`(operation_id, target_model, target_realtime_capable)
- Guards:
  - `operation_plan_resolved`
  - `operation_requires_scoped_override`
  - `no_operation_override_active`
- Emits: `ImageOperationPhaseChanged`, `ModelRoutingStatusChanged`
- To: `Running`

### `CompleteImageOperationIdle`
- From: `Idle`
- On: `CompleteImageOperation`(operation_id, terminal, terminal_payload)
- Guards:
  - `operation_active`
  - `operation_requires_scoped_override`
- Emits: `ImageOperationPhaseChanged`
- To: `Idle`

### `CompleteImageOperationAttached`
- From: `Attached`
- On: `CompleteImageOperation`(operation_id, terminal, terminal_payload)
- Guards:
  - `operation_active`
  - `operation_requires_scoped_override`
- Emits: `ImageOperationPhaseChanged`
- To: `Attached`

### `CompleteImageOperationRunning`
- From: `Running`
- On: `CompleteImageOperation`(operation_id, terminal, terminal_payload)
- Guards:
  - `operation_active`
  - `operation_requires_scoped_override`
- Emits: `ImageOperationPhaseChanged`
- To: `Running`

### `CompleteImageOperationWithoutScopedOverrideIdle`
- From: `Idle`
- On: `CompleteImageOperation`(operation_id, terminal, terminal_payload)
- Guards:
  - `operation_plan_resolved`
  - `operation_does_not_require_scoped_override`
  - `no_operation_override_active`
- Emits: `ImageOperationPhaseChanged`
- To: `Idle`

### `CompleteImageOperationWithoutScopedOverrideAttached`
- From: `Attached`
- On: `CompleteImageOperation`(operation_id, terminal, terminal_payload)
- Guards:
  - `operation_plan_resolved`
  - `operation_does_not_require_scoped_override`
  - `no_operation_override_active`
- Emits: `ImageOperationPhaseChanged`
- To: `Attached`

### `CompleteImageOperationWithoutScopedOverrideRunning`
- From: `Running`
- On: `CompleteImageOperation`(operation_id, terminal, terminal_payload)
- Guards:
  - `operation_plan_resolved`
  - `operation_does_not_require_scoped_override`
  - `no_operation_override_active`
- Emits: `ImageOperationPhaseChanged`
- To: `Running`

### `RestoreImageOperationOverrideIdle`
- From: `Idle`
- On: `RestoreImageOperationOverride`(operation_id)
- Guards:
  - `operation_active`
- Emits: `ImageOperationPhaseChanged`, `ModelRoutingStatusChanged`
- To: `Idle`

### `RestoreImageOperationOverrideAttached`
- From: `Attached`
- On: `RestoreImageOperationOverride`(operation_id)
- Guards:
  - `operation_active`
- Emits: `ImageOperationPhaseChanged`, `ModelRoutingStatusChanged`
- To: `Attached`

### `RestoreImageOperationOverrideRunning`
- From: `Running`
- On: `RestoreImageOperationOverride`(operation_id)
- Guards:
  - `operation_active`
- Emits: `ImageOperationPhaseChanged`, `ModelRoutingStatusChanged`
- To: `Running`

### `StagePersistentFilterIdle`
- From: `Idle`
- On: `StagePersistentFilter`(filter, witnesses)
- Guards:
  - `session_registered`
- To: `Idle`

### `StagePersistentFilterAttached`
- From: `Attached`
- On: `StagePersistentFilter`(filter, witnesses)
- Guards:
  - `session_registered`
- To: `Attached`

### `StagePersistentFilterRunning`
- From: `Running`
- On: `StagePersistentFilter`(filter, witnesses)
- Guards:
  - `session_registered`
- To: `Running`

### `StagePersistentFilterRetired`
- From: `Retired`
- On: `StagePersistentFilter`(filter, witnesses)
- Guards:
  - `session_registered`
- To: `Retired`

### `StagePersistentFilterStopped`
- From: `Stopped`
- On: `StagePersistentFilter`(filter, witnesses)
- Guards:
  - `session_registered`
- To: `Stopped`

### `RequestDeferredToolsIdle`
- From: `Idle`
- On: `RequestDeferredTools`(authorities)
- Guards:
  - `session_registered`
  - `deferred_authorities_non_empty`
  - `deferred_authorities_have_identity`
- To: `Idle`

### `RequestDeferredToolsAttached`
- From: `Attached`
- On: `RequestDeferredTools`(authorities)
- Guards:
  - `session_registered`
  - `deferred_authorities_non_empty`
  - `deferred_authorities_have_identity`
- To: `Attached`

### `RequestDeferredToolsRunning`
- From: `Running`
- On: `RequestDeferredTools`(authorities)
- Guards:
  - `session_registered`
  - `deferred_authorities_non_empty`
  - `deferred_authorities_have_identity`
- To: `Running`

### `RequestDeferredToolsRetired`
- From: `Retired`
- On: `RequestDeferredTools`(authorities)
- Guards:
  - `session_registered`
  - `deferred_authorities_non_empty`
  - `deferred_authorities_have_identity`
- To: `Retired`

### `RequestDeferredToolsStopped`
- From: `Stopped`
- On: `RequestDeferredTools`(authorities)
- Guards:
  - `session_registered`
  - `deferred_authorities_non_empty`
  - `deferred_authorities_have_identity`
- To: `Stopped`

### `PrepareBindingsInitializing`
- From: `Initializing`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, session_id)
- Emits: `RuntimeBound`
- To: `Initializing`

### `PrepareBindingsIdle`
- From: `Idle`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, session_id)
- Emits: `RuntimeBound`
- To: `Attached`

### `PrepareBindingsAttached`
- From: `Attached`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, session_id)
- Emits: `RuntimeBound`
- To: `Attached`

### `PrepareBindingsRunning`
- From: `Running`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, session_id)
- Emits: `RuntimeBound`
- To: `Running`

### `PrepareBindingsRetired`
- From: `Retired`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, session_id)
- Emits: `RuntimeBound`
- To: `Retired`

### `PrepareBindingsStopped`
- From: `Stopped`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, session_id)
- Emits: `RuntimeBound`
- To: `Stopped`

### `SetPeerIngressContextIdle`
- From: `Idle`
- On: `SetPeerIngressContext`(keep_alive)
- Guards:
  - `session_registered`
- To: `Idle`

### `SetPeerIngressContextAttached`
- From: `Attached`
- On: `SetPeerIngressContext`(keep_alive)
- Guards:
  - `session_registered`
- To: `Attached`

### `SetPeerIngressContextRunning`
- From: `Running`
- On: `SetPeerIngressContext`(keep_alive)
- Guards:
  - `session_registered`
- To: `Running`

### `SetPeerIngressContextRetired`
- From: `Retired`
- On: `SetPeerIngressContext`(keep_alive)
- Guards:
  - `session_registered`
- To: `Retired`

### `SetPeerIngressContextStopped`
- From: `Stopped`
- On: `SetPeerIngressContext`(keep_alive)
- Guards:
  - `session_registered`
- To: `Stopped`

### `NotifyDrainExitedIdle`
- From: `Idle`
- On: `NotifyDrainExited`(reason)
- Guards:
  - `session_registered`
- Emits: `RuntimeNotice`
- To: `Idle`

### `NotifyDrainExitedAttached`
- From: `Attached`
- On: `NotifyDrainExited`(reason)
- Guards:
  - `session_registered`
- Emits: `RuntimeNotice`
- To: `Attached`

### `NotifyDrainExitedRunning`
- From: `Running`
- On: `NotifyDrainExited`(reason)
- Guards:
  - `session_registered`
- Emits: `RuntimeNotice`
- To: `Running`

### `NotifyDrainExitedRetired`
- From: `Retired`
- On: `NotifyDrainExited`(reason)
- Guards:
  - `session_registered`
- Emits: `RuntimeNotice`
- To: `Retired`

### `NotifyDrainExitedStopped`
- From: `Stopped`
- On: `NotifyDrainExited`(reason)
- Guards:
  - `session_registered`
- Emits: `RuntimeNotice`
- To: `Stopped`

### `InterruptCurrentRunAttached`
- From: `Attached`
- On: `InterruptCurrentRun`()
- Emits: `WakeInterrupt`, `RequestCancellationAtBoundary`
- To: `Attached`

### `InterruptCurrentRun`
- From: `Running`
- On: `InterruptCurrentRun`()
- Emits: `WakeInterrupt`, `RequestCancellationAtBoundary`
- To: `Running`

### `CancelAfterBoundaryAttached`
- From: `Attached`
- On: `CancelAfterBoundary`(reason)
- Emits: `RequestCancellationAtBoundary`, `RuntimeEffectFact`
- To: `Attached`

### `CancelAfterBoundary`
- From: `Running`
- On: `CancelAfterBoundary`(reason)
- Emits: `RequestCancellationAtBoundary`, `RuntimeEffectFact`
- To: `Running`

### `BoundaryAppliedPublish`
- From: `Running`
- On: `BoundaryApplied`(revision)
- Emits: `CommittedVisibleSetPublished`
- To: `Running`

### `PublishCommittedVisibleSetIdle`
- From: `Idle`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_deferred_authorities, staged_deferred_authorities, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_requested_subset_of_staged_requested`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- Emits: `CommittedVisibleSetPublished`
- To: `Idle`

### `PublishCommittedVisibleSetAttached`
- From: `Attached`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_deferred_authorities, staged_deferred_authorities, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_requested_subset_of_staged_requested`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- Emits: `CommittedVisibleSetPublished`
- To: `Attached`

### `PublishCommittedVisibleSetRunning`
- From: `Running`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_deferred_authorities, staged_deferred_authorities, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_requested_subset_of_staged_requested`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- Emits: `CommittedVisibleSetPublished`
- To: `Running`

### `PublishCommittedVisibleSetRetired`
- From: `Retired`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_deferred_authorities, staged_deferred_authorities, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_requested_subset_of_staged_requested`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- Emits: `CommittedVisibleSetPublished`
- To: `Retired`

### `PublishCommittedVisibleSetStopped`
- From: `Stopped`
- On: `PublishCommittedVisibleSet`(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_deferred_authorities, staged_deferred_authorities, active_visibility_revision, staged_visibility_revision)
- Guards:
  - `session_registered`
  - `active_not_behind_staged`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_requested_subset_of_staged_requested`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- Emits: `CommittedVisibleSetPublished`
- To: `Stopped`

### `RetireRequestedFromIdle`
- From: `Idle`, `Attached`, `Running`
- On: `Retire`(session_id)
- Emits: `RuntimeRetired`
- To: `Retired`

### `RetireAlreadyRetired`
- From: `Retired`
- On: `Retire`(session_id)
- To: `Retired`

### `Reset`
- From: `Initializing`, `Idle`, `Attached`, `Retired`
- On: `Reset`()
- Emits: `RuntimeNotice`
- To: `Idle`

### `StopRuntimeExecutorUnbound`
- From: `Initializing`, `Idle`, `Retired`
- On: `StopRuntimeExecutor`(reason)
- Emits: `RuntimeNotice`, `RuntimeEffectFact`
- To: `Stopped`

### `StopRuntimeExecutorAttached`
- From: `Attached`
- On: `StopRuntimeExecutor`(reason)
- Emits: `RuntimeNotice`, `RuntimeEffectFact`
- To: `Attached`

### `StopRuntimeExecutorRunning`
- From: `Running`
- On: `StopRuntimeExecutor`(reason)
- Emits: `RuntimeNotice`, `RuntimeEffectFact`
- To: `Running`

### `RuntimeExecutorExitedFromAttached`
- From: `Attached`
- On: `RuntimeExecutorExited`()
- Emits: `RuntimeNotice`
- To: `Stopped`

### `RuntimeExecutorExitedFromRunning`
- From: `Running`
- On: `RuntimeExecutorExited`()
- Emits: `RuntimeNotice`
- To: `Stopped`

### `RuntimeExecutorExitedFromIdle`
- From: `Idle`
- On: `RuntimeExecutorExited`()
- Emits: `RuntimeNotice`
- To: `Stopped`

### `RuntimeExecutorExitedFromRetired`
- From: `Retired`
- On: `RuntimeExecutorExited`()
- Emits: `RuntimeNotice`
- To: `Stopped`

### `RuntimeExecutorExitedFromStopped`
- From: `Stopped`
- On: `RuntimeExecutorExited`()
- To: `Stopped`

### `DestroyInitializing`
- From: `Initializing`
- On: `Destroy`(session_id)
- Guards:
  - `runtime_is_bound`
- Emits: `RuntimeDestroyed`
- To: `Destroyed`

### `Destroy`
- From: `Idle`, `Attached`, `Running`, `Retired`, `Stopped`
- On: `Destroy`(session_id)
- Emits: `RuntimeDestroyed`
- To: `Destroyed`

### `RecoverInitializing`
- From: `Initializing`
- On: `Recover`()
- Emits: `RuntimeNotice`
- To: `Initializing`

### `RecoverIdle`
- From: `Idle`
- On: `Recover`()
- Emits: `RuntimeNotice`
- To: `Idle`

### `RecoverAttached`
- From: `Attached`
- On: `Recover`()
- Emits: `RuntimeNotice`
- To: `Attached`

### `RecoverRetired`
- From: `Retired`
- On: `Recover`()
- Emits: `RuntimeNotice`
- To: `Retired`

### `RecoverStopped`
- From: `Stopped`
- On: `Recover`()
- Emits: `RuntimeNotice`
- To: `Stopped`

### `EnsureSessionWithExecutorIdle`
- From: `Idle`
- On: `EnsureSessionWithExecutor`(session_id)
- To: `Attached`

### `EnsureSessionWithExecutorAttached`
- From: `Attached`
- On: `EnsureSessionWithExecutor`(session_id)
- To: `Attached`

### `EnsureSessionWithExecutorRunning`
- From: `Running`
- On: `EnsureSessionWithExecutor`(session_id)
- To: `Running`

### `EnsureSessionWithExecutorRetired`
- From: `Retired`
- On: `EnsureSessionWithExecutor`(session_id)
- To: `Retired`

### `EnsureSessionWithExecutorStopped`
- From: `Stopped`
- On: `EnsureSessionWithExecutor`(session_id)
- To: `Stopped`

### `SetSilentIntentsIdle`
- From: `Idle`
- On: `SetSilentIntents`(session_id, intents)
- Guards:
  - `session_registered`
- To: `Idle`

### `SetSilentIntentsAttached`
- From: `Attached`
- On: `SetSilentIntents`(session_id, intents)
- Guards:
  - `session_registered`
- To: `Attached`

### `SetSilentIntentsRunning`
- From: `Running`
- On: `SetSilentIntents`(session_id, intents)
- Guards:
  - `session_registered`
- To: `Running`

### `SetSilentIntentsRetired`
- From: `Retired`
- On: `SetSilentIntents`(session_id, intents)
- Guards:
  - `session_registered`
- To: `Retired`

### `SetSilentIntentsStopped`
- From: `Stopped`
- On: `SetSilentIntents`(session_id, intents)
- Guards:
  - `session_registered`
- To: `Stopped`

### `AbortIdle`
- From: `Idle`
- On: `Abort`(session_id)
- Guards:
  - `session_registered`
- To: `Idle`

### `AbortAttached`
- From: `Attached`
- On: `Abort`(session_id)
- Guards:
  - `session_registered`
- To: `Attached`

### `AbortRunning`
- From: `Running`
- On: `Abort`(session_id)
- Guards:
  - `session_registered`
- To: `Running`

### `AbortRetired`
- From: `Retired`
- On: `Abort`(session_id)
- Guards:
  - `session_registered`
- To: `Retired`

### `AbortStopped`
- From: `Stopped`
- On: `Abort`(session_id)
- Guards:
  - `session_registered`
- To: `Stopped`

### `WaitIdle`
- From: `Idle`
- On: `Wait`(session_id)
- Guards:
  - `session_registered`
- To: `Idle`

### `WaitAttached`
- From: `Attached`
- On: `Wait`(session_id)
- Guards:
  - `session_registered`
- To: `Attached`

### `WaitRunning`
- From: `Running`
- On: `Wait`(session_id)
- Guards:
  - `session_registered`
- To: `Running`

### `WaitRetired`
- From: `Retired`
- On: `Wait`(session_id)
- Guards:
  - `session_registered`
- To: `Retired`

### `WaitStopped`
- From: `Stopped`
- On: `Wait`(session_id)
- Guards:
  - `session_registered`
- To: `Stopped`

### `AbortAllIdle`
- From: `Idle`
- On: `AbortAll`()
- To: `Idle`

### `AbortAllAttached`
- From: `Attached`
- On: `AbortAll`()
- To: `Attached`

### `AbortAllRunning`
- From: `Running`
- On: `AbortAll`()
- To: `Running`

### `AbortAllRetired`
- From: `Retired`
- On: `AbortAll`()
- To: `Retired`

### `AbortAllStopped`
- From: `Stopped`
- On: `AbortAll`()
- To: `Stopped`

### `EnsureDrainRunningAttached`
- From: `Attached`
- On: `EnsureDrainRunning`()
- Guards:
  - `session_registered`
- Emits: `SpawnDrainTask`
- To: `Attached`

### `EnsureDrainRunningRunning`
- From: `Running`
- On: `EnsureDrainRunning`()
- Guards:
  - `session_registered`
- Emits: `SpawnDrainTask`
- To: `Running`

### `IngestIdle`
- From: `Idle`
- On: `Ingest`(runtime_id, work_id, origin)
- Guards:
  - `session_registered`
- Emits: `ResolveAdmission`
- To: `Idle`

### `IngestAttached`
- From: `Attached`
- On: `Ingest`(runtime_id, work_id, origin)
- Guards:
  - `session_registered`
- Emits: `ResolveAdmission`
- To: `Attached`

### `IngestRunning`
- From: `Running`
- On: `Ingest`(runtime_id, work_id, origin)
- Guards:
  - `session_registered`
- Emits: `ResolveAdmission`
- To: `Running`

### `PublishEventIdle`
- From: `Idle`
- On: `PublishEvent`(kind)
- Guards:
  - `session_registered`
- Emits: `IngressNotice`
- To: `Idle`

### `PublishEventAttached`
- From: `Attached`
- On: `PublishEvent`(kind)
- Guards:
  - `session_registered`
- Emits: `IngressNotice`
- To: `Attached`

### `PublishEventRunning`
- From: `Running`
- On: `PublishEvent`(kind)
- Guards:
  - `session_registered`
- Emits: `IngressNotice`
- To: `Running`

### `PublishEventRetired`
- From: `Retired`
- On: `PublishEvent`(kind)
- Guards:
  - `session_registered`
- Emits: `IngressNotice`
- To: `Retired`

### `PublishEventStopped`
- From: `Stopped`
- On: `PublishEvent`(kind)
- Guards:
  - `session_registered`
- Emits: `IngressNotice`
- To: `Stopped`

### `AcceptWithCompletionIdleQueued`
- From: `Idle`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`
- To: `Idle`

### `AcceptWithCompletionIdleImmediate`
- From: `Idle`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`
- To: `Idle`

### `AcceptWithCompletionAttachedImmediate`
- From: `Attached`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`, `SubmitRunPrimitive`
- To: `Attached`

### `AcceptWithCompletionAttachedQueued`
- From: `Attached`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`
- To: `Attached`

### `AcceptWithCompletionRunningQueuedPassive`
- From: `Running`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
  - `wake_if_idle`
- Emits: `IngressAccepted`
- To: `Running`

### `AcceptWithCompletionRunningQueuedWakeIfIdle`
- From: `Running`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
  - `wake_if_idle`
- Emits: `IngressAccepted`, `PostAdmissionSignal`
- To: `Running`

### `AcceptWithCompletionRunningInterruptYielding`
- From: `Running`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`, `RuntimeEffectFact`
- To: `Running`

### `AcceptWithCompletionRunningImmediate`
- From: `Running`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`, `RuntimeEffectFact`
- To: `Running`

### `AcceptWithoutWakeIdle`
- From: `Idle`
- On: `AcceptWithoutWake`(input_id)
- Guards:
  - `session_registered`
- Emits: `IngressAccepted`
- To: `Idle`

### `AcceptWithoutWakeAttached`
- From: `Attached`
- On: `AcceptWithoutWake`(input_id)
- Guards:
  - `session_registered`
- Emits: `IngressAccepted`
- To: `Attached`

### `AcceptWithoutWakeRunning`
- From: `Running`
- On: `AcceptWithoutWake`(input_id)
- Guards:
  - `session_registered`
- Emits: `IngressAccepted`
- To: `Running`

### `ClassifyExternalEnvelopeMessageAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_message`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeMessageRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_message`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestPeerAddedAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_added`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestPeerAddedRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_added`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestPeerRetiredAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestPeerRetiredRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestPeerUnwiredAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestPeerUnwiredRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestSupervisorSilentAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_supervisor_silent_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestSupervisorSilentRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_supervisor_silent_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestSilentAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_silent_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestSilentRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_silent_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestSupervisorAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_supervisor_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestSupervisorRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_supervisor_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestActionableAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_actionable_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestActionableRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_actionable_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeLifecycleAddedAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_added`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeLifecycleAddedRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_added`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeLifecycleRetiredAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeLifecycleRetiredRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeLifecycleUnwiredAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeLifecycleUnwiredRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeResponseAcceptedAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_response_accepted`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeResponseAcceptedRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_response_accepted`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeResponseCompletedAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_response_completed`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeResponseCompletedRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_response_completed`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeResponseFailedAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_response_failed`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeResponseFailedRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_response_failed`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeAckAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_ack`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeAckRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_ack`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyPlainEventAttached`
- From: `Attached`
- On: `ClassifyPlainEvent`(source_name)
- Guards:
  - `session_registered`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyPlainEventRunning`
- From: `Running`
- On: `ClassifyPlainEvent`(source_name)
- Guards:
  - `session_registered`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `PrepareIdle`
- From: `Idle`
- On: `Prepare`(session_id, run_id)
- Guards:
  - `session_registered`
- Emits: `SubmitRunPrimitive`
- To: `Running`

### `PrepareAttached`
- From: `Attached`
- On: `Prepare`(session_id, run_id)
- Guards:
  - `session_registered`
- Emits: `SubmitRunPrimitive`
- To: `Running`

### `DrainQueuedRunRetired`
- From: `Retired`
- On: `DrainQueuedRun`(run_id)
- Emits: `SubmitRunPrimitive`
- To: `Running`

### `StartConversationRunInitializing`
- From: `Initializing`
- On: `StartConversationRun`(run_id, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, max_extraction_retries)
- Guards:
  - `turn_resettable`
  - `conversation_shape_matches_primitive`
- Emits: `TurnRunStarted`
- To: `Running`

### `StartConversationRunAttached`
- From: `Attached`
- On: `StartConversationRun`(run_id, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, max_extraction_retries)
- Guards:
  - `turn_resettable`
  - `conversation_shape_matches_primitive`
- Emits: `TurnRunStarted`
- To: `Running`

### `StartConversationRunRunning`
- From: `Running`
- On: `StartConversationRun`(run_id, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, max_extraction_retries)
- Guards:
  - `turn_resettable`
  - `conversation_shape_matches_primitive`
- Emits: `TurnRunStarted`
- To: `Running`

### `StartImmediateAppendInitializing`
- From: `Initializing`
- On: `StartImmediateAppend`(run_id)
- Guards:
  - `turn_resettable`
- Emits: `TurnRunStarted`
- To: `Running`

### `StartImmediateAppendAttached`
- From: `Attached`
- On: `StartImmediateAppend`(run_id)
- Guards:
  - `turn_resettable`
- Emits: `TurnRunStarted`
- To: `Running`

### `StartImmediateAppendRunning`
- From: `Running`
- On: `StartImmediateAppend`(run_id)
- Guards:
  - `turn_resettable`
- Emits: `TurnRunStarted`
- To: `Running`

### `StartImmediateContextInitializing`
- From: `Initializing`
- On: `StartImmediateContext`(run_id)
- Guards:
  - `turn_resettable`
- Emits: `TurnRunStarted`
- To: `Running`

### `StartImmediateContextAttached`
- From: `Attached`
- On: `StartImmediateContext`(run_id)
- Guards:
  - `turn_resettable`
- Emits: `TurnRunStarted`
- To: `Running`

### `StartImmediateContextRunning`
- From: `Running`
- On: `StartImmediateContext`(run_id)
- Guards:
  - `turn_resettable`
- Emits: `TurnRunStarted`
- To: `Running`

### `PrimitiveAppliedConversation`
- From: `Running`
- On: `PrimitiveApplied`()
- Guards:
  - `turn_applying_conversation`
- Emits: `TurnCheckCompaction`
- To: `Running`

### `PrimitiveAppliedImmediate`
- From: `Running`
- On: `PrimitiveApplied`()
- Guards:
  - `turn_applying_immediate`
- Emits: `TurnBoundaryApplied`, `TurnRunCompleted`, `TurnCheckCompaction`
- To: `Running`

### `LlmReturnedToolCallsPositive`
- From: `Running`
- On: `LlmReturnedToolCalls`(tool_count)
- Guards:
  - `turn_calling_llm`
  - `tool_count_positive`
- To: `Running`

### `LlmReturnedToolCallsZero`
- From: `Running`
- On: `LlmReturnedToolCalls`(tool_count)
- Guards:
  - `turn_calling_llm`
  - `tool_count_zero`
- To: `Running`

### `LlmReturnedTerminal`
- From: `Running`
- On: `LlmReturnedTerminal`()
- Guards:
  - `turn_calling_llm`
- To: `Running`

### `RegisterPendingOps`
- From: `Running`
- On: `RegisterPendingOps`(op_refs, barrier_operation_ids)
- Guards:
  - `turn_waiting_or_calling`
- To: `Running`

### `ToolCallsResolvedToCalling`
- From: `Running`
- On: `ToolCallsResolved`()
- Guards:
  - `turn_waiting_for_ops`
  - `barrier_not_satisfied`
- To: `Running`

### `ToolCallsResolvedToBoundary`
- From: `Running`
- On: `ToolCallsResolved`()
- Guards:
  - `turn_waiting_for_ops`
  - `barrier_satisfied`
- To: `Running`

### `OpsBarrierSatisfied`
- From: `Running`
- On: `OpsBarrierSatisfied`(operation_ids)
- Guards:
  - `turn_waiting_for_ops`
  - `matching_barrier_ids`
- To: `Running`

### `BoundaryContinue`
- From: `Running`
- On: `BoundaryContinue`()
- Guards:
  - `turn_draining_boundary`
- Emits: `TurnBoundaryApplied`, `TurnCheckCompaction`
- To: `Running`

### `BoundaryComplete`
- From: `Running`
- On: `BoundaryComplete`()
- Guards:
  - `turn_draining_boundary`
- Emits: `TurnBoundaryApplied`, `TurnRunCompleted`, `TurnCheckCompaction`
- To: `Running`

### `EnterExtraction`
- From: `Running`
- On: `EnterExtraction`(max_extraction_retries)
- Guards:
  - `turn_draining_boundary`
- To: `Running`

### `ExtractionStart`
- From: `Running`
- On: `ExtractionStart`()
- Guards:
  - `turn_extracting`
- To: `Running`

### `ExtractionValidationPassed`
- From: `Running`
- On: `ExtractionValidationPassed`()
- Guards:
  - `turn_extracting`
- Emits: `TurnRunCompleted`, `TurnCheckCompaction`
- To: `Running`

### `ExtractionValidationFailedRetry`
- From: `Running`
- On: `ExtractionValidationFailed`(error)
- Guards:
  - `turn_extracting`
  - `retries_remaining`
- Emits: `TurnCheckCompaction`
- To: `Running`

### `ExtractionValidationFailedExhausted`
- From: `Running`
- On: `ExtractionValidationFailed`(error)
- Guards:
  - `turn_extracting`
  - `retries_exhausted`
- Emits: `TurnRunFailed`
- To: `Running`

### `RecoverableFailure`
- From: `Running`
- On: `RecoverableFailure`(failure_kind, retry_attempt, max_retries, selected_delay_ms, error)
- Guards:
  - `turn_non_terminal`
  - `retry_attempt_present`
- To: `Running`

### `FatalFailure`
- From: `Running`
- On: `FatalFailure`(terminal_cause_kind, error)
- Guards:
  - `turn_not_terminal`
  - `terminal_cause_known`
- Emits: `TurnRunFailed`
- To: `Running`

### `RetryRequested`
- From: `Running`
- On: `RetryRequested`(retry_attempt)
- Guards:
  - `turn_error_recovery`
  - `retry_attempt_matches`
- Emits: `TurnCheckCompaction`
- To: `Running`

### `CancelNow`
- From: `Running`
- On: `CancelNow`()
- Guards:
  - `turn_cancellable`
- To: `Running`

### `RequestCancelAfterBoundary`
- From: `Running`
- On: `RequestCancelAfterBoundary`()
- Guards:
  - `turn_cancellable`
- To: `Running`

### `CancellationObserved`
- From: `Running`
- On: `CancellationObserved`()
- Guards:
  - `turn_cancelling`
- Emits: `TurnRunCancelled`
- To: `Running`

### `AcknowledgeTerminal`
- From: `Running`
- On: `AcknowledgeTerminal`(outcome)
- Guards:
  - `turn_terminal`
- To: `Running`

### `TurnLimitReached`
- From: `Running`
- On: `TurnLimitReached`()
- Guards:
  - `turn_not_terminal`
- Emits: `TurnRunFailed`
- To: `Running`

### `BudgetExhausted`
- From: `Running`
- On: `BudgetExhausted`()
- Guards:
  - `turn_not_terminal`
- Emits: `TurnRunFailed`
- To: `Running`

### `TimeBudgetExceeded`
- From: `Running`
- On: `TimeBudgetExceeded`()
- Guards:
  - `turn_not_terminal`
- Emits: `TurnRunFailed`
- To: `Running`

### `ForceCancelNoRun`
- From: `Running`
- On: `ForceCancelNoRun`()
- Guards:
  - `no_run_bound`
  - `turn_ready`
- To: `Running`

### `RunCompleted`
- From: `Running`
- On: `RunCompleted`(run_id)
- Guards:
  - `run_matches_binding`
- To: `Running`

### `RunFailed`
- From: `Running`
- On: `RunFailed`(run_id, runtime_apply_failure_cause, runtime_apply_failure_message, terminal_cause_kind, error)
- Guards:
  - `run_matches_binding`
  - `terminal_cause_known`
- To: `Running`

### `RunCancelled`
- From: `Running`
- On: `RunCancelled`(run_id)
- Guards:
  - `run_matches_binding`
- To: `Running`

### `SurfaceRegisterAttached`
- From: `Attached`
- On: `SurfaceRegister`(surface_id)
- To: `Attached`

### `SurfaceRegisterRunning`
- From: `Running`
- On: `SurfaceRegister`(surface_id)
- To: `Running`

### `SurfaceStageAddAttached`
- From: `Attached`
- On: `SurfaceStageAdd`(surface_id, now_ms)
- Guards:
  - `surface_operating`
- To: `Attached`

### `SurfaceStageAddRunning`
- From: `Running`
- On: `SurfaceStageAdd`(surface_id, now_ms)
- Guards:
  - `surface_operating`
- To: `Running`

### `SurfaceStageRemoveAttached`
- From: `Attached`
- On: `SurfaceStageRemove`(surface_id, now_ms)
- Guards:
  - `surface_operating`
- To: `Attached`

### `SurfaceStageRemoveRunning`
- From: `Running`
- On: `SurfaceStageRemove`(surface_id, now_ms)
- Guards:
  - `surface_operating`
- To: `Running`

### `SurfaceStageReloadAttached`
- From: `Attached`
- On: `SurfaceStageReload`(surface_id, now_ms)
- Guards:
  - `surface_operating`
  - `surface_active`
- To: `Attached`

### `SurfaceStageReloadRunning`
- From: `Running`
- On: `SurfaceStageReload`(surface_id, now_ms)
- Guards:
  - `surface_operating`
  - `surface_active`
- To: `Running`

### `SurfaceApplyBoundaryAddAttached`
- From: `Attached`
- On: `SurfaceApplyBoundary`(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
- Guards:
  - `surface_operating`
  - `staged_add`
  - `staged_sequence_matches`
  - `no_pending_surface_op`
  - `base_accepts_add`
- Emits: `ScheduleSurfaceCompletion`, `EmitExternalToolDelta`
- To: `Attached`

### `SurfaceApplyBoundaryAddRunning`
- From: `Running`
- On: `SurfaceApplyBoundary`(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
- Guards:
  - `surface_operating`
  - `staged_add`
  - `staged_sequence_matches`
  - `no_pending_surface_op`
  - `base_accepts_add`
- Emits: `ScheduleSurfaceCompletion`, `EmitExternalToolDelta`
- To: `Running`

### `SurfaceApplyBoundaryReloadAttached`
- From: `Attached`
- On: `SurfaceApplyBoundary`(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
- Guards:
  - `surface_operating`
  - `staged_reload`
  - `staged_sequence_matches`
  - `surface_active`
  - `base_active`
  - `no_pending_surface_op`
- Emits: `ScheduleSurfaceCompletion`, `EmitExternalToolDelta`
- To: `Attached`

### `SurfaceApplyBoundaryReloadRunning`
- From: `Running`
- On: `SurfaceApplyBoundary`(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
- Guards:
  - `surface_operating`
  - `staged_reload`
  - `staged_sequence_matches`
  - `surface_active`
  - `base_active`
  - `no_pending_surface_op`
- Emits: `ScheduleSurfaceCompletion`, `EmitExternalToolDelta`
- To: `Running`

### `SurfaceApplyBoundaryRemoveDrainingAttached`
- From: `Attached`
- On: `SurfaceApplyBoundary`(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
- Guards:
  - `surface_operating`
  - `staged_remove`
  - `staged_sequence_matches`
  - `base_active`
  - `no_pending_surface_op`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Attached`

### `SurfaceApplyBoundaryRemoveDrainingRunning`
- From: `Running`
- On: `SurfaceApplyBoundary`(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
- Guards:
  - `surface_operating`
  - `staged_remove`
  - `staged_sequence_matches`
  - `base_active`
  - `no_pending_surface_op`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Running`

### `SurfaceApplyBoundaryRemoveNoopAttached`
- From: `Attached`
- On: `SurfaceApplyBoundary`(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
- Guards:
  - `surface_operating`
  - `staged_remove`
  - `staged_sequence_matches`
  - `base_not_active`
  - `no_pending_surface_op`
- To: `Attached`

### `SurfaceApplyBoundaryRemoveNoopRunning`
- From: `Running`
- On: `SurfaceApplyBoundary`(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
- Guards:
  - `surface_operating`
  - `staged_remove`
  - `staged_sequence_matches`
  - `base_not_active`
  - `no_pending_surface_op`
- To: `Running`

### `SurfaceMarkPendingSucceededAddAttached`
- From: `Attached`
- On: `SurfaceMarkPendingSucceeded`(surface_id, pending_task_sequence, staged_intent_sequence)
- Guards:
  - `pending_add`
  - `pending_sequence_matches`
  - `pending_lineage_matches`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Attached`

### `SurfaceMarkPendingSucceededAddRunning`
- From: `Running`
- On: `SurfaceMarkPendingSucceeded`(surface_id, pending_task_sequence, staged_intent_sequence)
- Guards:
  - `pending_add`
  - `pending_sequence_matches`
  - `pending_lineage_matches`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Running`

### `SurfaceMarkPendingSucceededReloadAttached`
- From: `Attached`
- On: `SurfaceMarkPendingSucceeded`(surface_id, pending_task_sequence, staged_intent_sequence)
- Guards:
  - `pending_reload`
  - `pending_sequence_matches`
  - `pending_lineage_matches`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Attached`

### `SurfaceMarkPendingSucceededReloadRunning`
- From: `Running`
- On: `SurfaceMarkPendingSucceeded`(surface_id, pending_task_sequence, staged_intent_sequence)
- Guards:
  - `pending_reload`
  - `pending_sequence_matches`
  - `pending_lineage_matches`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Running`

### `SurfaceMarkPendingFailedAttached`
- From: `Attached`
- On: `SurfaceMarkPendingFailed`(surface_id, pending_task_sequence, staged_intent_sequence, cause)
- Guards:
  - `pending_sequence_matches`
  - `pending_lineage_matches`
- Emits: `EmitExternalToolDelta`
- To: `Attached`

### `SurfaceMarkPendingFailedRunning`
- From: `Running`
- On: `SurfaceMarkPendingFailed`(surface_id, pending_task_sequence, staged_intent_sequence, cause)
- Guards:
  - `pending_sequence_matches`
  - `pending_lineage_matches`
- Emits: `EmitExternalToolDelta`
- To: `Running`

### `SurfaceCallStartedActiveAttached`
- From: `Attached`
- On: `SurfaceCallStarted`(surface_id)
- Guards:
  - `surface_active`
- To: `Attached`

### `SurfaceCallStartedActiveRunning`
- From: `Running`
- On: `SurfaceCallStarted`(surface_id)
- Guards:
  - `surface_active`
- To: `Running`

### `SurfaceCallStartedRejectRemovingAttached`
- From: `Attached`
- On: `SurfaceCallStarted`(surface_id)
- Guards:
  - `surface_removing`
- Emits: `RejectSurfaceCall`
- To: `Attached`

### `SurfaceCallStartedRejectRemovingRunning`
- From: `Running`
- On: `SurfaceCallStarted`(surface_id)
- Guards:
  - `surface_removing`
- Emits: `RejectSurfaceCall`
- To: `Running`

### `SurfaceCallStartedRejectUnavailableAttached`
- From: `Attached`
- On: `SurfaceCallStarted`(surface_id)
- Guards:
  - `surface_unavailable`
- Emits: `RejectSurfaceCall`
- To: `Attached`

### `SurfaceCallStartedRejectUnavailableRunning`
- From: `Running`
- On: `SurfaceCallStarted`(surface_id)
- Guards:
  - `surface_unavailable`
- Emits: `RejectSurfaceCall`
- To: `Running`

### `SurfaceCallFinishedAttached`
- From: `Attached`
- On: `SurfaceCallFinished`(surface_id)
- Guards:
  - `surface_active_or_removing`
  - `inflight_calls_remain`
- To: `Attached`

### `SurfaceCallFinishedRunning`
- From: `Running`
- On: `SurfaceCallFinished`(surface_id)
- Guards:
  - `surface_active_or_removing`
  - `inflight_calls_remain`
- To: `Running`

### `SurfaceFinalizeRemovalCleanAttached`
- From: `Attached`
- On: `SurfaceFinalizeRemovalClean`(surface_id)
- Guards:
  - `surface_removing`
  - `no_inflight_calls_remain`
- Emits: `CloseSurfaceConnection`, `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Attached`

### `SurfaceFinalizeRemovalCleanRunning`
- From: `Running`
- On: `SurfaceFinalizeRemovalClean`(surface_id)
- Guards:
  - `surface_removing`
  - `no_inflight_calls_remain`
- Emits: `CloseSurfaceConnection`, `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Running`

### `SurfaceFinalizeRemovalForcedAttached`
- From: `Attached`
- On: `SurfaceFinalizeRemovalForced`(surface_id)
- Guards:
  - `surface_removing`
- Emits: `CloseSurfaceConnection`, `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Attached`

### `SurfaceFinalizeRemovalForcedRunning`
- From: `Running`
- On: `SurfaceFinalizeRemovalForced`(surface_id)
- Guards:
  - `surface_removing`
- Emits: `CloseSurfaceConnection`, `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Running`

### `SurfaceSnapshotAlignedAttached`
- From: `Attached`
- On: `SurfaceSnapshotAligned`(epoch)
- To: `Attached`

### `SurfaceSnapshotAlignedRunning`
- From: `Running`
- On: `SurfaceSnapshotAligned`(epoch)
- To: `Running`

### `SurfaceShutdownAttached`
- From: `Attached`
- On: `SurfaceShutdown`()
- To: `Attached`

### `SurfaceShutdownRunning`
- From: `Running`
- On: `SurfaceShutdown`()
- To: `Running`

### `CommitRunningToIdle`
- From: `Running`
- On: `Commit`(input_id, run_id)
- Guards:
  - `pre_run_phase_matches_idle`
  - `current_run_id_matches_binding`
- To: `Idle`

### `CommitRunningToAttached`
- From: `Running`
- On: `Commit`(input_id, run_id)
- Guards:
  - `pre_run_phase_matches_attached`
  - `current_run_id_matches_binding`
- To: `Attached`

### `CommitRunningToRetired`
- From: `Running`
- On: `Commit`(input_id, run_id)
- Guards:
  - `pre_run_phase_matches_retired`
  - `current_run_id_matches_binding`
- To: `Retired`

### `FailRunningToIdle`
- From: `Running`
- On: `Fail`(run_id)
- Guards:
  - `pre_run_phase_matches_idle`
  - `current_run_id_matches_binding`
  - `turn_failed_with_cause`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `FailRunningToAttached`
- From: `Running`
- On: `Fail`(run_id)
- Guards:
  - `pre_run_phase_matches_attached`
  - `current_run_id_matches_binding`
  - `turn_failed_with_cause`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

### `FailRunningToRetired`
- From: `Running`
- On: `Fail`(run_id)
- Guards:
  - `pre_run_phase_matches_retired`
  - `current_run_id_matches_binding`
  - `turn_failed_with_cause`
- Emits: `RecordTerminalOutcome`
- To: `Retired`

### `RollbackRunRunningToIdle`
- From: `Running`
- On: `RollbackRun`(run_id)
- Guards:
  - `pre_run_phase_matches_idle`
  - `current_run_id_matches_binding`
- To: `Idle`

### `RollbackRunRunningToAttached`
- From: `Running`
- On: `RollbackRun`(run_id)
- Guards:
  - `pre_run_phase_matches_attached`
  - `current_run_id_matches_binding`
- To: `Attached`

### `RollbackRunRunningToRetired`
- From: `Running`
- On: `RollbackRun`(run_id)
- Guards:
  - `pre_run_phase_matches_retired`
  - `current_run_id_matches_binding`
- To: `Retired`

### `RecycleFromIdleOrRetired`
- From: `Idle`, `Retired`
- On: `Recycle`()
- Guards:
  - `session_registered`
- Emits: `InitiateRecycle`
- To: `Idle`

### `RecycleFromAttached`
- From: `Attached`
- On: `Recycle`()
- Guards:
  - `session_registered`
- Emits: `InitiateRecycle`
- To: `Attached`

### `RecoverInputLifecycleIdle`
- From: `Idle`
- On: `RecoverInputLifecycle`(input_id, phase, terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count, attempt_count, run_id, boundary_sequence, lane)
- Emits: `InputLifecycleNotice`
- To: `Idle`

### `RecoverInputLifecycleAttached`
- From: `Attached`
- On: `RecoverInputLifecycle`(input_id, phase, terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count, attempt_count, run_id, boundary_sequence, lane)
- Emits: `InputLifecycleNotice`
- To: `Attached`

### `RecoverInputLifecycleRunning`
- From: `Running`
- On: `RecoverInputLifecycle`(input_id, phase, terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count, attempt_count, run_id, boundary_sequence, lane)
- Emits: `InputLifecycleNotice`
- To: `Running`

### `RecoverInputLifecycleRetired`
- From: `Retired`
- On: `RecoverInputLifecycle`(input_id, phase, terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count, attempt_count, run_id, boundary_sequence, lane)
- Emits: `InputLifecycleNotice`
- To: `Retired`

### `RecoverInputLifecycleStopped`
- From: `Stopped`
- On: `RecoverInputLifecycle`(input_id, phase, terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count, attempt_count, run_id, boundary_sequence, lane)
- Emits: `InputLifecycleNotice`
- To: `Stopped`

### `QueueAcceptedIdle`
- From: `Idle`
- On: `QueueAccepted`(input_id)
- Guards:
  - `not_already_tracked`
- Emits: `IngressAccepted`
- To: `Idle`

### `QueueAcceptedAttached`
- From: `Attached`
- On: `QueueAccepted`(input_id)
- Guards:
  - `not_already_tracked`
- Emits: `IngressAccepted`
- To: `Attached`

### `QueueAcceptedRunning`
- From: `Running`
- On: `QueueAccepted`(input_id)
- Guards:
  - `not_already_tracked`
- Emits: `IngressAccepted`
- To: `Running`

### `QueueAcceptedRetired`
- From: `Retired`
- On: `QueueAccepted`(input_id)
- Guards:
  - `not_already_tracked`
- Emits: `IngressAccepted`
- To: `Retired`

### `QueueAcceptedStopped`
- From: `Stopped`
- On: `QueueAccepted`(input_id)
- Guards:
  - `not_already_tracked`
- Emits: `IngressAccepted`
- To: `Stopped`

### `SteerAcceptedIdle`
- From: `Idle`
- On: `SteerAccepted`(input_id)
- Guards:
  - `not_already_tracked`
- Emits: `IngressAccepted`
- To: `Idle`

### `SteerAcceptedAttached`
- From: `Attached`
- On: `SteerAccepted`(input_id)
- Guards:
  - `not_already_tracked`
- Emits: `IngressAccepted`
- To: `Attached`

### `SteerAcceptedRunning`
- From: `Running`
- On: `SteerAccepted`(input_id)
- Guards:
  - `not_already_tracked`
- Emits: `IngressAccepted`
- To: `Running`

### `SteerAcceptedRetired`
- From: `Retired`
- On: `SteerAccepted`(input_id)
- Guards:
  - `not_already_tracked`
- Emits: `IngressAccepted`
- To: `Retired`

### `SteerAcceptedStopped`
- From: `Stopped`
- On: `SteerAccepted`(input_id)
- Guards:
  - `not_already_tracked`
- Emits: `IngressAccepted`
- To: `Stopped`

### `ChangeLaneIdle`
- From: `Idle`
- On: `ChangeLane`(input_id, new_lane)
- Guards:
  - `input_tracked`
- To: `Idle`

### `ChangeLaneAttached`
- From: `Attached`
- On: `ChangeLane`(input_id, new_lane)
- Guards:
  - `input_tracked`
- To: `Attached`

### `ChangeLaneRunning`
- From: `Running`
- On: `ChangeLane`(input_id, new_lane)
- Guards:
  - `input_tracked`
- To: `Running`

### `ChangeLaneRetired`
- From: `Retired`
- On: `ChangeLane`(input_id, new_lane)
- Guards:
  - `input_tracked`
- To: `Retired`

### `ChangeLaneStopped`
- From: `Stopped`
- On: `ChangeLane`(input_id, new_lane)
- Guards:
  - `input_tracked`
- To: `Stopped`

### `StageForRunIdle`
- From: `Idle`
- On: `StageForRun`(input_id, run_id)
- Guards:
  - `input_tracked`
- Emits: `RecordRunAssociation`
- To: `Idle`

### `StageForRunAttached`
- From: `Attached`
- On: `StageForRun`(input_id, run_id)
- Guards:
  - `input_tracked`
- Emits: `RecordRunAssociation`
- To: `Attached`

### `StageForRunRunning`
- From: `Running`
- On: `StageForRun`(input_id, run_id)
- Guards:
  - `input_tracked`
- Emits: `RecordRunAssociation`
- To: `Running`

### `StageForRunRetired`
- From: `Retired`
- On: `StageForRun`(input_id, run_id)
- Guards:
  - `input_tracked`
- Emits: `RecordRunAssociation`
- To: `Retired`

### `StageForRunStopped`
- From: `Stopped`
- On: `StageForRun`(input_id, run_id)
- Guards:
  - `input_tracked`
- Emits: `RecordRunAssociation`
- To: `Stopped`

### `IncrementAttemptCountIdle`
- From: `Idle`
- On: `IncrementAttemptCount`(input_id)
- Guards:
  - `input_tracked`
- To: `Idle`

### `IncrementAttemptCountAttached`
- From: `Attached`
- On: `IncrementAttemptCount`(input_id)
- Guards:
  - `input_tracked`
- To: `Attached`

### `IncrementAttemptCountRunning`
- From: `Running`
- On: `IncrementAttemptCount`(input_id)
- Guards:
  - `input_tracked`
- To: `Running`

### `IncrementAttemptCountRetired`
- From: `Retired`
- On: `IncrementAttemptCount`(input_id)
- Guards:
  - `input_tracked`
- To: `Retired`

### `IncrementAttemptCountStopped`
- From: `Stopped`
- On: `IncrementAttemptCount`(input_id)
- Guards:
  - `input_tracked`
- To: `Stopped`

### `RollbackStagedIdle`
- From: `Idle`
- On: `RollbackStaged`(input_id, lane)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Idle`

### `RollbackStagedAttached`
- From: `Attached`
- On: `RollbackStaged`(input_id, lane)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Attached`

### `RollbackStagedRunning`
- From: `Running`
- On: `RollbackStaged`(input_id, lane)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Running`

### `RollbackStagedRetired`
- From: `Retired`
- On: `RollbackStaged`(input_id, lane)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Retired`

### `RollbackStagedStopped`
- From: `Stopped`
- On: `RollbackStaged`(input_id, lane)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Stopped`

### `MarkAppliedIdle`
- From: `Idle`
- On: `MarkApplied`(input_id)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Idle`

### `MarkAppliedAttached`
- From: `Attached`
- On: `MarkApplied`(input_id)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Attached`

### `MarkAppliedRunning`
- From: `Running`
- On: `MarkApplied`(input_id)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Running`

### `MarkAppliedRetired`
- From: `Retired`
- On: `MarkApplied`(input_id)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Retired`

### `MarkAppliedStopped`
- From: `Stopped`
- On: `MarkApplied`(input_id)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Stopped`

### `MarkAppliedPendingConsumptionIdle`
- From: `Idle`
- On: `MarkAppliedPendingConsumption`(input_id)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Idle`

### `MarkAppliedPendingConsumptionAttached`
- From: `Attached`
- On: `MarkAppliedPendingConsumption`(input_id)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Attached`

### `MarkAppliedPendingConsumptionRunning`
- From: `Running`
- On: `MarkAppliedPendingConsumption`(input_id)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Running`

### `MarkAppliedPendingConsumptionRetired`
- From: `Retired`
- On: `MarkAppliedPendingConsumption`(input_id)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Retired`

### `MarkAppliedPendingConsumptionStopped`
- From: `Stopped`
- On: `MarkAppliedPendingConsumption`(input_id)
- Guards:
  - `input_tracked`
- Emits: `InputLifecycleNotice`
- To: `Stopped`

### `ConsumeOnAcceptIdle`
- From: `Idle`
- On: `ConsumeOnAccept`(input_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `ConsumeOnAcceptAttached`
- From: `Attached`
- On: `ConsumeOnAccept`(input_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

### `ConsumeOnAcceptRunning`
- From: `Running`
- On: `ConsumeOnAccept`(input_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `ConsumeOnAcceptRetired`
- From: `Retired`
- On: `ConsumeOnAccept`(input_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Retired`

### `ConsumeOnAcceptStopped`
- From: `Stopped`
- On: `ConsumeOnAccept`(input_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Stopped`

### `RecordBoundarySeqIdle`
- From: `Idle`
- On: `RecordBoundarySeq`(input_id, seq)
- Guards:
  - `input_tracked`
- Emits: `RecordBoundarySequence`
- To: `Idle`

### `RecordBoundarySeqAttached`
- From: `Attached`
- On: `RecordBoundarySeq`(input_id, seq)
- Guards:
  - `input_tracked`
- Emits: `RecordBoundarySequence`
- To: `Attached`

### `RecordBoundarySeqRunning`
- From: `Running`
- On: `RecordBoundarySeq`(input_id, seq)
- Guards:
  - `input_tracked`
- Emits: `RecordBoundarySequence`
- To: `Running`

### `RecordBoundarySeqRetired`
- From: `Retired`
- On: `RecordBoundarySeq`(input_id, seq)
- Guards:
  - `input_tracked`
- Emits: `RecordBoundarySequence`
- To: `Retired`

### `RecordBoundarySeqStopped`
- From: `Stopped`
- On: `RecordBoundarySeq`(input_id, seq)
- Guards:
  - `input_tracked`
- Emits: `RecordBoundarySequence`
- To: `Stopped`

### `ConsumeInputIdle`
- From: `Idle`
- On: `ConsumeInput`(input_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `ConsumeInputAttached`
- From: `Attached`
- On: `ConsumeInput`(input_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

### `ConsumeInputRunning`
- From: `Running`
- On: `ConsumeInput`(input_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `ConsumeInputRetired`
- From: `Retired`
- On: `ConsumeInput`(input_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Retired`

### `ConsumeInputStopped`
- From: `Stopped`
- On: `ConsumeInput`(input_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Stopped`

### `SupersedeInputIdle`
- From: `Idle`
- On: `SupersedeInput`(input_id, superseded_by)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `SupersedeInputAttached`
- From: `Attached`
- On: `SupersedeInput`(input_id, superseded_by)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

### `SupersedeInputRunning`
- From: `Running`
- On: `SupersedeInput`(input_id, superseded_by)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `SupersedeInputRetired`
- From: `Retired`
- On: `SupersedeInput`(input_id, superseded_by)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Retired`

### `SupersedeInputStopped`
- From: `Stopped`
- On: `SupersedeInput`(input_id, superseded_by)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Stopped`

### `CoalesceInputIdle`
- From: `Idle`
- On: `CoalesceInput`(input_id, aggregate_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `CoalesceInputAttached`
- From: `Attached`
- On: `CoalesceInput`(input_id, aggregate_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

### `CoalesceInputRunning`
- From: `Running`
- On: `CoalesceInput`(input_id, aggregate_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `CoalesceInputRetired`
- From: `Retired`
- On: `CoalesceInput`(input_id, aggregate_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Retired`

### `CoalesceInputStopped`
- From: `Stopped`
- On: `CoalesceInput`(input_id, aggregate_id)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Stopped`

### `AbandonInputIdle`
- From: `Idle`
- On: `AbandonInput`(input_id, reason, attempt_count)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `AbandonInputAttached`
- From: `Attached`
- On: `AbandonInput`(input_id, reason, attempt_count)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

### `AbandonInputRunning`
- From: `Running`
- On: `AbandonInput`(input_id, reason, attempt_count)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `AbandonInputRetired`
- From: `Retired`
- On: `AbandonInput`(input_id, reason, attempt_count)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Retired`

### `AbandonInputStopped`
- From: `Stopped`
- On: `AbandonInput`(input_id, reason, attempt_count)
- Guards:
  - `input_tracked`
- Emits: `RecordTerminalOutcome`
- To: `Stopped`

### `RegisterOpIdle`
- From: `Idle`
- On: `RegisterOp`(operation_id, kind)
- Guards:
  - `not_already_registered`
- Emits: `SubmitOpEvent`
- To: `Idle`

### `RegisterOpAttached`
- From: `Attached`
- On: `RegisterOp`(operation_id, kind)
- Guards:
  - `not_already_registered`
- Emits: `SubmitOpEvent`
- To: `Attached`

### `RegisterOpRunning`
- From: `Running`
- On: `RegisterOp`(operation_id, kind)
- Guards:
  - `not_already_registered`
- Emits: `SubmitOpEvent`
- To: `Running`

### `RegisterOpRetired`
- From: `Retired`
- On: `RegisterOp`(operation_id, kind)
- Guards:
  - `not_already_registered`
- Emits: `SubmitOpEvent`
- To: `Retired`

### `RegisterOpStopped`
- From: `Stopped`
- On: `RegisterOp`(operation_id, kind)
- Guards:
  - `not_already_registered`
- Emits: `SubmitOpEvent`
- To: `Stopped`

### `StartOpIdle`
- From: `Idle`
- On: `StartOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Idle`

### `StartOpAttached`
- From: `Attached`
- On: `StartOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Attached`

### `StartOpRunning`
- From: `Running`
- On: `StartOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Running`

### `StartOpRetired`
- From: `Retired`
- On: `StartOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Retired`

### `StartOpStopped`
- From: `Stopped`
- On: `StartOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Stopped`

### `CompleteOpIdle`
- From: `Idle`
- On: `CompleteOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Idle`

### `CompleteOpAttached`
- From: `Attached`
- On: `CompleteOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Attached`

### `CompleteOpRunning`
- From: `Running`
- On: `CompleteOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Running`

### `CompleteOpRetired`
- From: `Retired`
- On: `CompleteOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Retired`

### `CompleteOpStopped`
- From: `Stopped`
- On: `CompleteOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Stopped`

### `FailOpIdle`
- From: `Idle`
- On: `FailOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Idle`

### `FailOpAttached`
- From: `Attached`
- On: `FailOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Attached`

### `FailOpRunning`
- From: `Running`
- On: `FailOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Running`

### `FailOpRetired`
- From: `Retired`
- On: `FailOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Retired`

### `FailOpStopped`
- From: `Stopped`
- On: `FailOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Stopped`

### `CancelOpIdle`
- From: `Idle`
- On: `CancelOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Idle`

### `CancelOpAttached`
- From: `Attached`
- On: `CancelOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Attached`

### `CancelOpRunning`
- From: `Running`
- On: `CancelOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Running`

### `CancelOpRetired`
- From: `Retired`
- On: `CancelOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Retired`

### `CancelOpStopped`
- From: `Stopped`
- On: `CancelOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Stopped`

### `AbortOpIdle`
- From: `Idle`
- On: `AbortOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Idle`

### `AbortOpAttached`
- From: `Attached`
- On: `AbortOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Attached`

### `AbortOpRunning`
- From: `Running`
- On: `AbortOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Running`

### `AbortOpRetired`
- From: `Retired`
- On: `AbortOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Retired`

### `AbortOpStopped`
- From: `Stopped`
- On: `AbortOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Stopped`

### `PeerReadyOpIdle`
- From: `Idle`
- On: `PeerReadyOp`(operation_id)
- Guards:
  - `op_registered`
  - `kind_is_mob_member_child`
  - `not_already_peer_ready`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Idle`

### `PeerReadyOpAttached`
- From: `Attached`
- On: `PeerReadyOp`(operation_id)
- Guards:
  - `op_registered`
  - `kind_is_mob_member_child`
  - `not_already_peer_ready`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Attached`

### `PeerReadyOpRunning`
- From: `Running`
- On: `PeerReadyOp`(operation_id)
- Guards:
  - `op_registered`
  - `kind_is_mob_member_child`
  - `not_already_peer_ready`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Running`

### `PeerReadyOpRetired`
- From: `Retired`
- On: `PeerReadyOp`(operation_id)
- Guards:
  - `op_registered`
  - `kind_is_mob_member_child`
  - `not_already_peer_ready`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Retired`

### `PeerReadyOpStopped`
- From: `Stopped`
- On: `PeerReadyOp`(operation_id)
- Guards:
  - `op_registered`
  - `kind_is_mob_member_child`
  - `not_already_peer_ready`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Stopped`

### `ProgressReportedOpIdle`
- From: `Idle`
- On: `ProgressReportedOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Idle`

### `ProgressReportedOpAttached`
- From: `Attached`
- On: `ProgressReportedOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Attached`

### `ProgressReportedOpRunning`
- From: `Running`
- On: `ProgressReportedOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Running`

### `ProgressReportedOpRetired`
- From: `Retired`
- On: `ProgressReportedOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Retired`

### `ProgressReportedOpStopped`
- From: `Stopped`
- On: `ProgressReportedOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Stopped`

### `RetireRequestedOpIdle`
- From: `Idle`
- On: `RetireRequestedOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Idle`

### `RetireRequestedOpAttached`
- From: `Attached`
- On: `RetireRequestedOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Attached`

### `RetireRequestedOpRunning`
- From: `Running`
- On: `RetireRequestedOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Running`

### `RetireRequestedOpRetired`
- From: `Retired`
- On: `RetireRequestedOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Retired`

### `RetireRequestedOpStopped`
- From: `Stopped`
- On: `RetireRequestedOp`(operation_id)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`
- To: `Stopped`

### `RetireCompletedOpIdle`
- From: `Idle`
- On: `RetireCompletedOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Idle`

### `RetireCompletedOpAttached`
- From: `Attached`
- On: `RetireCompletedOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Attached`

### `RetireCompletedOpRunning`
- From: `Running`
- On: `RetireCompletedOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Running`

### `RetireCompletedOpRetired`
- From: `Retired`
- On: `RetireCompletedOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Retired`

### `RetireCompletedOpStopped`
- From: `Stopped`
- On: `RetireCompletedOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Stopped`

### `TerminateOpIdle`
- From: `Idle`
- On: `TerminateOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Idle`

### `TerminateOpAttached`
- From: `Attached`
- On: `TerminateOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Attached`

### `TerminateOpRunning`
- From: `Running`
- On: `TerminateOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Running`

### `TerminateOpRetired`
- From: `Retired`
- On: `TerminateOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Retired`

### `TerminateOpStopped`
- From: `Stopped`
- On: `TerminateOp`(operation_id, outcome, payload)
- Guards:
  - `op_registered`
  - `from_status_valid`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`
- To: `Stopped`

### `RequestWaitAllIdle`
- From: `Idle`
- On: `RequestWaitAll`(wait_request_id, operation_ids, operation_id_tokens)
- To: `Idle`

### `RequestWaitAllAttached`
- From: `Attached`
- On: `RequestWaitAll`(wait_request_id, operation_ids, operation_id_tokens)
- To: `Attached`

### `RequestWaitAllRunning`
- From: `Running`
- On: `RequestWaitAll`(wait_request_id, operation_ids, operation_id_tokens)
- To: `Running`

### `RequestWaitAllRetired`
- From: `Retired`
- On: `RequestWaitAll`(wait_request_id, operation_ids, operation_id_tokens)
- To: `Retired`

### `RequestWaitAllStopped`
- From: `Stopped`
- On: `RequestWaitAll`(wait_request_id, operation_ids, operation_id_tokens)
- To: `Stopped`

### `SatisfyWaitAllIdle`
- From: `Idle`
- On: `SatisfyWaitAll`(wait_request_id, operation_id_tokens)
- Guards:
  - `wait_is_active`
  - `wait_request_matches`
  - `operation_tokens_match`
  - `all_members_terminal`
- Emits: `WaitAllSatisfied`
- To: `Idle`

### `SatisfyWaitAllAttached`
- From: `Attached`
- On: `SatisfyWaitAll`(wait_request_id, operation_id_tokens)
- Guards:
  - `wait_is_active`
  - `wait_request_matches`
  - `operation_tokens_match`
  - `all_members_terminal`
- Emits: `WaitAllSatisfied`
- To: `Attached`

### `SatisfyWaitAllRunning`
- From: `Running`
- On: `SatisfyWaitAll`(wait_request_id, operation_id_tokens)
- Guards:
  - `wait_is_active`
  - `wait_request_matches`
  - `operation_tokens_match`
  - `all_members_terminal`
- Emits: `WaitAllSatisfied`
- To: `Running`

### `SatisfyWaitAllRetired`
- From: `Retired`
- On: `SatisfyWaitAll`(wait_request_id, operation_id_tokens)
- Guards:
  - `wait_is_active`
  - `wait_request_matches`
  - `operation_tokens_match`
  - `all_members_terminal`
- Emits: `WaitAllSatisfied`
- To: `Retired`

### `SatisfyWaitAllStopped`
- From: `Stopped`
- On: `SatisfyWaitAll`(wait_request_id, operation_id_tokens)
- Guards:
  - `wait_is_active`
  - `wait_request_matches`
  - `operation_tokens_match`
  - `all_members_terminal`
- Emits: `WaitAllSatisfied`
- To: `Stopped`

### `CancelWaitAllIdle`
- From: `Idle`
- On: `CancelWaitAll`()
- Guards:
  - `wait_is_active`
- To: `Idle`

### `CancelWaitAllAttached`
- From: `Attached`
- On: `CancelWaitAll`()
- Guards:
  - `wait_is_active`
- To: `Attached`

### `CancelWaitAllRunning`
- From: `Running`
- On: `CancelWaitAll`()
- Guards:
  - `wait_is_active`
- To: `Running`

### `CancelWaitAllRetired`
- From: `Retired`
- On: `CancelWaitAll`()
- Guards:
  - `wait_is_active`
- To: `Retired`

### `CancelWaitAllStopped`
- From: `Stopped`
- On: `CancelWaitAll`()
- Guards:
  - `wait_is_active`
- To: `Stopped`

### `SpawnDrainIdle`
- From: `Idle`
- On: `SpawnDrain`(mode)
- Guards:
  - `drain_can_spawn`
- Emits: `SpawnDrainTask`
- To: `Idle`

### `SpawnDrainAttached`
- From: `Attached`
- On: `SpawnDrain`(mode)
- Guards:
  - `drain_can_spawn`
- Emits: `SpawnDrainTask`
- To: `Attached`

### `SpawnDrainRunning`
- From: `Running`
- On: `SpawnDrain`(mode)
- Guards:
  - `drain_can_spawn`
- Emits: `SpawnDrainTask`
- To: `Running`

### `SpawnDrainRetired`
- From: `Retired`
- On: `SpawnDrain`(mode)
- Guards:
  - `drain_can_spawn`
- Emits: `SpawnDrainTask`
- To: `Retired`

### `SpawnDrainStopped`
- From: `Stopped`
- On: `SpawnDrain`(mode)
- Guards:
  - `drain_can_spawn`
- Emits: `SpawnDrainTask`
- To: `Stopped`

### `StopDrainIdle`
- From: `Idle`
- On: `StopDrain`()
- Guards:
  - `drain_is_running`
- To: `Idle`

### `StopDrainAttached`
- From: `Attached`
- On: `StopDrain`()
- Guards:
  - `drain_is_running`
- To: `Attached`

### `StopDrainRunning`
- From: `Running`
- On: `StopDrain`()
- Guards:
  - `drain_is_running`
- To: `Running`

### `StopDrainRetired`
- From: `Retired`
- On: `StopDrain`()
- Guards:
  - `drain_is_running`
- To: `Retired`

### `StopDrainStopped`
- From: `Stopped`
- On: `StopDrain`()
- Guards:
  - `drain_is_running`
- To: `Stopped`

### `DrainExitedCleanIdle`
- From: `Idle`
- On: `DrainExitedClean`()
- To: `Idle`

### `DrainExitedCleanAttached`
- From: `Attached`
- On: `DrainExitedClean`()
- To: `Attached`

### `DrainExitedCleanRunning`
- From: `Running`
- On: `DrainExitedClean`()
- To: `Running`

### `DrainExitedCleanRetired`
- From: `Retired`
- On: `DrainExitedClean`()
- To: `Retired`

### `DrainExitedCleanStopped`
- From: `Stopped`
- On: `DrainExitedClean`()
- To: `Stopped`

### `DrainExitedRespawnableIdle`
- From: `Idle`
- On: `DrainExitedRespawnable`()
- To: `Idle`

### `DrainExitedRespawnableAttached`
- From: `Attached`
- On: `DrainExitedRespawnable`()
- To: `Attached`

### `DrainExitedRespawnableRunning`
- From: `Running`
- On: `DrainExitedRespawnable`()
- To: `Running`

### `DrainExitedRespawnableRetired`
- From: `Retired`
- On: `DrainExitedRespawnable`()
- To: `Retired`

### `DrainExitedRespawnableStopped`
- From: `Stopped`
- On: `DrainExitedRespawnable`()
- To: `Stopped`

### `StageVisibilityFilterIdle`
- From: `Idle`
- On: `StageVisibilityFilter`(filter)
- Emits: `RefreshVisibleSurfaceSet`
- To: `Idle`

### `StageVisibilityFilterAttached`
- From: `Attached`
- On: `StageVisibilityFilter`(filter)
- Emits: `RefreshVisibleSurfaceSet`
- To: `Attached`

### `StageVisibilityFilterRunning`
- From: `Running`
- On: `StageVisibilityFilter`(filter)
- Emits: `RefreshVisibleSurfaceSet`
- To: `Running`

### `StageVisibilityFilterRetired`
- From: `Retired`
- On: `StageVisibilityFilter`(filter)
- Emits: `RefreshVisibleSurfaceSet`
- To: `Retired`

### `StageVisibilityFilterStopped`
- From: `Stopped`
- On: `StageVisibilityFilter`(filter)
- Emits: `RefreshVisibleSurfaceSet`
- To: `Stopped`

### `CommitVisibilityFilterIdle`
- From: `Idle`
- On: `CommitVisibilityFilter`(filter, revision)
- Emits: `RefreshVisibleSurfaceSet`
- To: `Idle`

### `CommitVisibilityFilterAttached`
- From: `Attached`
- On: `CommitVisibilityFilter`(filter, revision)
- Emits: `RefreshVisibleSurfaceSet`
- To: `Attached`

### `CommitVisibilityFilterRunning`
- From: `Running`
- On: `CommitVisibilityFilter`(filter, revision)
- Emits: `RefreshVisibleSurfaceSet`
- To: `Running`

### `CommitVisibilityFilterRetired`
- From: `Retired`
- On: `CommitVisibilityFilter`(filter, revision)
- Emits: `RefreshVisibleSurfaceSet`
- To: `Retired`

### `CommitVisibilityFilterStopped`
- From: `Stopped`
- On: `CommitVisibilityFilter`(filter, revision)
- Emits: `RefreshVisibleSurfaceSet`
- To: `Stopped`

### `StageDeferredNamesIdle`
- From: `Idle`
- On: `StageDeferredNames`(names)
- Guards:
  - `deferred_names_empty`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Idle`

### `StageDeferredNamesAttached`
- From: `Attached`
- On: `StageDeferredNames`(names)
- Guards:
  - `deferred_names_empty`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Attached`

### `StageDeferredNamesRunning`
- From: `Running`
- On: `StageDeferredNames`(names)
- Guards:
  - `deferred_names_empty`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Running`

### `StageDeferredNamesRetired`
- From: `Retired`
- On: `StageDeferredNames`(names)
- Guards:
  - `deferred_names_empty`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Retired`

### `StageDeferredNamesStopped`
- From: `Stopped`
- On: `StageDeferredNames`(names)
- Guards:
  - `deferred_names_empty`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Stopped`

### `CommitDeferredNamesIdle`
- From: `Idle`
- On: `CommitDeferredNames`(authorities)
- Guards:
  - `deferred_authorities_have_identity`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Idle`

### `CommitDeferredNamesAttached`
- From: `Attached`
- On: `CommitDeferredNames`(authorities)
- Guards:
  - `deferred_authorities_have_identity`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Attached`

### `CommitDeferredNamesRunning`
- From: `Running`
- On: `CommitDeferredNames`(authorities)
- Guards:
  - `deferred_authorities_have_identity`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Running`

### `CommitDeferredNamesRetired`
- From: `Retired`
- On: `CommitDeferredNames`(authorities)
- Guards:
  - `deferred_authorities_have_identity`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Retired`

### `CommitDeferredNamesStopped`
- From: `Stopped`
- On: `CommitDeferredNames`(authorities)
- Guards:
  - `deferred_authorities_have_identity`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Stopped`

### `SyncVisibilityRevisionsIdle`
- From: `Idle`
- On: `SyncVisibilityRevisions`(active_revision, staged_revision, active_deferred_names, staged_deferred_names, active_deferred_authorities, staged_deferred_authorities)
- Guards:
  - `counter_advances`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- To: `Idle`

### `SyncVisibilityRevisionsAttached`
- From: `Attached`
- On: `SyncVisibilityRevisions`(active_revision, staged_revision, active_deferred_names, staged_deferred_names, active_deferred_authorities, staged_deferred_authorities)
- Guards:
  - `counter_advances`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- To: `Attached`

### `SyncVisibilityRevisionsRunning`
- From: `Running`
- On: `SyncVisibilityRevisions`(active_revision, staged_revision, active_deferred_names, staged_deferred_names, active_deferred_authorities, staged_deferred_authorities)
- Guards:
  - `counter_advances`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- To: `Running`

### `SyncVisibilityRevisionsRetired`
- From: `Retired`
- On: `SyncVisibilityRevisions`(active_revision, staged_revision, active_deferred_names, staged_deferred_names, active_deferred_authorities, staged_deferred_authorities)
- Guards:
  - `counter_advances`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- To: `Retired`

### `SyncVisibilityRevisionsStopped`
- From: `Stopped`
- On: `SyncVisibilityRevisions`(active_revision, staged_revision, active_deferred_names, staged_deferred_names, active_deferred_authorities, staged_deferred_authorities)
- Guards:
  - `counter_advances`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- To: `Stopped`

### `ProjectRealtimeIntentIdle`
- From: `Idle`
- On: `ProjectRealtimeIntent`(present)
- Guards:
  - `session_registered`
- Emits: `RealtimeIntentProjected`
- To: `Idle`

### `ProjectRealtimeIntentAttached`
- From: `Attached`
- On: `ProjectRealtimeIntent`(present)
- Guards:
  - `session_registered`
- Emits: `RealtimeIntentProjected`
- To: `Attached`

### `ProjectRealtimeIntentRunning`
- From: `Running`
- On: `ProjectRealtimeIntent`(present)
- Guards:
  - `session_registered`
- Emits: `RealtimeIntentProjected`
- To: `Running`

### `ProjectRealtimeIntentRetired`
- From: `Retired`
- On: `ProjectRealtimeIntent`(present)
- Guards:
  - `session_registered`
- Emits: `RealtimeIntentProjected`
- To: `Retired`

### `ProjectRealtimeIntentStopped`
- From: `Stopped`
- On: `ProjectRealtimeIntent`(present)
- Guards:
  - `session_registered`
- Emits: `RealtimeIntentProjected`
- To: `Stopped`

### `BeginRealtimeBindingIdle`
- From: `Idle`
- On: `BeginRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Idle`

### `BeginRealtimeBindingAttached`
- From: `Attached`
- On: `BeginRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Attached`

### `BeginRealtimeBindingRunning`
- From: `Running`
- On: `BeginRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Running`

### `BeginRealtimeBindingRetired`
- From: `Retired`
- On: `BeginRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Retired`

### `BeginRealtimeBindingStopped`
- From: `Stopped`
- On: `BeginRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Stopped`

### `ReplaceRealtimeBindingIdle`
- From: `Idle`
- On: `ReplaceRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Idle`

### `ReplaceRealtimeBindingAttached`
- From: `Attached`
- On: `ReplaceRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Attached`

### `ReplaceRealtimeBindingRunning`
- From: `Running`
- On: `ReplaceRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Running`

### `ReplaceRealtimeBindingRetired`
- From: `Retired`
- On: `ReplaceRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Retired`

### `ReplaceRealtimeBindingStopped`
- From: `Stopped`
- On: `ReplaceRealtimeBinding`()
- Guards:
  - `session_registered`
  - `no_topology_reconfigure_in_progress`
- Emits: `RealtimeBindingRotated`
- To: `Stopped`

### `DetachRealtimeBindingIdle`
- From: `Idle`
- On: `DetachRealtimeBinding`()
- Guards:
  - `session_registered`
- To: `Idle`

### `DetachRealtimeBindingAttached`
- From: `Attached`
- On: `DetachRealtimeBinding`()
- Guards:
  - `session_registered`
- To: `Attached`

### `DetachRealtimeBindingRunning`
- From: `Running`
- On: `DetachRealtimeBinding`()
- Guards:
  - `session_registered`
- To: `Running`

### `DetachRealtimeBindingRetired`
- From: `Retired`
- On: `DetachRealtimeBinding`()
- Guards:
  - `session_registered`
- To: `Retired`

### `DetachRealtimeBindingStopped`
- From: `Stopped`
- On: `DetachRealtimeBinding`()
- Guards:
  - `session_registered`
- To: `Stopped`

### `RequireRealtimeReattachIdle`
- From: `Idle`
- On: `RequireRealtimeReattach`()
- Guards:
  - `session_registered`
- To: `Idle`

### `RequireRealtimeReattachAttached`
- From: `Attached`
- On: `RequireRealtimeReattach`()
- Guards:
  - `session_registered`
- To: `Attached`

### `RequireRealtimeReattachRunning`
- From: `Running`
- On: `RequireRealtimeReattach`()
- Guards:
  - `session_registered`
- To: `Running`

### `RequireRealtimeReattachRetired`
- From: `Retired`
- On: `RequireRealtimeReattach`()
- Guards:
  - `session_registered`
- To: `Retired`

### `RequireRealtimeReattachStopped`
- From: `Stopped`
- On: `RequireRealtimeReattach`()
- Guards:
  - `session_registered`
- To: `Stopped`

### `RequireRealtimeReattachForAuthorityIdle`
- From: `Idle`
- On: `RequireRealtimeReattachForAuthority`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
- To: `Idle`

### `RequireRealtimeReattachForAuthorityAttached`
- From: `Attached`
- On: `RequireRealtimeReattachForAuthority`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
- To: `Attached`

### `RequireRealtimeReattachForAuthorityRunning`
- From: `Running`
- On: `RequireRealtimeReattachForAuthority`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
- To: `Running`

### `RequireRealtimeReattachForAuthorityRetired`
- From: `Retired`
- On: `RequireRealtimeReattachForAuthority`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
- To: `Retired`

### `RequireRealtimeReattachForAuthorityStopped`
- From: `Stopped`
- On: `RequireRealtimeReattachForAuthority`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
- To: `Stopped`

### `PublishRealtimeSignalIdle`
- From: `Idle`
- On: `PublishRealtimeSignal`(authority_epoch, next_binding_state)
- Guards:
  - `authority_matches_current`
  - `no_topology_reconfigure_in_progress`
  - `valid_next_state`
- To: `Idle`

### `PublishRealtimeSignalAttached`
- From: `Attached`
- On: `PublishRealtimeSignal`(authority_epoch, next_binding_state)
- Guards:
  - `authority_matches_current`
  - `no_topology_reconfigure_in_progress`
  - `valid_next_state`
- To: `Attached`

### `PublishRealtimeSignalRunning`
- From: `Running`
- On: `PublishRealtimeSignal`(authority_epoch, next_binding_state)
- Guards:
  - `authority_matches_current`
  - `no_topology_reconfigure_in_progress`
  - `valid_next_state`
- To: `Running`

### `PublishRealtimeSignalRetired`
- From: `Retired`
- On: `PublishRealtimeSignal`(authority_epoch, next_binding_state)
- Guards:
  - `authority_matches_current`
  - `no_topology_reconfigure_in_progress`
  - `valid_next_state`
- To: `Retired`

### `PublishRealtimeSignalStopped`
- From: `Stopped`
- On: `PublishRealtimeSignal`(authority_epoch, next_binding_state)
- Guards:
  - `authority_matches_current`
  - `no_topology_reconfigure_in_progress`
  - `valid_next_state`
- To: `Stopped`

### `BeginRealtimeReconnectCycleIdle`
- From: `Idle`
- On: `BeginRealtimeReconnectCycle`(next_retry_at_ms, deadline_at_ms)
- Guards:
  - `session_registered`
  - `reattach_required`
  - `cycle_idle`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Idle`

### `BeginRealtimeReconnectCycleAttached`
- From: `Attached`
- On: `BeginRealtimeReconnectCycle`(next_retry_at_ms, deadline_at_ms)
- Guards:
  - `session_registered`
  - `reattach_required`
  - `cycle_idle`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Attached`

### `BeginRealtimeReconnectCycleRunning`
- From: `Running`
- On: `BeginRealtimeReconnectCycle`(next_retry_at_ms, deadline_at_ms)
- Guards:
  - `session_registered`
  - `reattach_required`
  - `cycle_idle`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Running`

### `BeginRealtimeReconnectCycleRetired`
- From: `Retired`
- On: `BeginRealtimeReconnectCycle`(next_retry_at_ms, deadline_at_ms)
- Guards:
  - `session_registered`
  - `reattach_required`
  - `cycle_idle`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Retired`

### `BeginRealtimeReconnectCycleStopped`
- From: `Stopped`
- On: `BeginRealtimeReconnectCycle`(next_retry_at_ms, deadline_at_ms)
- Guards:
  - `session_registered`
  - `reattach_required`
  - `cycle_idle`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Stopped`

### `ScheduleRealtimeReconnectRetryIdle`
- From: `Idle`
- On: `ScheduleRealtimeReconnectRetry`(next_retry_at_ms)
- Guards:
  - `session_registered`
  - `cycle_reconnecting`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Idle`

### `ScheduleRealtimeReconnectRetryAttached`
- From: `Attached`
- On: `ScheduleRealtimeReconnectRetry`(next_retry_at_ms)
- Guards:
  - `session_registered`
  - `cycle_reconnecting`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Attached`

### `ScheduleRealtimeReconnectRetryRunning`
- From: `Running`
- On: `ScheduleRealtimeReconnectRetry`(next_retry_at_ms)
- Guards:
  - `session_registered`
  - `cycle_reconnecting`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Running`

### `ScheduleRealtimeReconnectRetryRetired`
- From: `Retired`
- On: `ScheduleRealtimeReconnectRetry`(next_retry_at_ms)
- Guards:
  - `session_registered`
  - `cycle_reconnecting`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Retired`

### `ScheduleRealtimeReconnectRetryStopped`
- From: `Stopped`
- On: `ScheduleRealtimeReconnectRetry`(next_retry_at_ms)
- Guards:
  - `session_registered`
  - `cycle_reconnecting`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Stopped`

### `ExhaustRealtimeReconnectCycleIdle`
- From: `Idle`
- On: `ExhaustRealtimeReconnectCycle`()
- Guards:
  - `session_registered`
  - `cycle_reconnecting`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Idle`

### `ExhaustRealtimeReconnectCycleAttached`
- From: `Attached`
- On: `ExhaustRealtimeReconnectCycle`()
- Guards:
  - `session_registered`
  - `cycle_reconnecting`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Attached`

### `ExhaustRealtimeReconnectCycleRunning`
- From: `Running`
- On: `ExhaustRealtimeReconnectCycle`()
- Guards:
  - `session_registered`
  - `cycle_reconnecting`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Running`

### `ExhaustRealtimeReconnectCycleRetired`
- From: `Retired`
- On: `ExhaustRealtimeReconnectCycle`()
- Guards:
  - `session_registered`
  - `cycle_reconnecting`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Retired`

### `ExhaustRealtimeReconnectCycleStopped`
- From: `Stopped`
- On: `ExhaustRealtimeReconnectCycle`()
- Guards:
  - `session_registered`
  - `cycle_reconnecting`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Stopped`

### `ClearRealtimeReconnectProgressIdle`
- From: `Idle`
- On: `ClearRealtimeReconnectProgress`()
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Idle`

### `ClearRealtimeReconnectProgressAttached`
- From: `Attached`
- On: `ClearRealtimeReconnectProgress`()
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Attached`

### `ClearRealtimeReconnectProgressRunning`
- From: `Running`
- On: `ClearRealtimeReconnectProgress`()
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Running`

### `ClearRealtimeReconnectProgressRetired`
- From: `Retired`
- On: `ClearRealtimeReconnectProgress`()
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Retired`

### `ClearRealtimeReconnectProgressStopped`
- From: `Stopped`
- On: `ClearRealtimeReconnectProgress`()
- Guards:
  - `session_registered`
- Emits: `RealtimeReconnectProgressProjected`
- To: `Stopped`

### `BeginLiveTopologyReconfigureIdle`
- From: `Idle`
- On: `BeginLiveTopologyReconfigure`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
  - `topology_idle`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `BeginLiveTopologyReconfigureAttached`
- From: `Attached`
- On: `BeginLiveTopologyReconfigure`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
  - `topology_idle`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `BeginLiveTopologyReconfigureRunning`
- From: `Running`
- On: `BeginLiveTopologyReconfigure`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
  - `topology_idle`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `BeginLiveTopologyReconfigureRetired`
- From: `Retired`
- On: `BeginLiveTopologyReconfigure`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
  - `topology_idle`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `BeginLiveTopologyReconfigureStopped`
- From: `Stopped`
- On: `BeginLiveTopologyReconfigure`(authority_epoch)
- Guards:
  - `session_registered`
  - `authority_matches_current`
  - `topology_idle`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `MarkLiveTopologyDetachedIdle`
- From: `Idle`
- On: `MarkLiveTopologyDetached`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
  - `turn_at_safe_boundary`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `MarkLiveTopologyDetachedAttached`
- From: `Attached`
- On: `MarkLiveTopologyDetached`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
  - `turn_at_safe_boundary`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `MarkLiveTopologyDetachedRunning`
- From: `Running`
- On: `MarkLiveTopologyDetached`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
  - `turn_at_safe_boundary`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `MarkLiveTopologyDetachedRetired`
- From: `Retired`
- On: `MarkLiveTopologyDetached`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
  - `turn_at_safe_boundary`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `MarkLiveTopologyDetachedStopped`
- From: `Stopped`
- On: `MarkLiveTopologyDetached`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
  - `turn_at_safe_boundary`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `ApplyLiveTopologyIdentityIdle`
- From: `Idle`
- On: `ApplyLiveTopologyIdentity`()
- Guards:
  - `session_registered`
  - `topology_detached`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `ApplyLiveTopologyIdentityAttached`
- From: `Attached`
- On: `ApplyLiveTopologyIdentity`()
- Guards:
  - `session_registered`
  - `topology_detached`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `ApplyLiveTopologyIdentityRunning`
- From: `Running`
- On: `ApplyLiveTopologyIdentity`()
- Guards:
  - `session_registered`
  - `topology_detached`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `ApplyLiveTopologyIdentityRetired`
- From: `Retired`
- On: `ApplyLiveTopologyIdentity`()
- Guards:
  - `session_registered`
  - `topology_detached`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `ApplyLiveTopologyIdentityStopped`
- From: `Stopped`
- On: `ApplyLiveTopologyIdentity`()
- Guards:
  - `session_registered`
  - `topology_detached`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `ApplyLiveTopologyVisibilityIdle`
- From: `Idle`
- On: `ApplyLiveTopologyVisibility`()
- Guards:
  - `session_registered`
  - `host_identity_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `ApplyLiveTopologyVisibilityAttached`
- From: `Attached`
- On: `ApplyLiveTopologyVisibility`()
- Guards:
  - `session_registered`
  - `host_identity_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `ApplyLiveTopologyVisibilityRunning`
- From: `Running`
- On: `ApplyLiveTopologyVisibility`()
- Guards:
  - `session_registered`
  - `host_identity_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `ApplyLiveTopologyVisibilityRetired`
- From: `Retired`
- On: `ApplyLiveTopologyVisibility`()
- Guards:
  - `session_registered`
  - `host_identity_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `ApplyLiveTopologyVisibilityStopped`
- From: `Stopped`
- On: `ApplyLiveTopologyVisibility`()
- Guards:
  - `session_registered`
  - `host_identity_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `CompleteLiveTopologyIdle`
- From: `Idle`
- On: `CompleteLiveTopology`()
- Guards:
  - `session_registered`
  - `host_visibility_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `CompleteLiveTopologyAttached`
- From: `Attached`
- On: `CompleteLiveTopology`()
- Guards:
  - `session_registered`
  - `host_visibility_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `CompleteLiveTopologyRunning`
- From: `Running`
- On: `CompleteLiveTopology`()
- Guards:
  - `session_registered`
  - `host_visibility_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `CompleteLiveTopologyRetired`
- From: `Retired`
- On: `CompleteLiveTopology`()
- Guards:
  - `session_registered`
  - `host_visibility_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `CompleteLiveTopologyStopped`
- From: `Stopped`
- On: `CompleteLiveTopology`()
- Guards:
  - `session_registered`
  - `host_visibility_applied`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `AbortLiveTopologyBeforeDetachIdle`
- From: `Idle`
- On: `AbortLiveTopologyBeforeDetach`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `AbortLiveTopologyBeforeDetachAttached`
- From: `Attached`
- On: `AbortLiveTopologyBeforeDetach`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `AbortLiveTopologyBeforeDetachRunning`
- From: `Running`
- On: `AbortLiveTopologyBeforeDetach`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `AbortLiveTopologyBeforeDetachRetired`
- From: `Retired`
- On: `AbortLiveTopologyBeforeDetach`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `AbortLiveTopologyBeforeDetachStopped`
- From: `Stopped`
- On: `AbortLiveTopologyBeforeDetach`()
- Guards:
  - `session_registered`
  - `topology_reconfiguring`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `FailLiveTopologyAfterDetachIdle`
- From: `Idle`
- On: `FailLiveTopologyAfterDetach`()
- Guards:
  - `session_registered`
  - `topology_past_detach`
- Emits: `LiveTopologyPhaseChanged`
- To: `Idle`

### `FailLiveTopologyAfterDetachAttached`
- From: `Attached`
- On: `FailLiveTopologyAfterDetach`()
- Guards:
  - `session_registered`
  - `topology_past_detach`
- Emits: `LiveTopologyPhaseChanged`
- To: `Attached`

### `FailLiveTopologyAfterDetachRunning`
- From: `Running`
- On: `FailLiveTopologyAfterDetach`()
- Guards:
  - `session_registered`
  - `topology_past_detach`
- Emits: `LiveTopologyPhaseChanged`
- To: `Running`

### `FailLiveTopologyAfterDetachRetired`
- From: `Retired`
- On: `FailLiveTopologyAfterDetach`()
- Guards:
  - `session_registered`
  - `topology_past_detach`
- Emits: `LiveTopologyPhaseChanged`
- To: `Retired`

### `FailLiveTopologyAfterDetachStopped`
- From: `Stopped`
- On: `FailLiveTopologyAfterDetach`()
- Guards:
  - `session_registered`
  - `topology_past_detach`
- Emits: `LiveTopologyPhaseChanged`
- To: `Stopped`

### `McpServerConnectPendingIdle`
- From: `Idle`
- On: `McpServerConnectPending`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Idle`

### `McpServerConnectPendingAttached`
- From: `Attached`
- On: `McpServerConnectPending`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Attached`

### `McpServerConnectPendingRunning`
- From: `Running`
- On: `McpServerConnectPending`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Running`

### `McpServerConnectPendingRetired`
- From: `Retired`
- On: `McpServerConnectPending`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Retired`

### `McpServerConnectPendingStopped`
- From: `Stopped`
- On: `McpServerConnectPending`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Stopped`

### `McpServerConnectedIdle`
- From: `Idle`
- On: `McpServerConnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Idle`

### `McpServerConnectedAttached`
- From: `Attached`
- On: `McpServerConnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Attached`

### `McpServerConnectedRunning`
- From: `Running`
- On: `McpServerConnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Running`

### `McpServerConnectedRetired`
- From: `Retired`
- On: `McpServerConnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Retired`

### `McpServerConnectedStopped`
- From: `Stopped`
- On: `McpServerConnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Stopped`

### `McpServerFailedIdle`
- From: `Idle`
- On: `McpServerFailed`(server_id, error)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Idle`

### `McpServerFailedAttached`
- From: `Attached`
- On: `McpServerFailed`(server_id, error)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Attached`

### `McpServerFailedRunning`
- From: `Running`
- On: `McpServerFailed`(server_id, error)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Running`

### `McpServerFailedRetired`
- From: `Retired`
- On: `McpServerFailed`(server_id, error)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Retired`

### `McpServerFailedStopped`
- From: `Stopped`
- On: `McpServerFailed`(server_id, error)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Stopped`

### `McpServerDisconnectedIdle`
- From: `Idle`
- On: `McpServerDisconnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Idle`

### `McpServerDisconnectedAttached`
- From: `Attached`
- On: `McpServerDisconnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Attached`

### `McpServerDisconnectedRunning`
- From: `Running`
- On: `McpServerDisconnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Running`

### `McpServerDisconnectedRetired`
- From: `Retired`
- On: `McpServerDisconnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Retired`

### `McpServerDisconnectedStopped`
- From: `Stopped`
- On: `McpServerDisconnected`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerStateChanged`
- To: `Stopped`

### `McpServerReloadIdle`
- From: `Idle`
- On: `McpServerReload`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerReloadRequested`, `McpServerStateChanged`
- To: `Idle`

### `McpServerReloadAttached`
- From: `Attached`
- On: `McpServerReload`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerReloadRequested`, `McpServerStateChanged`
- To: `Attached`

### `McpServerReloadRunning`
- From: `Running`
- On: `McpServerReload`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerReloadRequested`, `McpServerStateChanged`
- To: `Running`

### `McpServerReloadRetired`
- From: `Retired`
- On: `McpServerReload`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerReloadRequested`, `McpServerStateChanged`
- To: `Retired`

### `McpServerReloadStopped`
- From: `Stopped`
- On: `McpServerReload`(server_id)
- Guards:
  - `session_registered`
- Emits: `McpServerReloadRequested`, `McpServerStateChanged`
- To: `Stopped`

### `PeerRequestSentIdle`
- From: `Idle`
- On: `PeerRequestSent`(corr_id, to)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Idle`

### `PeerRequestSentAttached`
- From: `Attached`
- On: `PeerRequestSent`(corr_id, to)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Attached`

### `PeerRequestSentRunning`
- From: `Running`
- On: `PeerRequestSent`(corr_id, to)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Running`

### `PeerRequestSentRetired`
- From: `Retired`
- On: `PeerRequestSent`(corr_id, to)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Retired`

### `PeerRequestSentStopped`
- From: `Stopped`
- On: `PeerRequestSent`(corr_id, to)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Stopped`

### `PeerResponseProgressArrivedIdle`
- From: `Idle`
- On: `PeerResponseProgressArrived`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`
- To: `Idle`

### `PeerResponseProgressArrivedAttached`
- From: `Attached`
- On: `PeerResponseProgressArrived`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`
- To: `Attached`

### `PeerResponseProgressArrivedRunning`
- From: `Running`
- On: `PeerResponseProgressArrived`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`
- To: `Running`

### `PeerResponseProgressArrivedRetired`
- From: `Retired`
- On: `PeerResponseProgressArrived`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`
- To: `Retired`

### `PeerResponseProgressArrivedStopped`
- From: `Stopped`
- On: `PeerResponseProgressArrived`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`
- To: `Stopped`

### `PeerResponseTerminalArrivedCompletedIdle`
- From: `Idle`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `completed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Idle`

### `PeerResponseTerminalArrivedCompletedAttached`
- From: `Attached`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `completed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Attached`

### `PeerResponseTerminalArrivedCompletedRunning`
- From: `Running`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `completed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Running`

### `PeerResponseTerminalArrivedCompletedRetired`
- From: `Retired`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `completed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Retired`

### `PeerResponseTerminalArrivedCompletedStopped`
- From: `Stopped`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `completed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Stopped`

### `PeerResponseTerminalArrivedFailedIdle`
- From: `Idle`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `failed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Idle`

### `PeerResponseTerminalArrivedFailedAttached`
- From: `Attached`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `failed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Attached`

### `PeerResponseTerminalArrivedFailedRunning`
- From: `Running`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `failed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Running`

### `PeerResponseTerminalArrivedFailedRetired`
- From: `Retired`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `failed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Retired`

### `PeerResponseTerminalArrivedFailedStopped`
- From: `Stopped`
- On: `PeerResponseTerminalArrived`(corr_id, disposition)
- Guards:
  - `pending_exists`
  - `failed`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Stopped`

### `PeerRequestTimedOutIdle`
- From: `Idle`
- On: `PeerRequestTimedOut`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Idle`

### `PeerRequestTimedOutAttached`
- From: `Attached`
- On: `PeerRequestTimedOut`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Attached`

### `PeerRequestTimedOutRunning`
- From: `Running`
- On: `PeerRequestTimedOut`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Running`

### `PeerRequestTimedOutRetired`
- From: `Retired`
- On: `PeerRequestTimedOut`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Retired`

### `PeerRequestTimedOutStopped`
- From: `Stopped`
- On: `PeerRequestTimedOut`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Stopped`

### `PeerRequestReceivedIdle`
- From: `Idle`
- On: `PeerRequestReceived`(corr_id)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Idle`

### `PeerRequestReceivedAttached`
- From: `Attached`
- On: `PeerRequestReceived`(corr_id)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Attached`

### `PeerRequestReceivedRunning`
- From: `Running`
- On: `PeerRequestReceived`(corr_id)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Running`

### `PeerRequestReceivedRetired`
- From: `Retired`
- On: `PeerRequestReceived`(corr_id)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Retired`

### `PeerRequestReceivedStopped`
- From: `Stopped`
- On: `PeerRequestReceived`(corr_id)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Stopped`

### `PeerResponseRepliedIdle`
- From: `Idle`
- On: `PeerResponseReplied`(corr_id)
- Guards:
  - `inbound_exists`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Idle`

### `PeerResponseRepliedAttached`
- From: `Attached`
- On: `PeerResponseReplied`(corr_id)
- Guards:
  - `inbound_exists`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Attached`

### `PeerResponseRepliedRunning`
- From: `Running`
- On: `PeerResponseReplied`(corr_id)
- Guards:
  - `inbound_exists`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Running`

### `PeerResponseRepliedRetired`
- From: `Retired`
- On: `PeerResponseReplied`(corr_id)
- Guards:
  - `inbound_exists`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Retired`

### `PeerResponseRepliedStopped`
- From: `Stopped`
- On: `PeerResponseReplied`(corr_id)
- Guards:
  - `inbound_exists`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Stopped`

### `AdvanceSessionContextIdle`
- From: `Idle`
- On: `AdvanceSessionContext`(updated_at_ms)
- Guards:
  - `monotonic`
- Emits: `SessionContextAdvanced`
- To: `Idle`

### `AdvanceSessionContextAttached`
- From: `Attached`
- On: `AdvanceSessionContext`(updated_at_ms)
- Guards:
  - `monotonic`
- Emits: `SessionContextAdvanced`
- To: `Attached`

### `AdvanceSessionContextRunning`
- From: `Running`
- On: `AdvanceSessionContext`(updated_at_ms)
- Guards:
  - `monotonic`
- Emits: `SessionContextAdvanced`
- To: `Running`

### `AdvanceSessionContextRetired`
- From: `Retired`
- On: `AdvanceSessionContext`(updated_at_ms)
- Guards:
  - `monotonic`
- Emits: `SessionContextAdvanced`
- To: `Retired`

### `AdvanceSessionContextStopped`
- From: `Stopped`
- On: `AdvanceSessionContext`(updated_at_ms)
- Guards:
  - `monotonic`
- Emits: `SessionContextAdvanced`
- To: `Stopped`

### `InteractionStreamReservedIdle`
- From: `Idle`
- On: `InteractionStreamReserved`(corr_id)
- Guards:
  - `not_reserved`
  - `not_attached`
- Emits: `InteractionStreamStateChanged`
- To: `Idle`

### `InteractionStreamReservedAttached`
- From: `Attached`
- On: `InteractionStreamReserved`(corr_id)
- Guards:
  - `not_reserved`
  - `not_attached`
- Emits: `InteractionStreamStateChanged`
- To: `Attached`

### `InteractionStreamReservedRunning`
- From: `Running`
- On: `InteractionStreamReserved`(corr_id)
- Guards:
  - `not_reserved`
  - `not_attached`
- Emits: `InteractionStreamStateChanged`
- To: `Running`

### `InteractionStreamReservedRetired`
- From: `Retired`
- On: `InteractionStreamReserved`(corr_id)
- Guards:
  - `not_reserved`
  - `not_attached`
- Emits: `InteractionStreamStateChanged`
- To: `Retired`

### `InteractionStreamReservedStopped`
- From: `Stopped`
- On: `InteractionStreamReserved`(corr_id)
- Guards:
  - `not_reserved`
  - `not_attached`
- Emits: `InteractionStreamStateChanged`
- To: `Stopped`

### `InteractionStreamAttachedIdle`
- From: `Idle`
- On: `InteractionStreamAttached`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`
- To: `Idle`

### `InteractionStreamAttachedAttached`
- From: `Attached`
- On: `InteractionStreamAttached`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`
- To: `Attached`

### `InteractionStreamAttachedRunning`
- From: `Running`
- On: `InteractionStreamAttached`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`
- To: `Running`

### `InteractionStreamAttachedRetired`
- From: `Retired`
- On: `InteractionStreamAttached`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`
- To: `Retired`

### `InteractionStreamAttachedStopped`
- From: `Stopped`
- On: `InteractionStreamAttached`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`
- To: `Stopped`

### `InteractionStreamCompletedIdle`
- From: `Idle`
- On: `InteractionStreamCompleted`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Idle`

### `InteractionStreamCompletedAttached`
- From: `Attached`
- On: `InteractionStreamCompleted`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Attached`

### `InteractionStreamCompletedRunning`
- From: `Running`
- On: `InteractionStreamCompleted`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Running`

### `InteractionStreamCompletedRetired`
- From: `Retired`
- On: `InteractionStreamCompleted`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Retired`

### `InteractionStreamCompletedStopped`
- From: `Stopped`
- On: `InteractionStreamCompleted`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Stopped`

### `InteractionStreamExpiredIdle`
- From: `Idle`
- On: `InteractionStreamExpired`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Idle`

### `InteractionStreamExpiredAttached`
- From: `Attached`
- On: `InteractionStreamExpired`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Attached`

### `InteractionStreamExpiredRunning`
- From: `Running`
- On: `InteractionStreamExpired`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Running`

### `InteractionStreamExpiredRetired`
- From: `Retired`
- On: `InteractionStreamExpired`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Retired`

### `InteractionStreamExpiredStopped`
- From: `Stopped`
- On: `InteractionStreamExpired`(corr_id)
- Guards:
  - `is_reserved`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Stopped`

### `InteractionStreamClosedEarlyIdle`
- From: `Idle`
- On: `InteractionStreamClosedEarly`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Idle`

### `InteractionStreamClosedEarlyAttached`
- From: `Attached`
- On: `InteractionStreamClosedEarly`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Attached`

### `InteractionStreamClosedEarlyRunning`
- From: `Running`
- On: `InteractionStreamClosedEarly`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Running`

### `InteractionStreamClosedEarlyRetired`
- From: `Retired`
- On: `InteractionStreamClosedEarly`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Retired`

### `InteractionStreamClosedEarlyStopped`
- From: `Stopped`
- On: `InteractionStreamClosedEarly`(corr_id)
- Guards:
  - `is_attached`
- Emits: `InteractionStreamStateChanged`, `InteractionStreamCleanup`
- To: `Stopped`

### `ProductTurnInFlightInitializing`
- From: `Initializing`
- On: `ProductTurnInFlight`()
- Guards:
  - `only_from_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductTurnInFlightIdle`
- From: `Idle`
- On: `ProductTurnInFlight`()
- Guards:
  - `only_from_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductTurnInFlightAttached`
- From: `Attached`
- On: `ProductTurnInFlight`()
- Guards:
  - `only_from_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductTurnInFlightRunning`
- From: `Running`
- On: `ProductTurnInFlight`()
- Guards:
  - `only_from_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductTurnInFlightRetired`
- From: `Retired`
- On: `ProductTurnInFlight`()
- Guards:
  - `only_from_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductTurnInFlightStopped`
- From: `Stopped`
- On: `ProductTurnInFlight`()
- Guards:
  - `only_from_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductTurnCommittedFromAwaitingInitializing`
- From: `Initializing`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductTurnCommittedFromAwaitingIdle`
- From: `Idle`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductTurnCommittedFromAwaitingAttached`
- From: `Attached`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductTurnCommittedFromAwaitingRunning`
- From: `Running`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductTurnCommittedFromAwaitingRetired`
- From: `Retired`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductTurnCommittedFromAwaitingStopped`
- From: `Stopped`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductTurnCommittedFromOutputInitializing`
- From: `Initializing`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductTurnCommittedFromOutputIdle`
- From: `Idle`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductTurnCommittedFromOutputAttached`
- From: `Attached`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductTurnCommittedFromOutputRunning`
- From: `Running`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductTurnCommittedFromOutputRetired`
- From: `Retired`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductTurnCommittedFromOutputStopped`
- From: `Stopped`
- On: `ProductTurnCommitted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductOutputStartedFromAwaitingInitializing`
- From: `Initializing`
- On: `ProductOutputStarted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductOutputStartedFromAwaitingIdle`
- From: `Idle`
- On: `ProductOutputStarted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductOutputStartedFromAwaitingAttached`
- From: `Attached`
- On: `ProductOutputStarted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductOutputStartedFromAwaitingRunning`
- From: `Running`
- On: `ProductOutputStarted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductOutputStartedFromAwaitingRetired`
- From: `Retired`
- On: `ProductOutputStarted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductOutputStartedFromAwaitingStopped`
- From: `Stopped`
- On: `ProductOutputStarted`()
- Guards:
  - `from_awaiting`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductOutputStartedFromCommittedInitializing`
- From: `Initializing`
- On: `ProductOutputStarted`()
- Guards:
  - `from_committed`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductOutputStartedFromCommittedIdle`
- From: `Idle`
- On: `ProductOutputStarted`()
- Guards:
  - `from_committed`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductOutputStartedFromCommittedAttached`
- From: `Attached`
- On: `ProductOutputStarted`()
- Guards:
  - `from_committed`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductOutputStartedFromCommittedRunning`
- From: `Running`
- On: `ProductOutputStarted`()
- Guards:
  - `from_committed`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductOutputStartedFromCommittedRetired`
- From: `Retired`
- On: `ProductOutputStarted`()
- Guards:
  - `from_committed`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductOutputStartedFromCommittedStopped`
- From: `Stopped`
- On: `ProductOutputStarted`()
- Guards:
  - `from_committed`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductTurnInterruptedFromPreemptibleInitializing`
- From: `Initializing`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_preemptible`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductTurnInterruptedFromPreemptibleIdle`
- From: `Idle`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_preemptible`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductTurnInterruptedFromPreemptibleAttached`
- From: `Attached`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_preemptible`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductTurnInterruptedFromPreemptibleRunning`
- From: `Running`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_preemptible`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductTurnInterruptedFromPreemptibleRetired`
- From: `Retired`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_preemptible`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductTurnInterruptedFromPreemptibleStopped`
- From: `Stopped`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_preemptible`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductTurnInterruptedFromOutputInitializing`
- From: `Initializing`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductTurnInterruptedFromOutputIdle`
- From: `Idle`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductTurnInterruptedFromOutputAttached`
- From: `Attached`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductTurnInterruptedFromOutputRunning`
- From: `Running`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductTurnInterruptedFromOutputRetired`
- From: `Retired`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductTurnInterruptedFromOutputStopped`
- From: `Stopped`
- On: `ProductTurnInterrupted`()
- Guards:
  - `from_output_started`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `ProductTurnTerminalInitializing`
- From: `Initializing`
- On: `ProductTurnTerminal`()
- Guards:
  - `not_already_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Initializing`

### `ProductTurnTerminalIdle`
- From: `Idle`
- On: `ProductTurnTerminal`()
- Guards:
  - `not_already_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Idle`

### `ProductTurnTerminalAttached`
- From: `Attached`
- On: `ProductTurnTerminal`()
- Guards:
  - `not_already_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Attached`

### `ProductTurnTerminalRunning`
- From: `Running`
- On: `ProductTurnTerminal`()
- Guards:
  - `not_already_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Running`

### `ProductTurnTerminalRetired`
- From: `Retired`
- On: `ProductTurnTerminal`()
- Guards:
  - `not_already_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Retired`

### `ProductTurnTerminalStopped`
- From: `Stopped`
- On: `ProductTurnTerminal`()
- Guards:
  - `not_already_idle`
- Emits: `RealtimeProductTurnPhaseChanged`
- To: `Stopped`

### `RealtimeProjectionAdvanceDuringTurnInitializing`
- From: `Initializing`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_in_flight`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Initializing`

### `RealtimeProjectionAdvanceDuringTurnIdle`
- From: `Idle`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_in_flight`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Idle`

### `RealtimeProjectionAdvanceDuringTurnAttached`
- From: `Attached`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_in_flight`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Attached`

### `RealtimeProjectionAdvanceDuringTurnRunning`
- From: `Running`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_in_flight`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Running`

### `RealtimeProjectionAdvanceDuringTurnRetired`
- From: `Retired`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_in_flight`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Retired`

### `RealtimeProjectionAdvanceDuringTurnStopped`
- From: `Stopped`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_in_flight`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Stopped`

### `RealtimeProjectionAdvanceWhileIdleInitializing`
- From: `Initializing`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_idle`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Initializing`

### `RealtimeProjectionAdvanceWhileIdleIdle`
- From: `Idle`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_idle`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Idle`

### `RealtimeProjectionAdvanceWhileIdleAttached`
- From: `Attached`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_idle`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Attached`

### `RealtimeProjectionAdvanceWhileIdleRunning`
- From: `Running`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_idle`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Running`

### `RealtimeProjectionAdvanceWhileIdleRetired`
- From: `Retired`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_idle`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Retired`

### `RealtimeProjectionAdvanceWhileIdleStopped`
- From: `Stopped`
- On: `RealtimeProjectionAdvanceObserved`(advanced_at_ms)
- Guards:
  - `monotonic`
  - `turn_idle`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Stopped`

### `RealtimeProjectionRefreshedInitializing`
- From: `Initializing`
- On: `RealtimeProjectionRefreshed`(observed_ms)
- Guards:
  - `not_behind_frontier`
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Initializing`

### `RealtimeProjectionRefreshedIdle`
- From: `Idle`
- On: `RealtimeProjectionRefreshed`(observed_ms)
- Guards:
  - `not_behind_frontier`
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Idle`

### `RealtimeProjectionRefreshedAttached`
- From: `Attached`
- On: `RealtimeProjectionRefreshed`(observed_ms)
- Guards:
  - `not_behind_frontier`
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Attached`

### `RealtimeProjectionRefreshedRunning`
- From: `Running`
- On: `RealtimeProjectionRefreshed`(observed_ms)
- Guards:
  - `not_behind_frontier`
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Running`

### `RealtimeProjectionRefreshedRetired`
- From: `Retired`
- On: `RealtimeProjectionRefreshed`(observed_ms)
- Guards:
  - `not_behind_frontier`
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Retired`

### `RealtimeProjectionRefreshedStopped`
- From: `Stopped`
- On: `RealtimeProjectionRefreshed`(observed_ms)
- Guards:
  - `not_behind_frontier`
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Stopped`

### `RealtimeProjectionResetInitializing`
- From: `Initializing`
- On: `RealtimeProjectionReset`(baseline_ms)
- Guards:
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Initializing`

### `RealtimeProjectionResetIdle`
- From: `Idle`
- On: `RealtimeProjectionReset`(baseline_ms)
- Guards:
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Idle`

### `RealtimeProjectionResetAttached`
- From: `Attached`
- On: `RealtimeProjectionReset`(baseline_ms)
- Guards:
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Attached`

### `RealtimeProjectionResetRunning`
- From: `Running`
- On: `RealtimeProjectionReset`(baseline_ms)
- Guards:
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Running`

### `RealtimeProjectionResetRetired`
- From: `Retired`
- On: `RealtimeProjectionReset`(baseline_ms)
- Guards:
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Retired`

### `RealtimeProjectionResetStopped`
- From: `Stopped`
- On: `RealtimeProjectionReset`(baseline_ms)
- Guards:
  - `actually_changing`
- Emits: `RealtimeProjectionFreshnessChanged`
- To: `Stopped`

### `ClassifyRealtimeClientInputSubmittedInitializing`
- From: `Initializing`
- On: `ClassifyRealtimeClientInputSubmitted`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Initializing`

### `ClassifyRealtimeClientInputSubmittedIdle`
- From: `Idle`
- On: `ClassifyRealtimeClientInputSubmitted`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Idle`

### `ClassifyRealtimeClientInputSubmittedAttached`
- From: `Attached`
- On: `ClassifyRealtimeClientInputSubmitted`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Attached`

### `ClassifyRealtimeClientInputSubmittedRunning`
- From: `Running`
- On: `ClassifyRealtimeClientInputSubmitted`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Running`

### `ClassifyRealtimeClientInputSubmittedRetired`
- From: `Retired`
- On: `ClassifyRealtimeClientInputSubmitted`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Retired`

### `ClassifyRealtimeClientInputSubmittedStopped`
- From: `Stopped`
- On: `ClassifyRealtimeClientInputSubmitted`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Stopped`

### `ClassifyRealtimeMidTurnActivityInitializing`
- From: `Initializing`
- On: `ClassifyRealtimeMidTurnActivity`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Initializing`

### `ClassifyRealtimeMidTurnActivityIdle`
- From: `Idle`
- On: `ClassifyRealtimeMidTurnActivity`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Idle`

### `ClassifyRealtimeMidTurnActivityAttached`
- From: `Attached`
- On: `ClassifyRealtimeMidTurnActivity`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Attached`

### `ClassifyRealtimeMidTurnActivityRunning`
- From: `Running`
- On: `ClassifyRealtimeMidTurnActivity`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Running`

### `ClassifyRealtimeMidTurnActivityRetired`
- From: `Retired`
- On: `ClassifyRealtimeMidTurnActivity`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Retired`

### `ClassifyRealtimeMidTurnActivityStopped`
- From: `Stopped`
- On: `ClassifyRealtimeMidTurnActivity`()
- Guards:
  - `not_already_reattach`
- Emits: `RealtimeReconnectPolicyChanged`
- To: `Stopped`

### `ClassifyRealtimeTurnTerminatedInitializing`
- From: `Initializing`
- On: `ClassifyRealtimeTurnTerminated`()
- Guards:
  - `actually_changing`
- Emits: `RealtimeReconnectPolicyChanged`, `RealtimeProjectionFreshnessChanged`
- To: `Initializing`

### `ClassifyRealtimeTurnTerminatedIdle`
- From: `Idle`
- On: `ClassifyRealtimeTurnTerminated`()
- Guards:
  - `actually_changing`
- Emits: `RealtimeReconnectPolicyChanged`, `RealtimeProjectionFreshnessChanged`
- To: `Idle`

### `ClassifyRealtimeTurnTerminatedAttached`
- From: `Attached`
- On: `ClassifyRealtimeTurnTerminated`()
- Guards:
  - `actually_changing`
- Emits: `RealtimeReconnectPolicyChanged`, `RealtimeProjectionFreshnessChanged`
- To: `Attached`

### `ClassifyRealtimeTurnTerminatedRunning`
- From: `Running`
- On: `ClassifyRealtimeTurnTerminated`()
- Guards:
  - `actually_changing`
- Emits: `RealtimeReconnectPolicyChanged`, `RealtimeProjectionFreshnessChanged`
- To: `Running`

### `ClassifyRealtimeTurnTerminatedRetired`
- From: `Retired`
- On: `ClassifyRealtimeTurnTerminated`()
- Guards:
  - `actually_changing`
- Emits: `RealtimeReconnectPolicyChanged`, `RealtimeProjectionFreshnessChanged`
- To: `Retired`

### `ClassifyRealtimeTurnTerminatedStopped`
- From: `Stopped`
- On: `ClassifyRealtimeTurnTerminated`()
- Guards:
  - `actually_changing`
- Emits: `RealtimeReconnectPolicyChanged`, `RealtimeProjectionFreshnessChanged`
- To: `Stopped`

### `AttachSessionIngressIdle`
- From: `Idle`
- On: `AttachSessionIngress`(comms_runtime_id)
- Guards:
  - `session_registered`
  - `owner_allows_session_attach`
- To: `Idle`

### `AttachSessionIngressAttached`
- From: `Attached`
- On: `AttachSessionIngress`(comms_runtime_id)
- Guards:
  - `session_registered`
  - `owner_allows_session_attach`
- To: `Attached`

### `AttachSessionIngressRunning`
- From: `Running`
- On: `AttachSessionIngress`(comms_runtime_id)
- Guards:
  - `session_registered`
  - `owner_allows_session_attach`
- To: `Running`

### `AttachSessionIngressRetired`
- From: `Retired`
- On: `AttachSessionIngress`(comms_runtime_id)
- Guards:
  - `session_registered`
  - `owner_allows_session_attach`
- To: `Retired`

### `AttachSessionIngressStopped`
- From: `Stopped`
- On: `AttachSessionIngress`(comms_runtime_id)
- Guards:
  - `session_registered`
  - `owner_allows_session_attach`
- To: `Stopped`

### `AttachMobIngressIdle`
- From: `Idle`
- On: `AttachMobIngress`(comms_runtime_id, mob_id)
- Guards:
  - `session_registered`
  - `owner_allows_mob_attach`
- To: `Idle`

### `AttachMobIngressAttached`
- From: `Attached`
- On: `AttachMobIngress`(comms_runtime_id, mob_id)
- Guards:
  - `session_registered`
  - `owner_allows_mob_attach`
- To: `Attached`

### `AttachMobIngressRunning`
- From: `Running`
- On: `AttachMobIngress`(comms_runtime_id, mob_id)
- Guards:
  - `session_registered`
  - `owner_allows_mob_attach`
- To: `Running`

### `AttachMobIngressRetired`
- From: `Retired`
- On: `AttachMobIngress`(comms_runtime_id, mob_id)
- Guards:
  - `session_registered`
  - `owner_allows_mob_attach`
- To: `Retired`

### `AttachMobIngressStopped`
- From: `Stopped`
- On: `AttachMobIngress`(comms_runtime_id, mob_id)
- Guards:
  - `session_registered`
  - `owner_allows_mob_attach`
- To: `Stopped`

### `DetachIngressIdle`
- From: `Idle`
- On: `DetachIngress`()
- Guards:
  - `session_registered`
- To: `Idle`

### `DetachIngressAttached`
- From: `Attached`
- On: `DetachIngress`()
- Guards:
  - `session_registered`
- To: `Attached`

### `DetachIngressRunning`
- From: `Running`
- On: `DetachIngress`()
- Guards:
  - `session_registered`
- To: `Running`

### `DetachIngressRetired`
- From: `Retired`
- On: `DetachIngress`()
- Guards:
  - `session_registered`
- To: `Retired`

### `DetachIngressStopped`
- From: `Stopped`
- On: `DetachIngress`()
- Guards:
  - `session_registered`
- To: `Stopped`

### `BindSupervisorIdle`
- From: `Idle`
- On: `BindSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_unbound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Idle`

### `BindSupervisorAttached`
- From: `Attached`
- On: `BindSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_unbound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Attached`

### `BindSupervisorRunning`
- From: `Running`
- On: `BindSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_unbound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Running`

### `BindSupervisorRetired`
- From: `Retired`
- On: `BindSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_unbound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Retired`

### `BindSupervisorStopped`
- From: `Stopped`
- On: `BindSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_unbound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Stopped`

### `AuthorizeSupervisorIdle`
- From: `Idle`
- On: `AuthorizeSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_bound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Idle`

### `AuthorizeSupervisorAttached`
- From: `Attached`
- On: `AuthorizeSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_bound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Attached`

### `AuthorizeSupervisorRunning`
- From: `Running`
- On: `AuthorizeSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_bound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Running`

### `AuthorizeSupervisorRetired`
- From: `Retired`
- On: `AuthorizeSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_bound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Retired`

### `AuthorizeSupervisorStopped`
- From: `Stopped`
- On: `AuthorizeSupervisor`(name, peer_id, address, epoch)
- Guards:
  - `supervisor_bound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Stopped`

### `RevokeSupervisorIdle`
- From: `Idle`
- On: `RevokeSupervisor`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- Emits: `RevokeSupervisorTrustEdge`
- To: `Idle`

### `RevokeSupervisorAttached`
- From: `Attached`
- On: `RevokeSupervisor`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- Emits: `RevokeSupervisorTrustEdge`
- To: `Attached`

### `RevokeSupervisorRunning`
- From: `Running`
- On: `RevokeSupervisor`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- Emits: `RevokeSupervisorTrustEdge`
- To: `Running`

### `RevokeSupervisorRetired`
- From: `Retired`
- On: `RevokeSupervisor`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- Emits: `RevokeSupervisorTrustEdge`
- To: `Retired`

### `RevokeSupervisorStopped`
- From: `Stopped`
- On: `RevokeSupervisor`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- Emits: `RevokeSupervisorTrustEdge`
- To: `Stopped`

### `SupervisorTrustEdgePublishedIdle`
- From: `Idle`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Idle`

### `SupervisorTrustEdgePublishedAttached`
- From: `Attached`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Attached`

### `SupervisorTrustEdgePublishedRunning`
- From: `Running`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Running`

### `SupervisorTrustEdgePublishedRetired`
- From: `Retired`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Retired`

### `SupervisorTrustEdgePublishedStopped`
- From: `Stopped`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Stopped`

### `SupervisorTrustEdgePublishFailedIdle`
- From: `Idle`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Idle`

### `SupervisorTrustEdgePublishFailedAttached`
- From: `Attached`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Attached`

### `SupervisorTrustEdgePublishFailedRunning`
- From: `Running`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Running`

### `SupervisorTrustEdgePublishFailedRetired`
- From: `Retired`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Retired`

### `SupervisorTrustEdgePublishFailedStopped`
- From: `Stopped`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Stopped`

### `SupervisorTrustEdgeRevokedIdle`
- From: `Idle`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Idle`

### `SupervisorTrustEdgeRevokedAttached`
- From: `Attached`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Attached`

### `SupervisorTrustEdgeRevokedRunning`
- From: `Running`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Running`

### `SupervisorTrustEdgeRevokedRetired`
- From: `Retired`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Retired`

### `SupervisorTrustEdgeRevokedStopped`
- From: `Stopped`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Stopped`

### `SupervisorTrustEdgeRevokeFailedIdle`
- From: `Idle`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Idle`

### `SupervisorTrustEdgeRevokeFailedAttached`
- From: `Attached`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Attached`

### `SupervisorTrustEdgeRevokeFailedRunning`
- From: `Running`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Running`

### `SupervisorTrustEdgeRevokeFailedRetired`
- From: `Retired`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Retired`

### `SupervisorTrustEdgeRevokeFailedStopped`
- From: `Stopped`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
- To: `Stopped`

### `PublishLocalEndpointIdle`
- From: `Idle`
- On: `PublishLocalEndpoint`(endpoint)
- Emits: `LocalEndpointChanged`
- To: `Idle`

### `PublishLocalEndpointAttached`
- From: `Attached`
- On: `PublishLocalEndpoint`(endpoint)
- Emits: `LocalEndpointChanged`
- To: `Attached`

### `PublishLocalEndpointRunning`
- From: `Running`
- On: `PublishLocalEndpoint`(endpoint)
- Emits: `LocalEndpointChanged`
- To: `Running`

### `ClearLocalEndpointIdle`
- From: `Idle`
- On: `ClearLocalEndpoint`()
- Guards:
  - `local_endpoint_present`
- Emits: `LocalEndpointChanged`
- To: `Idle`

### `ClearLocalEndpointAttached`
- From: `Attached`
- On: `ClearLocalEndpoint`()
- Guards:
  - `local_endpoint_present`
- Emits: `LocalEndpointChanged`
- To: `Attached`

### `ClearLocalEndpointRunning`
- From: `Running`
- On: `ClearLocalEndpoint`()
- Guards:
  - `local_endpoint_present`
- Emits: `LocalEndpointChanged`
- To: `Running`

### `AddDirectPeerEndpointIdle`
- From: `Idle`
- On: `AddDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_not_already_direct`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Idle`

### `AddDirectPeerEndpointAttached`
- From: `Attached`
- On: `AddDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_not_already_direct`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Attached`

### `AddDirectPeerEndpointRunning`
- From: `Running`
- On: `AddDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_not_already_direct`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Running`

### `RemoveDirectPeerEndpointIdle`
- From: `Idle`
- On: `RemoveDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_present_in_direct`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Idle`

### `RemoveDirectPeerEndpointAttached`
- From: `Attached`
- On: `RemoveDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_present_in_direct`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Attached`

### `RemoveDirectPeerEndpointRunning`
- From: `Running`
- On: `RemoveDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_present_in_direct`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Running`

### `ApplyMobPeerOverlayIdle`
- From: `Idle`
- On: `ApplyMobPeerOverlay`(epoch, endpoints)
- Guards:
  - `stale_overlay_epoch`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Idle`

### `ApplyMobPeerOverlayAttached`
- From: `Attached`
- On: `ApplyMobPeerOverlay`(epoch, endpoints)
- Guards:
  - `stale_overlay_epoch`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Attached`

### `ApplyMobPeerOverlayRunning`
- From: `Running`
- On: `ApplyMobPeerOverlay`(epoch, endpoints)
- Guards:
  - `stale_overlay_epoch`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Running`

## Coverage
### Code Anchors
- `meerkat-runtime/src/meerkat_machine/mod.rs` — authoritative MeerkatMachine command dispatch and state ownership for initialize, recover initializing, register, unregister, reconfigure, stage filters and tools, prepare bindings, drain, interrupt, cancel boundary, cancellation, abort, wait, ingest, publish event, accept input, recover input lifecycle, classify envelope, append/context starts, run preparation, primitive applied conversation/immediate, enter extraction, extraction validation passed/failed retry/exhausted, recoverable/fatal failure, retry requested, budget exhausted, steer accepted, increment attempt count, rollback staged, consume on accept, commit, fail, pending/call/finalize tool surface, retire/retired, reset, stop/stopped executor, destroy/destroyed, ensure executor, runtime notice, silent intents, recycle, realtime binding, MCP server, interaction stream, product turn, live topology, ingress, supervisor, trust reconcile, ops barrier, local endpoint, admission, completion, compaction, submit op event, progress reported op, terminate op, notify op watcher, collect/enqueue, terminal records, model routing status, set model routing baseline, finite switch turn, until changed switch turn, assistant turn admission, image operation begin activate complete restore, routing approval, routing denial, scoped override, sync visibility revisions, and persistent reconfigure
- `meerkat/src/meerkat_machine.rs` — MeerkatMachine snapshot/diagnostic facade
- `meerkat-comms/src/peer_directory_reachability_authority.rs` — peer directory reachability state now owned as a MeerkatMachine-internal region

### Scenarios
- `bind-run-boundary-terminal` — runtime binds, runs work, applies a boundary, and reports a terminal outcome
- `retire-reset-destroy` — runtime retires, resets, stops, and destroys without reopening superseded work
- `staged_visibility_apply` — tool visibility staged state promotes into the committed visible revision at a boundary
- `turn_interrupt_and_shutdown` — running work records interrupt and shutdown intent without escaping the Meerkat authority boundary
- `peer_reachability_probe` — resolved peer directory updates and send outcomes mutate Meerkat-owned peer reachability state
- `session_registration_and_binding` — initialize, recover initializing, register, unregister, reconfigure session identity, prepare bindings, ensure executor, attach session ingress, detach ingress, drain exit, and runtime bound/retired/destroyed notices
- `input_admission_and_queueing` — ingest and publish event, accept input with or without completion, classify external envelope or plain event, prepare run work, primitive applied conversation or immediate, enter extraction, extraction validation passed, recoverable or fatal failure, budget exhausted, steer accepted, increment attempt count, consume on accept, enqueue classified entry, resolve admission, submit admitted ingress effect, post admission signal, and input or ingress notices
- `ops_completion_and_waiters` — abort, wait, abort all, request cancellation at boundary, completion produced/resolved, wait all satisfied, collect completed result, submit op event, notify op watcher, reject surface call, retain or evict completed terminal records
- `realtime_connection_projection` — project realtime intent, begin replace detach binding, require reattach, publish signal, reconnect progress, MCP server connect/connected/failed/disconnected/reload, advance session context, interaction stream reserved/attached/completed/expired/closed early, freshness, policy, and binding rotation
- `product_turn_streaming` — product turn in flight, committed, output started, interrupted, terminal, realtime projection advance/refreshed/reset, client input submitted, mid turn activity, and turn terminated classification
- `recycle_and_compaction` — recycle from idle or retired, initiate recycle, check compaction, and re-enter ready runtime ownership without preserving stale completed records
- `model_routing_and_image_operation` — set model routing baseline, request finite switch turn, request until changed switch turn, admit model routing assistant turn, begin image operation, activate image operation override, complete image operation, restore image operation override, project model routing status changed, switch turn denied, switch turn persistent reconfigure requested, switch turn finite override activated/restored, image operation phase changed/denied, and model routing approval terminalized
- `live_topology_and_supervision` — begin live topology reconfigure, mark detached, apply identity or visibility, complete/abort/fail topology, bind/authorize/revoke supervisor, publish/revoke trust edge, comms trust reconcile, and local endpoint publish or clear
